// The kernel's component boundary, enforced by reading the workspace: no
// crate outside `cadmark-kernel` names a build123d or OCP class, handles a
// Python object, or branches on a kernel-specific operation name, and the
// provenance-resolution interface exposes no kernel-specific type. A
// build that reintroduces any of these fails here before it ships.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cadmark_core::ledger::SemanticOperation;

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

/// Every operation is named through an exhaustive match so that extending the
/// enum makes this boundary test fail to compile until its vocabulary is
/// deliberately updated.
fn semantic_operation_name(operation: SemanticOperation) -> &'static str {
    match operation {
        SemanticOperation::Box => "Box",
        SemanticOperation::Cylinder => "Cylinder",
        SemanticOperation::Sphere => "Sphere",
        SemanticOperation::Cone => "Cone",
        SemanticOperation::Torus => "Torus",
        SemanticOperation::Wedge => "Wedge",
        SemanticOperation::Extrude => "Extrude",
        SemanticOperation::Revolve => "Revolve",
        SemanticOperation::Loft => "Loft",
        SemanticOperation::Sweep => "Sweep",
        SemanticOperation::Thicken => "Thicken",
        SemanticOperation::Shell => "Shell",
        SemanticOperation::Draft => "Draft",
        SemanticOperation::Split => "Split",
        SemanticOperation::BooleanFuse => "BooleanFuse",
        SemanticOperation::BooleanCut => "BooleanCut",
        SemanticOperation::BooleanCommon => "BooleanCommon",
        SemanticOperation::Fillet => "Fillet",
        SemanticOperation::Chamfer => "Chamfer",
    }
}

fn semantic_operation_names() -> Vec<String> {
    SemanticOperation::ALL
        .iter()
        .copied()
        .map(semantic_operation_name)
        .map(str::to_owned)
        .collect()
}

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

fn contains_unqualified_identifier(code: &str, identifier: &str) -> bool {
    code.match_indices(identifier).any(|(start, _)| {
        let before = code[..start].chars().rev().find(|ch| !ch.is_whitespace());
        let after = code[start + identifier.len()..].chars().next();
        before != Some(':')
            && !before.is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
            && !after.is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
    })
}

fn starts_plain_use_statement(line: &str) -> bool {
    line.starts_with("use ")
        || line.starts_with("pub use ")
        || (line.starts_with("pub(") && line.contains(") use "))
}

fn starts_use_statement(line: &str) -> bool {
    let line = line.trim_start();
    if starts_plain_use_statement(line) {
        return true;
    }
    line.strip_prefix("#[")
        .and_then(|attribute| attribute.split_once(']'))
        .is_some_and(|(_, statement)| starts_plain_use_statement(statement.trim_start()))
}

/// Local names made available by a SemanticOperation use declaration in this
/// file, mapped to the operation variant each one denotes.
fn imported_operation_variants(code: &[String]) -> BTreeMap<String, String> {
    let known = semantic_operation_names();
    let mut imported = BTreeMap::new();
    let mut use_statement = String::new();
    for line in code {
        if use_statement.is_empty() {
            if starts_use_statement(line) {
                use_statement.push_str(line);
            } else {
                continue;
            }
        } else {
            use_statement.push('\n');
            use_statement.push_str(line);
        }
        if !use_statement.contains(';') {
            continue;
        }

        let Some((_, import)) = use_statement.split_once("SemanticOperation::") else {
            use_statement.clear();
            continue;
        };
        if let Some(names) = import
            .strip_prefix('{')
            .and_then(|rest| rest.split_once('}'))
        {
            for name in names.0.split(',').map(str::trim) {
                let (name, local) = name
                    .split_once(" as ")
                    .map_or((name, name), |(name, local)| (name.trim(), local.trim()));
                if known.iter().any(|known_name| known_name == name) {
                    imported.insert(local.to_owned(), name.to_owned());
                }
            }
        } else if import.starts_with('*') {
            imported.extend(known.iter().cloned().map(|name| (name.clone(), name)));
        } else {
            let name: String = import
                .chars()
                .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                .collect();
            let local = import[name.len()..]
                .trim_start()
                .strip_prefix("as ")
                .and_then(|rest| {
                    let alias: String = rest
                        .chars()
                        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                        .collect();
                    (!alias.is_empty()).then_some(alias)
                })
                .unwrap_or_else(|| name.clone());
            if known.iter().any(|known_name| known_name == &name) {
                imported.insert(local, name);
            }
        }
        use_statement.clear();
    }
    imported
}

fn operation_in_pattern(
    pattern: &str,
    imported: &BTreeMap<String, String>,
    permit_self: bool,
) -> Option<String> {
    if let Some(variant) = semantic_operation_names().into_iter().find(|variant| {
        pattern.contains(&format!("SemanticOperation::{variant}"))
            || (!permit_self && pattern.contains(&format!("Self::{variant}")))
    }) {
        return Some(variant);
    }
    imported.iter().find_map(|(local, variant)| {
        contains_unqualified_identifier(pattern, local).then(|| variant.clone())
    })
}

/// Returns the operation variant when a line branches on it. Match arms only
/// inspect the pattern before `=>`, so constructing an operation as a match
/// arm's value remains allowed. `Self::` is permitted only inside the enum's
/// inherent implementation, where it defines the vocabulary itself.
fn operation_branch_on_line(
    code: &str,
    imported: &BTreeMap<String, String>,
    permit_self: bool,
) -> Option<String> {
    if let Some((pattern, _)) = code.split_once("=>") {
        return operation_in_pattern(pattern, imported, permit_self);
    }
    if code.contains("matches!") {
        return operation_in_pattern(code, imported, permit_self);
    }
    let Some((_, condition)) = code.split_once("if") else {
        return None;
    };
    let condition = condition.split('{').next().unwrap_or(condition);
    if condition.contains("==") || condition.contains("!=") {
        operation_in_pattern(condition, imported, permit_self)
    } else {
        None
    }
}

