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
        name: "sketch_profile_only",
        operations: &[
            "drawing",
            "2d",
            "flat",
            "svg",
            "dxf",
            "laser",
            "lasercut",
            "plotter",
            "cnc",
            "cutout",
            "template",
            "silhouette",
        ],
        body: include_str!("example_library/sketch_profile_only.md"),
    },
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

    // Every operation the request names gets its material: the example
    // library is consulted per operation, so a request naming four covered
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
        // A flat part is a sketch left as the result, not an extrusion.
        assert!(names("a DXF template for the laser cutter").contains(&"sketch_profile_only"));
        assert!(
            !names("a DXF template for the laser cutter").contains(&"sketched_profile_extrude")
        );
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
            for block in parameter_blocks(example.body) {
                assert!(
                    derives_a_parameter(&block),
                    "every parameter block of {} must derive a value from another \
                     parameter, not only state literals; this one does not:\n{}",
                    example.name,
                    block.join("\n")
                );
            }
        }
    }

    /// The lines of each `# Parameters` block in an example: the
    /// assignments that follow the heading, ending at the first line that
    /// is neither blank nor a plain `name = value` assignment (the
    /// `# Geometry` heading, or the first statement). Geometry below is
    /// deliberately out of scope — a derived value down in the modelling
    /// code is not a parameter block that derives.
    fn parameter_blocks(body: &str) -> Vec<Vec<&str>> {
        let mut blocks = Vec::new();
        let mut lines = body.lines();
        while let Some(line) = lines.next() {
            if line.trim() != "# Parameters" {
                continue;
            }
            let mut block = Vec::new();
            for line in lines.by_ref() {
                if line.trim().is_empty() {
                    continue;
                }
                if assigned_name(line).is_none() {
                    break;
                }
                block.push(line);
            }
            blocks.push(block);
        }
        blocks
    }

    /// The name a line assigns, when the line is a plain assignment.
    fn assigned_name(line: &str) -> Option<&str> {
        let (left, right) = line.trim().split_once('=')?;
        if right.starts_with('=') || left.ends_with(['!', '<', '>', '=']) {
            return None;
        }
        let name = left.trim();
        (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')).then_some(name)
    }

    /// Whether some assignment in this block names a parameter assigned
    /// above it.
    fn derives_a_parameter(block: &[&str]) -> bool {
        let mut assigned: Vec<&str> = Vec::new();
        for line in block {
            let Some(name) = assigned_name(line) else {
                continue;
            };
            let right = line.split_once('=').map(|(_, right)| right).unwrap_or("");
            let right = right.split('#').next().unwrap_or(right);
            if assigned.iter().any(|earlier| {
                right
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .any(|word| word == *earlier)
            }) {
                return true;
            }
            assigned.push(name);
        }
        false
    }

    #[test]
    fn a_parameter_block_of_bare_literals_is_not_accepted() {
        // The shape the whole-body scan used to accept: literal
        // parameters, with the only derived value down in the geometry.
        let literal_block = "# Parameters\nwidth = 60.0\ndepth = 40.0\n\n\
                             with BuildPart():\n    helper = width / 2\n";
        let blocks = parameter_blocks(literal_block);
        assert_eq!(blocks.len(), 1);
        assert!(!derives_a_parameter(&blocks[0]));
        // ...and the shape that is: the block itself derives.
        let derived_block = "# Parameters\nwidth = 60.0\ndepth = width * 2 / 3\n";
        assert!(derives_a_parameter(&parameter_blocks(derived_block)[0]));
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
