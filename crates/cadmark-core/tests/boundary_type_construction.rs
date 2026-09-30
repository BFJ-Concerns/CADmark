//! Boundary-crossing types are built in tests on their shared base.
//!
//! A type that crosses a crate boundary — a geometry context, a kernel
//! result, a chat message — gains fields as the product grows. Where every
//! test builds it as a full literal, each added field edits every fixture
//! in the workspace, most of which never read it. So tests build these
//! types on the type's own constructor or `Default`, with struct-update
//! syntax naming only the fields the test reads, and this test reads the
//! tree to hold them to it: a test-side literal of a listed type without a
//! `..base` fails here, naming the file and line.
//!
//! Test-side means an integration test file, or anything from a file's
//! first `#[cfg(test)]` to its end. Production code is exempt on purpose:
//! it is where a new field must be filled, and the compiler already says
//! so there.

use std::path::{Path, PathBuf};

/// The types held to the rule, with the base a test builds on. A type
/// joins the list when it crosses a crate boundary and tests build it by
/// hand; its constructor takes only what the type cannot default.
const BOUNDARY_TYPES: &[(&str, &str)] = &[
    (
        "GeometryContext",
        "GeometryContext::new(element, provenance)",
    ),
    (
        "ToolActivity",
        "ToolActivity::begin(call_id, tool, arguments)",
    ),
    ("ExecutedModel", "ExecutedModel::of(form)"),
    ("SolidResult", "SolidResult::new(summary, file)"),
    ("ExecutedPart", "ExecutedPart::new(id, name, file)"),
    ("SketchResult", "SketchResult::new(profile, file)"),
    ("ModelSummary", "ModelSummary::default()"),
    ("SolidValidity", "SolidValidity::default()"),
    ("SketchProfile", "SketchProfile::default()"),
    ("SketchCurve", "SketchCurve::default()"),
    ("SketchRegion", "SketchRegion::default()"),
    ("SketchCorner", "SketchCorner::default()"),
];

/// A test-side literal of a listed type built without its base.
#[derive(Debug, PartialEq, Eq)]
struct BareLiteral {
    path: PathBuf,
    line: usize,
    type_name: &'static str,
    base: &'static str,
}

impl std::fmt::Display for BareLiteral {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}: `{} {{ .. }}` is built field by field in test code; build it on `{}` \
             with `..` and name only the fields the test reads",
            self.path.display(),
            self.line,
            self.type_name,
            self.base
        )
    }
}

#[test]
fn test_side_literals_of_boundary_types_build_on_their_base() {
    let root = workspace_root();
    let mut violations = Vec::new();
    for path in rust_sources(&root.join("crates")) {
        let source = std::fs::read_to_string(&path).expect("a readable source file");
        let Some((first_line, region)) = test_side_region(&path, &source) else {
            continue;
        };
        let code = code_only(region);
        for &(type_name, base) in BOUNDARY_TYPES {
            for line in bare_literal_lines(&code, type_name) {
                violations.push(BareLiteral {
                    path: path.strip_prefix(&root).unwrap().to_path_buf(),
                    line: first_line + line,
                    type_name,
                    base,
                });
            }
        }
    }
    violations.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    let report: Vec<String> = violations.iter().map(ToString::to_string).collect();
    assert!(
        violations.is_empty(),
        "{} test-side literal(s) built without their base:\n{}",
        violations.len(),
        report.join("\n")
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root")
}

fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// The part of a file this test reads, with the one-based line it starts
/// on: the whole of an integration test file, or a source file from its
/// first `#[cfg(test)]` on. `None` for a file with no test side.
fn test_side_region<'a>(path: &Path, source: &'a str) -> Option<(usize, &'a str)> {
    let under_tests = path
        .components()
        .any(|component| component.as_os_str() == "tests");
    if under_tests {
        return Some((1, source));
    }
    let start = source.find("#[cfg(test)]")?;
    Some((source[..start].lines().count() + 1, &source[start..]))
}