fn semantic_operation_impl_lines(code: &[String]) -> BTreeSet<usize> {
    let mut lines = BTreeSet::new();
    let mut depth = 0usize;
    let mut semantic_impl_depth = None;
    for (number, line) in code.iter().enumerate() {
        if line.contains("impl SemanticOperation") && line.contains('{') {
            semantic_impl_depth = Some(depth + 1);
        }
        if semantic_impl_depth.is_some() {
            lines.insert(number);
        }
        depth += line.matches('{').count();
        depth = depth.saturating_sub(line.matches('}').count());
        if semantic_impl_depth.is_some_and(|impl_depth| depth < impl_depth) {
            semantic_impl_depth = None;
        }
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
fn no_crate_outside_the_kernel_branches_on_a_semantic_operation() {
    let mut violations = Vec::new();
    for file in sources_outside_the_kernel() {
        let text = std::fs::read_to_string(&file).unwrap();
        let original: Vec<&str> = text.lines().collect();
        let code = code_only(&text);
        let imported = imported_operation_variants(&code);
        let semantic_impl_lines = semantic_operation_impl_lines(&code);
        for (number, line) in code.iter().enumerate() {
            if let Some(operation) =
                operation_branch_on_line(line, &imported, semantic_impl_lines.contains(&number))
            {
                violations.push(format!(
                    "{}:{}: branches on kernel-specific operation {operation}: {}",
                    file.strip_prefix(workspace_root()).unwrap().display(),
                    number + 1,
                    original[number].trim()
                ));
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
fn operation_branch_detection_allows_data_and_rejects_branches() {
    let no_imports = BTreeMap::new();
    let imported = imported_operation_variants(&code_only(
        "use cadmark_core::ledger::SemanticOperation::{\n    Chamfer, Fillet,\n};",
    ));
    let single_import = imported_operation_variants(&code_only(
        "use cadmark_core::ledger::SemanticOperation::Fillet;",
    ));
    let glob_import = imported_operation_variants(&code_only(
        "use cadmark_core::ledger::SemanticOperation::*;",
    ));
    let alias_import = imported_operation_variants(&code_only(
        "use cadmark_core::ledger::SemanticOperation::Fillet as Round;",
    ));
    let attribute_import = imported_operation_variants(&code_only(
        "#[allow(dead_code)] use cadmark_core::ledger::SemanticOperation::Fillet;",
    ));
    let data_only = code_only(
        "let cause = describe(\n    cadmark_core::ledger::SemanticOperation::Fillet,\n);",
    );
    assert!(
        imported_operation_variants(&data_only).is_empty(),
        "an ordinary data reference is not a use statement"
    );
    assert_eq!(
        operation_branch_on_line(
            "let operation = SemanticOperation::Fillet;",
            &no_imports,
            false
        ),
        None
    );
    assert_eq!(
        operation_branch_on_line("SemanticOperation::Fillet => render(),", &no_imports, false),
        Some("Fillet".to_owned())
    );
    assert_eq!(
        operation_branch_on_line(
            "if operation == SemanticOperation::Fillet {",
            &no_imports,
            false
        ),
        Some("Fillet".to_owned())
    );
    assert_eq!(
        operation_branch_on_line(
            "matches!(operation, SemanticOperation::Fillet)",
            &no_imports,
            false
        ),
        Some("Fillet".to_owned())
    );
    assert_eq!(
        operation_branch_on_line("Fillet => render(),", &imported, false),
        Some("Fillet".to_owned())
    );
    assert_eq!(
        operation_branch_on_line("Fillet => render(),", &single_import, false),
        Some("Fillet".to_owned())
    );
    assert_eq!(
        operation_branch_on_line("Fillet => render(),", &glob_import, false),
        Some("Fillet".to_owned())
    );
    assert_eq!(
        operation_branch_on_line("Round => render(),", &alias_import, false),
        Some("Fillet".to_owned())
    );
    assert_eq!(
        operation_branch_on_line("Fillet => render(),", &attribute_import, false),
        Some("Fillet".to_owned())
    );
    assert_eq!(
        operation_branch_on_line("Corner::Fillet => render(),", &single_import, false),
        None,
        "an imported operation does not make another enum's qualified variant a kernel branch"
    );
    assert_eq!(
        operation_branch_on_line("Self::Fillet => render(),", &no_imports, false),
        Some("Fillet".to_owned())
    );
    assert_eq!(
        operation_branch_on_line("Self::Fillet => render(),", &no_imports, true),
        None,
        "only the enum's own implementation may branch through Self"
    );
    assert_eq!(
        operation_branch_on_line("0 => SemanticOperation::Fillet,", &no_imports, false),
        None,
        "constructing an operation as a match arm value is not an operation branch"
    );
    assert_eq!(
        operation_branch_on_line(
            &code_only(r#"if name == "Fillet" {"#).join("\n"),
            &no_imports,
            false
        ),
        None,
        "string-literal branches are intentionally outside code_only's boundary"
    );
}

#[test]
fn semantic_operation_vocabulary_cannot_drift_silently() {
    let names = semantic_operation_names();
    assert_eq!(semantic_operation_name(SemanticOperation::Fillet), "Fillet");
    assert!(names.contains(&"Fillet".to_owned()));
    assert!(names.contains(&"Chamfer".to_owned()));
    assert!(!names.iter().any(|name| name.contains('(')));
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
