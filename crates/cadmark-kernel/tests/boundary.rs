// The kernel's component boundary, enforced by reading the workspace: no
// crate outside `cadmark-kernel` names a build123d or OCP class, handles a
// Python object, or branches on a kernel-specific operation name, and the
// provenance-resolution interface exposes no kernel-specific type. A
// build that reintroduces any of these fails here before it ships.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

/// Every Rust source file of the crates outside the kernel.
fn sources_outside_the_kernel() -> Vec<PathBuf> {
    let crates = workspace_root().join("crates");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&crates).expect("crates directory") {
        let entry = entry.unwrap();
        if entry.file_name() == "cadmark-kernel" {
            continue;
        }
        collect_rust_files(&entry.path(), &mut files);
    }
    assert!(
        files.len() > 10,
        "expected the other crates' sources, found {}",
        files.len()
    );
    files
}

fn collect_rust_files(dir: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

/// Text that only the kernel may contain. Each entry is a substring and
/// the reason it marks a boundary crossing.
const FORBIDDEN_OUTSIDE_THE_KERNEL: &[(&str, &str)] = &[
    ("pyo3", "handles a Python object"),
    ("Python::", "handles a Python object"),
    ("PyAny", "handles a Python object"),
    ("PyDict", "handles a Python object"),
    ("build123d::", "names a build123d class"),
    ("BuildPart", "names a build123d class"),
    ("TopoDS", "names an OCP class"),
    ("BRep", "names an OCP class"),
    ("OCP.", "names an OCP module"),
    (
        "cadmark_kernel::execution",
        "reaches past the worker boundary into in-process execution",
    ),
    (
        "cadmark_kernel::tessellation",
        "reaches past the worker boundary",
    ),
    (
        "cadmark_kernel::provenance",
        "reaches past the worker boundary",
    ),
    (
        "cadmark_kernel::measurement",
        "reaches past the worker boundary",
    ),
    ("cadmark_kernel::export", "reaches past the worker boundary"),
];

/// Lines that legitimately mention a forbidden word: the prompt text
/// the model reads, and prose in comments. Only code is checked.
fn is_code(line: &str) -> bool {
    let trimmed = line.trim_start();
    !(trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("//!"))
}

#[test]
fn no_crate_outside_the_kernel_names_python_build123d_or_ocp() {
    let mut violations = Vec::new();
    for file in sources_outside_the_kernel() {
        let text = std::fs::read_to_string(&file).unwrap();
        // String literals in tests that assert on the model's *words*
        // are prose, not code.
        for (number, line) in text.lines().enumerate() {
            if !is_code(line) || (line.contains('"') && line.contains("assert!")) {
                continue;
            }
            for (needle, reason) in FORBIDDEN_OUTSIDE_THE_KERNEL {
                if line.contains(needle) {
                    violations.push(format!(
                        "{}:{}: {reason} ({needle}): {}",
                        file.strip_prefix(workspace_root()).unwrap().display(),
                        number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "kernel boundary crossed:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_provenance_resolution_interface_is_kernel_neutral() {
    // The types the rest of the system resolves provenance through live in
    // the core crate, which depends on no kernel: its manifest is the proof.
    let core_manifest =
        std::fs::read_to_string(workspace_root().join("crates/cadmark-core/Cargo.toml")).unwrap();
    for forbidden in ["pyo3", "cadmark-kernel", "build123d", "ocp"] {
        assert!(
            !core_manifest.to_ascii_lowercase().contains(forbidden),
            "cadmark-core depends on {forbidden}"
        );
    }
    // And the renderer, which consumes the mesh, depends on the core alone
    // among workspace crates.
    let renderer_manifest =
        std::fs::read_to_string(workspace_root().join("crates/cadmark-renderer/Cargo.toml"))
            .unwrap();
    assert!(!renderer_manifest.contains("cadmark-kernel"));
}

#[test]
fn the_worker_protocol_carries_only_plain_data() {
    // Everything that crosses the process boundary serialises: a type that
    // held a Python object could not.
    let protocol =
        std::fs::read_to_string(workspace_root().join("crates/cadmark-kernel/src/protocol.rs"))
            .unwrap();
    for forbidden in ["Py<", "PyAny", "Bound<", "pyo3"] {
        assert!(
            !protocol.contains(forbidden),
            "protocol.rs mentions {forbidden}"
        );
    }
    assert!(protocol.contains("Serialize, Deserialize"));
}
