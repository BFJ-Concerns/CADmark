// The return leg of the pointing channel: how the AI names geometry in
// its reply, and how those names become the elements the viewport lights
// up.
//
// The convention is one element per square bracket, written the way the
// grounding already names elements to the model — `[edge 12]`, `[face 3]`,
// `[vertex 7]`. A bracket is what separates a reference from prose, so
// "the top edge" in a sentence names nothing and lights nothing.
//
// Parsing fails closed at every step. A bracket whose contents are not
// exactly a kind and a number is not a reference; a reference to an
// element the resolving scope does not contain is dropped. Nothing here
// ever widens a reference to its source line's other elements: a reply
// about one edge highlights that edge alone.
//
// The other end of the convention is the inventory: the model can only
// name an element whose ID it has seen, so `describe_elements` renders
// the current model's elements into the `run_script` result. It is
// grouped by source line and compressed into ID ranges so its size
// follows the number of operations rather than the number of elements;
// a small model additionally gets a measurement per element, which is
// what lets the model tell one edge of a fillet from another.

use cadmark_core::geometry::{EdgeId, FaceId, TopologyElement, VertexId};
use cadmark_core::ledger::{LedgerValue, ProvenanceLedger};
use cadmark_kernel::protocol::ExecutedModel;

/// Beyond this many elements the inventory drops per-element measurements
/// and lists ID ranges only. A tool result rides in the model's context
/// window every subsequent request of the turn, so the detailed form is
/// affordable only while the model is small.
const DETAIL_LIMIT: usize = 60;

/// The most references one reply can highlight. A reply naming more than
/// this has stopped being a reference to specific geometry, and the
/// renderer's highlight set is bounded anyway.
pub const MAX_REFERENCES: usize = 32;

/// What a reference is allowed to resolve against. Provenance is rebuilt
/// by every execution and never persisted, so a reference resolves only
/// against a ledger this turn produced, or against the elements the user
/// pointed at in this turn's own comments. An ID quoted from an earlier
/// turn has no scope to resolve in and highlights nothing.
pub enum ReferenceScope<'a> {
    /// A script executed in this turn: any element of its ledger.
    Model(&'a ExecutedModel),
    /// Nothing executed this turn: only the elements the user's comments
    /// anchored, which came from the model on screen.
    Anchors(&'a [TopologyElement]),
}

impl ReferenceScope<'_> {
    fn contains(&self, element: &TopologyElement) -> bool {
        match self {
            Self::Model(model) => in_ledger(&model.ledger, element),
            Self::Anchors(anchors) => anchors.contains(element),
        }
    }
}

fn in_ledger(ledger: &ProvenanceLedger, element: &TopologyElement) -> bool {
    match element {
        TopologyElement::Face(id) => ledger.lookup_face(*id).is_some(),
        TopologyElement::Edge(id) => ledger.lookup_edge(*id).is_some(),
        TopologyElement::Vertex(id) => ledger.lookup_vertex(*id).is_some(),
    }
}

/// The elements `reply` references and the scope can account for, in the
/// order they appear, without repeats. Prose that merely mentions
/// geometry yields nothing, and so does a reference the scope does not
/// contain.
pub fn resolve_references(reply: &str, scope: &ReferenceScope<'_>) -> Vec<TopologyElement> {
    let mut resolved: Vec<TopologyElement> = Vec::new();
    for element in parse_references(reply) {
        if !scope.contains(&element) || resolved.contains(&element) {
            continue;
        }
        resolved.push(element);
        if resolved.len() == MAX_REFERENCES {
            break;
        }
    }
    resolved
}

/// Every well-formed reference in `reply`, in order, before any check
/// that the elements exist.
fn parse_references(reply: &str) -> Vec<TopologyElement> {
    let mut references = Vec::new();
    let bytes = reply.as_bytes();
    let mut start = 0;
    while let Some(open) = bytes[start..].iter().position(|byte| *byte == b'[') {
        let open = start + open;
        let Some(close) = bytes[open + 1..].iter().position(|byte| *byte == b']') else {
            break;
        };
        let close = open + 1 + close;
        if let Some(element) = parse_reference(&reply[open + 1..close]) {
            references.push(element);
        }
        start = close + 1;
    }
    references
}

