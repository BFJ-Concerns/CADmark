// How a spatial comment's anchors are put into words for the model: the
// element, the line that produced it (or the honest alternative — several
// candidate lines, the one the user chose among them, or none), the
// operation and its relation, and the
// element's measurements. This text is the grounding the pointing channel
// exists to deliver; every anchor of a comment is rendered, in order.

use cadmark_core::geometry::{GeometryContext, TopologyElement};
use cadmark_core::ledger::LedgerValue;

/// One comment as the model receives it: the user's words and the
/// anchors they point at.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundedComment {
    pub text: String,
    pub anchors: Vec<GeometryContext>,
}

/// Render a comment and its anchors as the user message text.
pub fn render_comment(comment: &GroundedComment) -> String {
    let mut out = String::new();
    match comment.anchors.len() {
        0 => {}
        1 => out.push_str("The user selected a geometry element in the viewport and commented on it.\n"),
        n => out.push_str(&format!(
            "The user selected {n} geometry elements in the viewport and commented on them together.\n"
        )),
    }
    for anchor in &comment.anchors {
        out.push_str(&render_anchor(anchor));
    }
    if !comment.anchors.is_empty() {
        out.push('\n');
    }
    out.push_str(&comment.text);
    out
}

fn render_anchor(context: &GeometryContext) -> String {
    let mut out = format!("- {}: ", context.element.display_label());
    match &context.provenance {
        LedgerValue::Resolved(entry) => out.push_str(&format!(
            "{} {} at line {}: `{}`",
            entry.relation.display_phrase(),
            entry.operation.display_name(),
            entry.source.line,
            entry.source.code
        )),
        LedgerValue::Ambiguous(candidates) => match &context.chosen_candidate {
            // The user was offered the candidates and picked one. That is
            // knowledge the model cannot derive, so it is stated as theirs
            // and the candidates they rejected are not sent.
            Some(chosen) => out.push_str(&format!(
                "its source was ambiguous and the user chose which line it is: {} {} at line \
                 {}: `{}`",
                chosen.relation.display_phrase(),
                chosen.operation.display_name(),
                chosen.source.line,
                chosen.source.code
            )),
            None => {
                out.push_str(
                    "its source is ambiguous; it was produced by one of these lines (decide from \
                     the measurements and the user's words, and say which you chose):",
                );
                for candidate in candidates {
                    out.push_str(&format!(
                        "\n    - {} {} at line {}: `{}`",
                        candidate.relation.display_phrase(),
                        candidate.operation.display_name(),
                        candidate.source.line,
                        candidate.source.code
                    ));
                }
            }
        },
        LedgerValue::Untraced => out.push_str(
            "no source line is known for it (it came from an operation CADmark cannot trace); \
             locate it from the measurements",
        ),
    }
    if !context.identification.is_empty() {
        let mut identification: Vec<_> = context.identification.iter().collect();
        identification.sort();
        out.push_str("\n    measured: ");
        out.push_str(
            &identification
                .iter()
                .map(|(key, value)| format!("{key} {value}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    out.push_str("\n    sketch: ");
    out.push_str(&context.sketch.describe());
    out.push_str("\n    surrounding code:\n");
    if context.source_context.trim().is_empty() {
        out.push_str("The executed script text is unavailable.");
    } else {
        out.push_str(&context.source_context);
    }
    out.push_str("\n    neighbours: ");
    if context.neighbours.is_empty() {
        out.push_str("none measured");
    } else {
        out.push_str(
            &context
                .neighbours
                .iter()
                .map(TopologyElement::display_label)
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadmark_core::geometry::{EdgeId, FaceId, PickedElement, TopologyElement, VertexId};
    use cadmark_core::ledger::{ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef};

    fn entry(line: u32, operation: SemanticOperation) -> ProvenanceEntry {
        ProvenanceEntry {
            source: SourceRef {
                line,
                code: format!("line {line}"),
            },
            operation,
            operation_id: u64::from(line),
            relation: ProvenanceRelation::Modified,
        }
    }

    /// An edge two lines could have produced, optionally with the user's
    /// choice among them recorded.
    fn ambiguous_edge(chosen: Option<ProvenanceEntry>) -> GroundedComment {
        GroundedComment {
            text: "round this".into(),
            anchors: vec![GeometryContext {
                part: None,
                element: PickedElement::Solid(TopologyElement::Edge(EdgeId(4))),
                provenance: LedgerValue::Ambiguous(vec![
                    entry(2, SemanticOperation::Box),
                    entry(3, SemanticOperation::Fillet),
                ]),
                identification: Default::default(),
                source_context: String::new(),
                neighbours: Vec::new(),
                chosen_candidate: chosen,
                sketch: Default::default(),
            }],
        }
    }

    #[test]
    fn a_chosen_candidate_is_the_only_line_the_model_is_given() {
        let text = render_comment(&ambiguous_edge(Some(entry(3, SemanticOperation::Fillet))));
        assert!(
            text.contains("the user chose which line it is: modified by fillet at line 3"),
            "the model must be told the choice was the user's: {text}"
        );
        assert!(
            !text.contains("line 2"),
            "the rejected candidate must not reach the model: {text}"
        );
        assert!(!text.contains("decide from"));
    }

    #[test]
    fn an_unchosen_ambiguity_still_reaches_the_model_with_every_candidate() {
        let text = render_comment(&ambiguous_edge(None));
        assert!(text.contains("its source is ambiguous"));
        assert!(text.contains("    - modified by box at line 2"));
        assert!(text.contains("    - modified by fillet at line 3"));
    }

    #[test]
    fn a_plain_chat_message_is_rendered_as_itself() {
        let comment = GroundedComment {
            text: "make a box".into(),
            anchors: Vec::new(),
        };
        assert_eq!(render_comment(&comment), "make a box");
    }

    #[test]
    fn every_anchor_is_rendered_with_its_source_and_measurements() {
        let mut identification = std::collections::HashMap::new();
        identification.insert("surface".to_string(), "plane".to_string());
        identification.insert("area_mm2".to_string(), "200.00".to_string());
        let comment = GroundedComment {
            text: "round this".into(),
            anchors: vec![
                GeometryContext {
                    part: None,
                    sketch: Default::default(),
                    element: PickedElement::Solid(TopologyElement::Face(FaceId(3))),
                    provenance: LedgerValue::Resolved(entry(5, SemanticOperation::Box)),
                    identification,
                    source_context: "lines 3-7:\n3 | with BuildPart():\n5 | Box(10, 10, 2)".into(),
                    neighbours: vec![TopologyElement::Edge(EdgeId(1))],
                    chosen_candidate: None,
                },
                GeometryContext {
                    part: None,
                    sketch: Default::default(),
                    element: PickedElement::Solid(TopologyElement::Edge(EdgeId(4))),
                    provenance: LedgerValue::Ambiguous(vec![
                        entry(2, SemanticOperation::Box),
                        entry(3, SemanticOperation::Fillet),
                    ]),
                    identification: Default::default(),
                    source_context: "lines 1-5:\n2 | Box(10, 10, 2)\n3 | fillet(...)".into(),
                    neighbours: vec![
                        TopologyElement::Face(FaceId(0)),
                        TopologyElement::Vertex(VertexId(2)),
                    ],
                    chosen_candidate: None,
                },
                GeometryContext {
                    part: None,
                    sketch: Default::default(),
                    element: PickedElement::Solid(TopologyElement::Edge(EdgeId(9))),
                    provenance: LedgerValue::Untraced,
                    identification: Default::default(),
                    source_context:
                        "lines 1-3:\n1 | from build123d import *\n2 | part = imported_shape".into(),
                    neighbours: vec![TopologyElement::Edge(EdgeId(8))],
                    chosen_candidate: None,
                },
            ],
        };
        let text = render_comment(&comment);
        assert!(text.starts_with("The user selected 3 geometry elements"));
        assert!(text.contains("- face 3: modified by box at line 5: `line 5`"));
        assert!(text.contains("measured: area_mm2 200.00, surface plane"));
        assert!(text.contains("surrounding code:\nlines 3-7"));
        assert!(text.contains("neighbours: edge 1"));
        assert!(text.contains("- edge 4: its source is ambiguous"));
        assert!(text.contains("    - modified by fillet at line 3"));
        assert!(text.contains("lines 1-5:\n2 | Box(10, 10, 2)"));
        assert!(text.contains("neighbours: face 0, vertex 2"));
        assert!(text.contains("- edge 9: no source line is known"));
        assert!(text.contains("lines 1-3:\n1 | from build123d import *"));
        assert!(text.contains("neighbours: edge 8"));
        assert!(text.ends_with("\nround this"));
    }

    #[test]
    fn an_anchor_carries_its_sketch_route_or_says_there_is_none() {
        let anchor = |sketch: cadmark_core::sketch_lineage::SketchLineage| GeometryContext {
            part: None,
            sketch,
            element: PickedElement::Solid(TopologyElement::Face(FaceId(0))),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: "1 | from build123d import *".into(),
            neighbours: Vec::new(),
            chosen_candidate: None,
        };

        let drawn = render_comment(&GroundedComment {
            text: "widen this".into(),
            anchors: vec![anchor(
                cadmark_core::sketch_lineage::SketchLineage::Resolved(
                    cadmark_core::sketch_lineage::SketchSource {
                        source: SourceRef {
                            line: 5,
                            code: "Rectangle(20, 10)".into(),
                        },
                        object: "Rectangle".into(),
                    },
                ),
            )],
        });
        assert!(
            drawn.contains("sketch: drawn by Rectangle at line 5"),
            "{drawn}",
        );

        // The model must be told the route is absent, not left to infer a
        // line from the surrounding code.
        let unreachable = render_comment(&GroundedComment {
            text: "widen this".into(),
            anchors: vec![anchor(
                cadmark_core::sketch_lineage::SketchLineage::NoRoute(
                    cadmark_core::sketch_lineage::NoSketchRoute::CleanUpStep,
                ),
            )],
        });
        assert!(
            unreachable.contains("sketch: no sketch route:"),
            "{unreachable}",
        );
    }

    #[test]
    fn legacy_anchor_without_source_context_states_what_is_unavailable() {
        let text = render_comment(&GroundedComment {
            text: "adjust this".into(),
            anchors: vec![GeometryContext {
                part: None,
                sketch: Default::default(),
                element: PickedElement::Solid(TopologyElement::Face(FaceId(0))),
                provenance: LedgerValue::Untraced,
                identification: Default::default(),
                source_context: String::new(),
                neighbours: Vec::new(),
                chosen_candidate: None,
            }],
        });

        assert!(text.contains("surrounding code:\nThe executed script text is unavailable."));
    }
}
