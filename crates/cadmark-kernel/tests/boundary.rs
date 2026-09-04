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

/// The file's code with comments and string literals removed: prose the
/// model reads and prose in comments may name anything; code may not.
/// Returns one entry per source line so a finding can cite its line.
fn code_only(source: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for line in source.lines() {
        let mut out = String::new();
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            if in_string {
                match (ch, escaped) {
                    ('\\', false) => escaped = true,
                    ('"', false) => in_string = false,
                    _ => escaped = false,
                }
                continue;
            }
            match ch {
                '/' if chars.peek() == Some(&'/') => break,
                '"' => in_string = true,
                // A char literal such as '"' is not a string start.
                '\'' => {
                    let rest: String = chars.clone().take(3).collect();
                    if rest.starts_with("\"'") || rest.starts_with("\\") {
                        out.push(ch);
                        for _ in 0..(if rest.starts_with("\\") { 3 } else { 2 }) {
                            if let Some(c) = chars.next() {
                                out.push(c);
                            }
                        }
                    } else {
                        out.push(ch);
                    }
                }
                _ => out.push(ch),
            }
        }
        lines.push(out);
    }
    lines
}

#[test]
fn no_crate_outside_the_kernel_names_python_build123d_or_ocp() {
    let mut violations = Vec::new();
    for file in sources_outside_the_kernel() {
        let text = std::fs::read_to_string(&file).unwrap();
        let original: Vec<&str> = text.lines().collect();
        for (number, code) in code_only(&text).iter().enumerate() {
            let line = original[number];
            for (needle, reason) in FORBIDDEN_OUTSIDE_THE_KERNEL {
                if code.contains(needle) {
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
fn only_code_is_inspected() {
    let one = |s: &str| code_only(s).join("\n");
    assert_eq!(one("    // TopoDS in a comment"), "    ");
    assert_eq!(one(r#"let x = "BuildPart"; TopoDS"#), "let x = ; TopoDS");
    assert_eq!(
        one(r#"assert!(s.contains("BuildPart"))"#),
        "assert!(s.contains())"
    );
    assert_eq!(one(r#"let q = "a \" BRep \" b"; PyAny"#), "let q = ; PyAny");
    // A string continued across lines stays a string; a char literal of
    // a quote does not open one.
    assert_eq!(
        code_only("let s = \"first BuildPart\nsecond TopoDS\"; PyAny").join("|"),
        "let s = |; PyAny"
    );
    assert_eq!(
        one(r#"if c == '"' { TopoDS }"#),
        r#"if c == '"' { TopoDS }"#
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