/// One bracket's contents as an element, or None when it is anything but
/// a kind and a number.
fn parse_reference(inner: &str) -> Option<TopologyElement> {
    let (kind, id) = inner.trim().split_once(char::is_whitespace)?;
    let id: u32 = id.trim().parse().ok()?;
    match kind.trim().to_ascii_lowercase().as_str() {
        "face" => Some(TopologyElement::Face(FaceId(id))),
        "edge" => Some(TopologyElement::Edge(EdgeId(id))),
        "vertex" => Some(TopologyElement::Vertex(VertexId(id))),
        _ => None,
    }
}

/// The elements of `model` as the AI reads them in a tool result: what it
/// may reference, and enough about each to tell them apart.
pub fn describe_elements(model: &ExecutedModel) -> String {
    let ledger = &model.ledger;
    let total = ledger.len();
    if total == 0 {
        return String::new();
    }

    let mut text = String::from("\nElements of this model you can reference in your reply");
    if total <= DETAIL_LIMIT {
        text.push_str(", with the measurement that tells them apart:\n");
        for element in elements_in_order(ledger) {
            text.push_str(&format!(
                "- [{}]: {}{}\n",
                element.display_label(),
                source_of(ledger, &element),
                measurement_of(model, &element)
            ));
        }
    } else {
        text.push_str(" (too many to list singly, so by source):\n");
        for (source, elements) in grouped_by_source(ledger) {
            text.push_str(&format!("- {source}: {}\n", ranges_of(&elements)));
        }
        text.push_str(
            "Ask for a render, or work from the user's own selections, when you need to \
             tell two elements of one operation apart.\n",
        );
    }
    text
}

/// Faces, then edges, then vertices, each in ID order.
fn elements_in_order(ledger: &ProvenanceLedger) -> Vec<TopologyElement> {
    let mut faces: Vec<_> = ledger.face_ids().collect();
    faces.sort_by_key(|id| id.0);
    let mut edges: Vec<_> = ledger.edge_ids().collect();
    edges.sort_by_key(|id| id.0);
    let mut vertices: Vec<_> = ledger.vertex_ids().collect();
    vertices.sort_by_key(|id| id.0);
    faces
        .into_iter()
        .map(TopologyElement::Face)
        .chain(edges.into_iter().map(TopologyElement::Edge))
        .chain(vertices.into_iter().map(TopologyElement::Vertex))
        .collect()
}

fn provenance<'a>(
    ledger: &'a ProvenanceLedger,
    element: &TopologyElement,
) -> Option<&'a LedgerValue> {
    match element {
        TopologyElement::Face(id) => ledger.lookup_face(*id),
        TopologyElement::Edge(id) => ledger.lookup_edge(*id),
        TopologyElement::Vertex(id) => ledger.lookup_vertex(*id),
    }
}

fn source_of(ledger: &ProvenanceLedger, element: &TopologyElement) -> String {
    match provenance(ledger, element) {
        Some(value) => value.describe(),
        None => "no source line".to_string(),
    }
}

/// The one measurement that distinguishes this element from its siblings:
/// where it is, and how big it is.
fn measurement_of(model: &ExecutedModel, element: &TopologyElement) -> String {
    match element {
        TopologyElement::Face(id) => model.descriptors.face(*id).map(|face| {
            format!(
                ", {} face, area {} mm², centre {}",
                face.surface_type,
                number(face.area),
                point(face.centre)
            )
        }),
        TopologyElement::Edge(id) => model.descriptors.edge(*id).map(|edge| {
            format!(
                ", {} edge, length {} mm, centre {}",
                edge.curve_type,
                number(edge.length),
                point(edge.centre)
            )
        }),
        TopologyElement::Vertex(id) => model
            .descriptors
            .vertex(*id)
            .map(|vertex| format!(", at {}", point(vertex.position))),
    }
    .unwrap_or_default()
}

/// Every element grouped under the source that made it, faces then edges
/// then vertices within each group.
fn grouped_by_source(ledger: &ProvenanceLedger) -> Vec<(String, Vec<TopologyElement>)> {
    let mut groups: Vec<(String, Vec<TopologyElement>)> = Vec::new();
    for element in elements_in_order(ledger) {
        let source = source_of(ledger, &element);
        match groups.iter_mut().find(|(known, _)| *known == source) {
            Some((_, elements)) => elements.push(element),
            None => groups.push((source, vec![element])),
        }
    }
    groups
}

