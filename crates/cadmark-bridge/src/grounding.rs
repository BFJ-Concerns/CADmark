// How a spatial comment's anchors are put into words for the model: the
// element, the line that produced it (or the honest alternative — several
// candidate lines, or none), the operation and its relation, and the
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
        LedgerValue::Ambiguous(candidates) => {
            out.push_str(
                "its source is ambiguous; it was produced by one of these lines (decide from the \
                 measurements and the user's words, and say which you chose):",
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
    out.push_str("\n    surrounding code:\n");
    out.push_str(&context.source_context);
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
    use cadmark_core::geometry::{EdgeId, FaceId, TopologyElement, VertexId};
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
                    element: TopologyElement::Face(FaceId(3)),
                    provenance: LedgerValue::Resolved(entry(5, SemanticOperation::Box)),
                    identification,
                    source_context: "lines 3-7:\n3 | with BuildPart():\n5 | Box(10, 10, 2)".into(),
                    neighbours: vec![TopologyElement::Edge(EdgeId(1))],
                },
                GeometryContext {
                    element: TopologyElement::Edge(EdgeId(4)),
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
                },
                GeometryContext {
                    element: TopologyElement::Edge(EdgeId(9)),
                    provenance: LedgerValue::Untraced,
                    identification: Default::default(),
                    source_context:
                        "lines 1-3:\n1 | from build123d import *\n2 | part = imported_shape".into(),
                    neighbours: vec![TopologyElement::Edge(EdgeId(8))],
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
}
