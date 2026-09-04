//! The curated few-shot example library: short, complete build123d
//! scripts chosen to show what a good answer looks like, selected per
//! request from the operations the request names and carried into the
//! turn's context.
//!
//! The documentation corpus (`doc_lookup`) says what an API does, and the
//! model consults it when it chooses. This library is the other half: it
//! shows the shape the project wants — a named parameter block with
//! derived expressions, the sketch-or-solid route named before an edit,
//! and the operations a real part needs — and it arrives whether or not
//! the model asks for it.
//!
//! Every example runs under stock build123d alone; none imports a CADmark
//! module. They illustrate, they do not prescribe: the library shows both
//! builder and algebra idioms and forbids neither, nor anything else the
//! library offers.

/// One curated example: the operations it demonstrates and its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Example {
    /// Stable identifier, used in reports and tests.
    pub name: &'static str,
    /// Lower-case operation words that select this example. A request
    /// word selects when it begins with one of these, so `revolve`
    /// catches `revolved` and `hole` catches `holes`.
    pub operations: &'static [&'static str],
    /// The example itself, as Markdown with a fenced script.
    pub body: &'static str,
}

/// The library, in the order examples are shown.
pub const LIBRARY: &[Example] = &[
    Example {
        name: "sketched_profile_extrude",
        operations: &[
            "sketch",
            "extrude",
            "extrusion",
            "profile",
            "plate",
            "outline",
            "pad",
            "boss",
            "thickness",
            "thick",
        ],
        body: include_str!("example_library/sketched_profile_extrude.md"),
    },
    Example {
        name: "revolve_profile",
        operations: &[
            "revolve",
            "revolution",
            "lathe",
            "turned",
            "bushing",
            "axisymmetric",
            "spindle",
        ],
        body: include_str!("example_library/revolve_profile.md"),
    },
    Example {
        name: "cut_and_holes",
        operations: &[
            "cut",
            "hole",
            "pocket",
            "bore",
            "counterbore",
            "drill",
            "subtract",
            "recess",
            "slot",
            "through",
        ],
        body: include_str!("example_library/cut_and_holes.md"),
    },
    Example {
        name: "fillet_and_chamfer",
        operations: &[
            "fillet", "chamfer", "round", "bevel", "deburr", "corner", "edge",
        ],
        body: include_str!("example_library/fillet_and_chamfer.md"),
    },
    Example {
        name: "algebra_mode_part",
        operations: &[
            "algebra",
            "operator",
            "fuse",
            "union",
            "boolean",
            "intersect",
            "combine",
        ],
        body: include_str!("example_library/algebra_mode_part.md"),
    },
    Example {
        name: "parametric_edit",
        operations: &[
            "parameter",
            "parametric",
            "edit",
            "adjust",
            "resize",
            "wider",
            "narrower",
            "taller",
            "shorter",
            "bigger",
            "smaller",
            "change",
        ],
        body: include_str!("example_library/parametric_edit.md"),
    },
];

/// The example shown when a request names no operation the library
/// covers: the one that carries the parameter block and the commonest
/// route into a part.
const FALLBACK: &str = "sketched_profile_extrude";

/// The examples a request selects, in library order.
pub fn select(request: &str) -> Vec<&'static Example> {
    let words: Vec<String> = request
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_lowercase())
        .collect();

    // Every operation the request names gets its material: C34 is a
    // per-operation obligation, so a request naming four covered
    // operations carries four examples. The library's own size is the
    // only bound.
    let mut selected: Vec<&'static Example> = LIBRARY
        .iter()
        .filter(|example| {
            example.operations.iter().any(|operation| {
                words
                    .iter()
                    .any(|word| word.starts_with(operation) && word.len() <= operation.len() + 3)
            })
        })
        .collect();

    if selected.is_empty() {
        selected.extend(LIBRARY.iter().filter(|example| example.name == FALLBACK));
    }
    selected
}