/// Consecutive IDs of one kind written as a range, so a group's size
/// follows its number of runs rather than its number of elements.
fn ranges_of(elements: &[TopologyElement]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut run: Option<(&'static str, u32, u32)> = None;
    for element in elements {
        let (kind, id) = kind_and_id(element);
        match run {
            Some((open_kind, first, last)) if open_kind == kind && id == last + 1 => {
                run = Some((kind, first, id));
            }
            Some((open_kind, first, last)) => {
                parts.push(run_label(open_kind, first, last));
                run = Some((kind, id, id));
            }
            None => run = Some((kind, id, id)),
        }
    }
    if let Some((kind, first, last)) = run {
        parts.push(run_label(kind, first, last));
    }
    parts.join(", ")
}

fn run_label(kind: &str, first: u32, last: u32) -> String {
    if first == last {
        format!("{kind} {first}")
    } else {
        format!("{kind}s {first}–{last}")
    }
}

fn kind_and_id(element: &TopologyElement) -> (&'static str, u32) {
    match element {
        TopologyElement::Face(FaceId(id)) => ("face", *id),
        TopologyElement::Edge(EdgeId(id)) => ("edge", *id),
        TopologyElement::Vertex(VertexId(id)) => ("vertex", *id),
    }
}

/// A measurement with as few decimals as convey it.
fn number(value: f64) -> String {
    if (value - value.round()).abs() < 5e-3 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.2}")
    }
}