/// The code with every `//` comment cut and every string and character
/// literal emptied, line structure kept, so a mention of a type in prose
/// is not read as a literal and a brace inside a string does not end one.
fn code_only(code: &str) -> String {
    code.lines()
        .map(|line| {
            let chars: Vec<char> = line.chars().collect();
            let mut out = String::new();
            let mut index = 0;
            while index < chars.len() {
                match chars[index] {
                    '"' => {
                        out.push('"');
                        index += 1;
                        while index < chars.len() && chars[index] != '"' {
                            if chars[index] == '\\' {
                                index += 1;
                            }
                            index += 1;
                        }
                        out.push('"');
                    }
                    '\'' if chars.get(index + 2) == Some(&'\'') => {
                        out.push_str("''");
                        index += 2;
                    }
                    '\'' if chars.get(index + 1) == Some(&'\\')
                        && chars.get(index + 3) == Some(&'\'') =>
                    {
                        out.push_str("''");
                        index += 3;
                    }
                    '/' if chars.get(index + 1) == Some(&'/') => break,
                    ch => out.push(ch),
                }
                index += 1;
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Zero-based line offsets, within `code`, of every `Type {` struct
/// literal whose body carries no `..base` at its own depth. Declarations
/// (`struct Type {`, `impl Type {`, `impl Trait for Type {`) and longer
/// names that merely end in the type's are not literals and are skipped.
fn bare_literal_lines(code: &str, type_name: &str) -> Vec<usize> {
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    let mut search_from = 0;
    while let Some(relative) = code[search_from..].find(type_name) {
        let start = search_from + relative;
        let end = start + type_name.len();
        search_from = end;
        if start > 0 && is_identifier_byte(bytes[start - 1]) {
            continue;
        }
        let after = code[end..].trim_start();
        if !after.starts_with('{') || is_declaration(&code[..start]) {
            continue;
        }
        if !literal_has_base(after) {
            found.push(code[..start].matches('\n').count());
        }
    }
    found
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether the word before a `Type {` makes it a declaration or a
/// signature rather than a value: `struct Type {`, `impl Type {`,
/// `impl Trait for Type {`, `fn f() -> Type {`.
fn is_declaration(before: &str) -> bool {
    let Some(last) = before.split_whitespace().next_back() else {
        return false;
    };
    matches!(
        last,
        "struct" | "enum" | "union" | "for" | "trait" | "mod" | "dyn" | "type" | "->"
    ) || last.starts_with("impl")
}

/// Whether the struct literal opening at `body[0] == '{'` carries a
/// `..base` in field position at its own depth: `..` directly after the
/// `{` or after a field's `,`, not the `..` of a range inside a value.
/// Runs over [`code_only`] output, so no string can hold a stray brace.
fn literal_has_base(body: &str) -> bool {
    debug_assert!(body.starts_with('{'));
    let mut depth = 0usize;
    let mut previous_significant = '{';
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' | '(' | '[' => {
                depth += 1;
                previous_significant = ch;
            }
            '}' | ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return false;
                }
                previous_significant = ch;
            }
            '.' if depth == 1
                && chars.peek() == Some(&'.')
                && matches!(previous_significant, '{' | ',') =>
            {
                return true;
            }
            _ if ch.is_whitespace() => {}
            _ => previous_significant = ch,
        }
    }
    false
}

#[test]
fn a_literal_on_its_base_passes_and_a_bare_one_fails() {
    let on_base = "let c = GeometryContext { part: None, ..GeometryContext::new(e, p) };";
    assert_eq!(
        bare_literal_lines(on_base, "GeometryContext"),
        Vec::<usize>::new()
    );

    let bare = "let c = GeometryContext {\n    part: None,\n    element: e,\n};";
    assert_eq!(bare_literal_lines(bare, "GeometryContext"), vec![0]);

    let qualified = "x(\n\n  protocol::ExecutedModel { mesh: m, form: f });";
    assert_eq!(bare_literal_lines(qualified, "ExecutedModel"), vec![2]);
}

#[test]
fn declarations_ranges_and_strings_are_not_read_as_bare_literals() {
    let declarations = "pub struct SolidValidity {\n closed: bool }\nimpl SolidValidity {\n fn f() {} }\nimpl Default for SolidValidity {\n fn default() -> Self { todo!() } }\nimpl<T> From<T> for SolidValidity {}\nfn fixture() -> SolidValidity {\n todo!() }";
    assert_eq!(
        bare_literal_lines(declarations, "SolidValidity"),
        Vec::<usize>::new()
    );

    // A range inside a field value is not a struct-update base.
    let range = "SketchCurve { points: (0..3).map(f).collect(), curve_id: 1 }";
    assert_eq!(bare_literal_lines(range, "SketchCurve"), vec![0]);

    // Braces inside a string do not end the literal early.
    let braces =
        code_only("SketchCurve { curve_type: format!(\"{}\", kind), ..Default::default() }");
    assert_eq!(
        bare_literal_lines(&braces, "SketchCurve"),
        Vec::<usize>::new()
    );

    // A name that merely ends in the type's is another type.
    let other = "MySketchCurve { a: 1 }";
    assert_eq!(
        bare_literal_lines(other, "SketchCurve"),
        Vec::<usize>::new()
    );
}

#[test]
fn only_the_test_side_of_a_source_file_is_read() {
    let source = "pub struct X;\nfn f() { GeometryContext { a: 1 } }\n#[cfg(test)]\nmod tests {\n GeometryContext { a: 1 }\n}\n";
    let (first_line, region) = test_side_region(Path::new("crates/x/src/lib.rs"), source).unwrap();
    assert_eq!(first_line, 3);
    assert_eq!(bare_literal_lines(region, "GeometryContext"), vec![2]);
    assert_eq!(
        test_side_region(Path::new("crates/x/tests/it.rs"), source).map(|(line, _)| line),
        Some(1)
    );
    assert!(test_side_region(Path::new("crates/x/src/pure.rs"), "fn f() {}").is_none());
}

#[test]
fn comments_and_literal_contents_are_blanked_but_lines_are_kept() {
    let code = "a // GeometryContext { x }\n\"// GeometryContext {\" b '{' c\nd";
    let stripped = code_only(code);
    assert_eq!(stripped.lines().count(), 3);
    assert!(!stripped.contains("GeometryContext"));
    assert_eq!(stripped.lines().nth(1), Some("\"\" b '' c"));
}