/// The library material a request carries into the model's context: the
/// selected examples under a heading that says what they are for.
pub fn context_block(request: &str) -> String {
    let mut block = String::from(
        "# Example library\n\nWorked build123d scripts for the operations this request names. \
         They show one good way to write this kind of part — the idiom is illustrative, not \
         required; use whatever build123d offers that fits the part best.\n",
    );
    for example in select(request) {
        block.push_str("\n---\n\n");
        block.push_str(example.body.trim_end());
        block.push('\n');
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_selects_the_examples_for_the_operations_it_names() {
        let names = |request: &str| {
            select(request)
                .into_iter()
                .map(|example| example.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names("revolve this profile about the Z axis").first(),
            Some(&"sketched_profile_extrude")
        );
        assert!(names("revolve this about the Z axis").contains(&"revolve_profile"));
        assert!(!names("revolve this about the Z axis").contains(&"fillet_and_chamfer"));
        assert!(names("round these corners").contains(&"fillet_and_chamfer"));
        assert!(!names("round these corners").contains(&"revolve_profile"));
        assert!(names("bore two holes through the boss").contains(&"cut_and_holes"));
    }

    #[test]
    fn a_request_naming_no_known_operation_still_carries_one_example() {
        assert_eq!(
            select("make me something nice")
                .into_iter()
                .map(|example| example.name)
                .collect::<Vec<_>>(),
            ["sketched_profile_extrude"]
        );
    }

    #[test]
    fn every_operation_a_request_names_carries_its_own_example() {
        let names = select("extrude the profile, revolve the boss, cut a hole, fillet the corners")
            .into_iter()
            .map(|example| example.name)
            .collect::<Vec<_>>();
        for expected in [
            "sketched_profile_extrude",
            "revolve_profile",
            "cut_and_holes",
            "fillet_and_chamfer",
        ] {
            assert!(names.contains(&expected), "{expected} was dropped");
        }
    }

    #[test]
    fn the_context_block_carries_the_selected_example_and_not_the_others() {
        let block = context_block("revolve this section about the axis");
        assert!(block.contains("revolve(axis=Axis.Z"));
        assert!(!block.contains("chamfer("));
    }

    #[test]
    fn every_example_is_stock_build123d_with_a_parameter_block() {
        for example in LIBRARY {
            assert!(
                example.body.contains("from build123d import *"),
                "{} must import build123d",
                example.name
            );
            assert!(
                !example.body.contains("cadmark"),
                "{} must not depend on a CADmark module",
                example.name
            );
            assert!(
                example.body.contains("# Parameters"),
                "{} must show a named parameter block",
                example.name
            );
            assert!(
                has_a_derived_parameter(example.body),
                "{} must derive at least one parameter from another (C33)",
                example.name
            );
        }
    }

    /// Whether any parameter assignment's right-hand side names a
    /// parameter assigned above it — a derived value rather than a
    /// literal.
    fn has_a_derived_parameter(body: &str) -> bool {
        let mut assigned: Vec<&str> = Vec::new();
        for line in body.lines() {
            let line = line.trim();
            let Some((left, right)) = line.split_once('=') else {
                continue;
            };
            if right.starts_with('=') || left.ends_with(['!', '<', '>', '=']) {
                continue;
            }
            let name = left.trim();
            if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let right = right.split('#').next().unwrap_or(right);
            let referenced = assigned.iter().any(|earlier| {
                right
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .any(|word| word == *earlier)
            });
            if referenced {
                return true;
            }
            assigned.push(name);
        }
        false
    }

    #[test]
    fn the_library_shows_more_than_one_idiom() {
        let builder = LIBRARY
            .iter()
            .any(|example| example.body.contains("with BuildPart()"));
        let algebra = LIBRARY
            .iter()
            .any(|example| example.body.contains("part = "));
        assert!(builder && algebra, "both idioms must be illustrated");
    }
}