fn point(position: [f64; 3]) -> String {
    format!(
        "({}, {}, {})",
        number(position[0]),
        number(position[1]),
        number(position[2])
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadmark_core::geometry::{
        EdgeDescriptor, FaceDescriptor, GeometryDescriptors, ModelSummary,
    };
    use cadmark_core::ledger::{ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef};
    use cadmark_core::mesh::TessellatedMesh;
    use cadmark_kernel::protocol::ModelFile;

    fn made_at(line: u32, operation: SemanticOperation) -> LedgerValue {
        LedgerValue::Resolved(ProvenanceEntry {
            source: SourceRef {
                line,
                code: format!("line {line}"),
            },
            operation,
            operation_id: u64::from(line),
            relation: ProvenanceRelation::Generated,
        })
    }

    fn empty_model() -> ExecutedModel {
        ExecutedModel {
            mesh: TessellatedMesh::default(),
            ledger: ProvenanceLedger::new(),
            descriptors: GeometryDescriptors::default(),
            summary: ModelSummary {
                volume: 0.0,
                bounds_min: [0.0; 3],
                bounds_max: [0.0; 3],
                face_count: 0,
                edge_count: 0,
                vertex_count: 0,
            },
            validity: Vec::new(),
            model: ModelFile(std::path::PathBuf::from("/scratch/model.brep")),
        }
    }

    /// A box whose line 6 made four edges, and a fillet on line 8 that made
    /// two more: the shape that makes "one edge of a line, not the line's
    /// edges" a discriminating case.
    fn filleted_model() -> ExecutedModel {
        let mut model = empty_model();
        for id in 0..4 {
            model
                .ledger
                .record_edge(EdgeId(id), made_at(6, SemanticOperation::Box))
                .unwrap();
        }
        for id in 4..6 {
            model
                .ledger
                .record_edge(EdgeId(id), made_at(8, SemanticOperation::Fillet))
                .unwrap();
        }
        model
            .ledger
            .record_face(FaceId(0), made_at(6, SemanticOperation::Box))
            .unwrap();
        model.descriptors.edges = (0..6)
            .map(|id| EdgeDescriptor {
                curve_type: "line".to_string(),
                length: 10.0 + f64::from(id),
                centre: [f64::from(id), 0.0, 0.0],
            })
            .collect();
        model.descriptors.faces = vec![FaceDescriptor {
            surface_type: "plane".to_string(),
            area: 100.0,
            centre: [0.0, 0.0, 5.0],
            normal: [0.0, 0.0, 1.0],
        }];
        model
    }

    #[test]
    fn a_reference_resolves_to_that_element_alone() {
        let model = filleted_model();
        let scope = ReferenceScope::Model(&model);
        assert_eq!(
            resolve_references("I rounded [edge 4] only.", &scope),
            vec![TopologyElement::Edge(EdgeId(4))]
        );
    }

    #[test]
    fn prose_naming_geometry_without_the_convention_references_nothing() {
        let model = filleted_model();
        let scope = ReferenceScope::Model(&model);
        assert!(
            resolve_references(
                "I filleted the top edge of the box and the face beside it (edge 4).",
                &scope
            )
            .is_empty()
        );
    }

    #[test]
    fn a_malformed_or_unknown_bracket_references_nothing() {
        let model = filleted_model();
        let scope = ReferenceScope::Model(&model);
        for reply in [
            "[the top edge]",
            "[edge]",
            "[edge four]",
            "[edges 4]",
            "[edge 4 and edge 5]",
            "[loop 4]",
            "see [the docs](https://example.com/edge 4)",
        ] {
            assert!(
                resolve_references(reply, &scope).is_empty(),
                "{reply} should reference nothing"
            );
        }
    }

    #[test]
    fn a_reference_the_model_does_not_contain_is_dropped() {
        let model = filleted_model();
        let scope = ReferenceScope::Model(&model);
        assert!(resolve_references("I changed [edge 99].", &scope).is_empty());
        assert!(resolve_references("I changed [vertex 0].", &scope).is_empty());
    }

    #[test]
    fn several_references_resolve_in_order_without_repeats() {
        let model = filleted_model();
        let scope = ReferenceScope::Model(&model);
        assert_eq!(
            resolve_references("[edge 5], [face 0] and [edge 5] again.", &scope),
            vec![
                TopologyElement::Edge(EdgeId(5)),
                TopologyElement::Face(FaceId(0)),
            ]
        );
    }

    #[test]
    fn a_reply_naming_more_than_the_cap_is_truncated() {
        let mut model = empty_model();
        for id in 0..(MAX_REFERENCES as u32 + 10) {
            model
                .ledger
                .record_edge(EdgeId(id), made_at(6, SemanticOperation::Box))
                .unwrap();
        }
        let reply: String = (0..(MAX_REFERENCES as u32 + 10))
            .map(|id| format!("[edge {id}] "))
            .collect();
        assert_eq!(
            resolve_references(&reply, &ReferenceScope::Model(&model)).len(),
            MAX_REFERENCES
        );
    }

    #[test]
    fn without_an_execution_only_the_users_own_anchors_resolve() {
        let anchors = vec![TopologyElement::Edge(EdgeId(4))];
        let scope = ReferenceScope::Anchors(&anchors);
        assert_eq!(
            resolve_references("[edge 4] is 14 mm long; [edge 5] is not.", &scope),
            vec![TopologyElement::Edge(EdgeId(4))]
        );
    }

    #[test]
    fn the_inventory_names_every_element_with_its_source_and_measurement() {
        let text = describe_elements(&filleted_model());
        assert!(text.contains("- [face 0]: created by box at line 6, plane face, area 100 mm²"));
        assert!(text.contains("- [edge 4]: created by fillet at line 8, line edge, length 14 mm"));
        assert!(text.contains("- [edge 5]: created by fillet at line 8, line edge, length 15 mm"));
        // Each element is named singly, so the model can pick one of a line's several.
        assert_eq!(text.matches("- [edge ").count(), 6);
    }

    #[test]
    fn a_model_with_no_elements_has_no_inventory() {
        assert_eq!(describe_elements(&empty_model()), "");
    }

    #[test]
    fn a_large_model_is_summarised_by_source_as_id_ranges() {
        let mut model = empty_model();
        for id in 0..(DETAIL_LIMIT as u32 + 20) {
            let line = if id < 40 { 6 } else { 8 };
            let operation = if id < 40 {
                SemanticOperation::Box
            } else {
                SemanticOperation::Fillet
            };
            model
                .ledger
                .record_edge(EdgeId(id), made_at(line, operation))
                .unwrap();
        }
        let text = describe_elements(&model);
        assert!(text.contains("- created by box at line 6: edges 0–39"));
        assert!(text.contains("- created by fillet at line 8: edges 40–79"));
        assert!(!text.contains("- [edge 0]"));
        // The summary's length follows the operations, not the elements.
        assert!(text.lines().count() < 8, "{text}");
    }
}
