// The return leg of the pointing channel: how the AI names geometry in
// its reply, and how those names become the elements the viewport lights
// up.
//
// The convention is one element per square bracket, written the way the
// inventory names it — `[edge 12 @7c1e0a94b2d3f065]`, where the trailing tag
// is the execution that assigned the ID. A bracket is what separates a
// reference
// from prose, so "the top edge" in a sentence names nothing and lights
// nothing.
//
// The tag is what makes a stale ID harmless. IDs are reassigned by every
// execution, so `edge 1` of one run is unrelated to `edge 1` of the next.
// The tag is not a serial number handed out and then remembered: it is a
// fingerprint of the execution's own elements — their IDs, provenance
// and measurements — re-derived from whatever model a reference is being
// resolved against. A name therefore resolves only when the geometry in
// hand is verifiably the geometry that was named, and a reference from
// any other execution fails closed without anything about that execution
// having to be remembered, or the clock having to be trusted.
//
// Faces, edges and vertices are all referenceable, each named exactly as
// `TopologyElement::display_label` writes it. A vertex resolves and
// reaches the highlight set like any other element; the viewport draws
// no vertex markers yet, so it is the one kind whose highlight cannot
// currently be seen — a renderer gap, not a hole in the convention.
//
// Parsing fails closed at every step. A bracket whose contents are not
// exactly a kind, a number and (where the scope has one) a matching tag
// is not a reference; a reference to an element the resolving scope does
// not contain is dropped. Nothing here ever widens a reference to its
// source line's other elements: a reply about one edge highlights that
// edge alone.
//
// The other end of the convention is the inventory: the model can only
// name an element whose ID it has seen, so `describe_elements` renders
// the current model's elements into the `run_script` result. A small
// model is listed element by element, each with the measurement that
// tells it from its siblings. A large one is grouped by source line and
// compressed into ID ranges, so the result's size follows the number of
// operations rather than the number of elements, and the same
// per-element detail is fetched a run at a time by `describe_run` when
// the model asks for it. Detail is therefore deferred on a large model,
// never withdrawn: every element of every model can be told from its
// siblings and named individually, at the cost of one tool call.

use cadmark_bridge::tools::ElementKind;
use cadmark_core::geometry::{EdgeId, FaceId, TopologyElement, VertexId};
use cadmark_core::ledger::{LedgerValue, ProvenanceEntry, ProvenanceLedger};
use cadmark_kernel::protocol::ExecutedModel;

/// The most elements detailed in one tool result: the size above which
/// the run inventory groups instead of listing, and the size of one page
/// of `describe_run`. A tool result rides in the model's context window
/// every subsequent request of the turn, so an unbounded listing would
/// swamp it; a bounded one the model can ask for repeatedly costs a
/// round trip instead and has no ceiling.
const DETAIL_LIMIT: usize = 60;

/// The most references one reply can highlight. A reply naming more than
/// this has stopped being a reference to specific geometry, and the
/// renderer's highlight set is bounded anyway.
pub const MAX_REFERENCES: usize = 32;

/// The identity of one execution, quoted alongside every ID that
/// execution assigned: a fingerprint of the elements the execution
/// produced, so the tag is a property of the geometry rather than a
/// serial number that has to be remembered to mean anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionTag(String);

impl ExecutionTag {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ExecutionTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The tag of `model`: the fingerprint of every element it contains,
/// each element's provenance and measurements, and the model's own
/// summary.
///
/// Two executions share a tag only when they agree on all of that, and
/// then a name means the same element in both. Any difference — a
/// different script, a different run of the same script that moved an
/// element, a model built by an earlier run of the application — yields
/// a different tag, so a reference carrying the old one resolves to
/// nothing. Nothing here consults the clock or any state outside the
/// model, so a restart cannot reissue a tag it did not earn.
pub fn execution_tag(model: &ExecutedModel) -> ExecutionTag {
    let mut fingerprint = Fingerprint::new();
    for element in elements_in_order(&model.ledger) {
        let (kind, id) = kind_and_id(&element);
        fingerprint.text(kind);
        fingerprint.integer(u64::from(id));
        fingerprint.provenance(provenance(&model.ledger, &element));
        fingerprint.measurement(model, &element);
    }
    let summary = &model.summary;
    fingerprint.decimal(summary.volume);
    for value in summary.bounds_min.iter().chain(summary.bounds_max.iter()) {
        fingerprint.decimal(*value);
    }
    fingerprint.integer(summary.face_count as u64);
    fingerprint.integer(summary.edge_count as u64);
    fingerprint.integer(summary.vertex_count as u64);
    ExecutionTag(format!("{:016x}", fingerprint.finish()))
}

/// FNV-1a over an execution's content. Written out here rather than
/// taken from the standard library's hasher so the digest is fixed by
/// this file alone: a tag means the same thing in every build, which is
/// what lets one be re-derived instead of remembered.
struct Fingerprint(u64);

impl Fingerprint {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    /// Terminated, so two neighbouring fields cannot run together into
    /// the byte stream a different pair of fields would produce.
    fn text(&mut self, text: &str) {
        self.bytes(text.as_bytes());
        self.bytes(&[0]);
    }

    fn integer(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn decimal(&mut self, value: f64) {
        self.integer(value.to_bits());
    }

    fn point(&mut self, point: [f64; 3]) {
        for value in point {
            self.decimal(value);
        }
    }

    fn provenance(&mut self, value: Option<&LedgerValue>) {
        match value {
            None => self.text("unrecorded"),
            Some(LedgerValue::Untraced) => self.text("untraced"),
            Some(LedgerValue::Resolved(entry)) => {
                self.text("resolved");
                self.entry(entry);
            }
            Some(LedgerValue::Ambiguous(candidates)) => {
                self.text("ambiguous");
                self.integer(candidates.len() as u64);
                for entry in candidates {
                    self.entry(entry);
                }
            }
        }
    }

    fn entry(&mut self, entry: &ProvenanceEntry) {
        self.integer(u64::from(entry.source.line));
        self.text(&entry.source.code);
        self.text(entry.operation.display_name());
        self.integer(entry.operation_id);
        self.text(entry.relation.display_phrase());
    }

    /// The measured geometry of one element, so two models that share
    /// every ID and source line but not the geometry itself still tag
    /// differently.
    fn measurement(&mut self, model: &ExecutedModel, element: &TopologyElement) {
        match element {
            TopologyElement::Face(id) => match model.descriptors.face(*id) {
                Some(face) => {
                    self.text(&face.surface_type);
                    self.decimal(face.area);
                    self.point(face.centre);
                    self.point(face.normal);
                }
                None => self.text("unmeasured"),
            },
            TopologyElement::Edge(id) => match model.descriptors.edge(*id) {
                Some(edge) => {
                    self.text(&edge.curve_type);
                    self.decimal(edge.length);
                    self.point(edge.centre);
                }
                None => self.text("unmeasured"),
            },
            TopologyElement::Vertex(id) => match model.descriptors.vertex(*id) {
                Some(vertex) => self.point(vertex.position),
                None => self.text("unmeasured"),
            },
        }
    }

    fn finish(self) -> u64 {
        self.0
    }
}

/// What a reference is allowed to resolve against. Provenance is rebuilt
/// by every execution and never persisted, so a reference resolves only
/// against the ledger of the execution whose tag it quotes, or against
/// the elements the user pointed at in this turn's own comments. An ID
/// quoted from any other execution highlights nothing.
pub enum ReferenceScope<'a> {
    /// A script executed in this turn: an element of its ledger, named
    /// with that execution's own tag, which is recomputed from this
    /// model rather than taken on trust.
    Model { model: &'a ExecutedModel },
    /// Nothing executed this turn: only the elements the user's comments
    /// anchored, which came from the model on screen and carry no tag.
    Anchors(&'a [TopologyElement]),
}

/// One well-formed bracket: the element named, and the execution tag it
/// was named with, if any.
#[derive(Debug, PartialEq)]
struct Reference {
    element: TopologyElement,
    tag: Option<String>,
}

impl ReferenceScope<'_> {
    /// The tag a reference must quote to be resolved here, derived from
    /// the geometry in hand. The user's own anchors carry none, which is
    /// how they were named to the model.
    fn tag(&self) -> Option<ExecutionTag> {
        match self {
            Self::Model { model } => Some(execution_tag(model)),
            Self::Anchors(_) => None,
        }
    }

    fn contains(&self, reference: &Reference, tag: Option<&ExecutionTag>) -> bool {
        if reference.tag.as_deref() != tag.map(ExecutionTag::as_str) {
            return false;
        }
        match self {
            Self::Model { model } => in_ledger(&model.ledger, &reference.element),
            Self::Anchors(anchors) => anchors.contains(&reference.element),
        }
    }
}

fn in_ledger(ledger: &ProvenanceLedger, element: &TopologyElement) -> bool {
    provenance(ledger, element).is_some()
}

/// The elements `reply` references and the scope can account for, in the
/// order they appear, without repeats. Prose that merely mentions
/// geometry yields nothing, and so does a reference the scope does not
/// contain.
pub fn resolve_references(reply: &str, scope: &ReferenceScope<'_>) -> Vec<TopologyElement> {
    // Computed once, from the model being resolved against: the tag a
    // reference quotes is checked against the execution in hand, never
    // against a tag carried along from when the name was written.
    let tag = scope.tag();
    let mut resolved: Vec<TopologyElement> = Vec::new();
    for reference in parse_references(reply) {
        if !scope.contains(&reference, tag.as_ref()) || resolved.contains(&reference.element) {
            continue;
        }
        resolved.push(reference.element);
        if resolved.len() == MAX_REFERENCES {
            break;
        }
    }
    resolved
}

/// Every well-formed reference in `reply`, in order, before any check
/// that the elements exist.
fn parse_references(reply: &str) -> Vec<Reference> {
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

/// One bracket's contents as a reference, or None when it is anything
/// but a kind, a number and an optional `@tag`.
fn parse_reference(inner: &str) -> Option<Reference> {
    let parts: Vec<&str> = inner.split_whitespace().collect();
    let (kind, id, tag) = match parts.as_slice() {
        [kind, id] => (*kind, *id, None),
        [kind, id, tag] => (*kind, *id, Some(tag.strip_prefix('@')?.to_string())),
        _ => return None,
    };
    if tag.as_ref().is_some_and(|tag| tag.is_empty()) {
        return None;
    }
    let id: u32 = id.parse().ok()?;
    let element = match kind.to_ascii_lowercase().as_str() {
        "face" => TopologyElement::Face(FaceId(id)),
        "edge" => TopologyElement::Edge(EdgeId(id)),
        "vertex" => TopologyElement::Vertex(VertexId(id)),
        _ => return None,
    };
    Some(Reference { element, tag })
}

/// The elements of `model` as the AI reads them in a tool result: what it
/// may reference, and enough about each to tell them apart.
pub fn describe_elements(model: &ExecutedModel, tag: &ExecutionTag) -> String {
    let ledger = &model.ledger;
    let elements = elements_in_order(ledger);
    let total = elements.len();
    if total == 0 {
        return String::new();
    }

    let mut text = format!(
        "\nElements of this model you can reference in your reply, named with \
         this run's tag @{tag}"
    );
    if total <= DETAIL_LIMIT {
        text.push_str(", with the measurement that tells them apart:\n");
        for element in elements {
            text.push_str(&detail_line(model, &element, tag));
        }
    } else {
        text.push_str(" (too many to list singly, so by source):\n");
        for (source, elements) in grouped_by_source(ledger) {
            text.push_str(&format!("- {source}: {}\n", ranges_of(&elements)));
        }
        text.push_str(&format!(
            "Write any of them as [edge 12 @{tag}]. Call inspect_elements for any run of \
             these IDs to get each element's source line and measurements, as listed \
             above for a smaller model — that is how you tell two elements of one \
             operation apart before you name one.\n"
        ));
    }
    text
}

/// The elements of `model` between `first` and `last` of one kind, in the
/// detail the grouped inventory leaves out: what the model reads when it
/// asks about a run of IDs it has been given.
///
/// A run longer than one page is answered a page at a time, ending with
/// the ID to ask from next, so the answer's size is bounded however large
/// the model or the run. IDs the model does not contain are simply
/// absent, and a run with none of them says so rather than inventing one.
pub fn describe_run(
    model: &ExecutedModel,
    tag: &ExecutionTag,
    kind: ElementKind,
    first: u32,
    last: Option<u32>,
) -> String {
    let wanted = kind_label(kind);
    let last = last.unwrap_or(first).max(first);
    let run: Vec<TopologyElement> = elements_in_order(&model.ledger)
        .into_iter()
        .filter(|element| {
            let (kind, id) = kind_and_id(element);
            kind == wanted && (first..=last).contains(&id)
        })
        .collect();
    if run.is_empty() {
        return format!(
            "This model has no {wanted} between {first} and {last}. Only the IDs the last \
             run listed exist; nothing else can be named.\n"
        );
    }

    let page = &run[..run.len().min(DETAIL_LIMIT)];
    let mut text = format!(
        "The {} of this model, named with this run's tag @{tag}, with the measurement \
         that tells them apart:\n",
        // The heading names the page that follows, not the range asked
        // for, which may be wider than the IDs that exist.
        run_label(
            wanted,
            kind_and_id(&page[0]).1,
            kind_and_id(&page[page.len() - 1]).1
        )
    );
    for element in page {
        text.push_str(&detail_line(model, element, tag));
    }
    if let Some(next) = run.get(DETAIL_LIMIT) {
        text.push_str(&format!(
            "{} more in this run; ask again from {} for the rest.\n",
            run.len() - DETAIL_LIMIT,
            kind_and_id(next).1
        ));
    }
    text
}

/// One element as the model reads it: the name to quote back, the line
/// that made it, and the measurement that tells it from its siblings.
fn detail_line(model: &ExecutedModel, element: &TopologyElement, tag: &ExecutionTag) -> String {
    format!(
        "- [{} @{tag}]: {}{}\n",
        element.display_label(),
        source_of(&model.ledger, element),
        measurement_of(model, element)
    )
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

fn kind_label(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Face => "face",
        ElementKind::Edge => "edge",
        ElementKind::Vertex => "vertex",
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
        EdgeDescriptor, FaceDescriptor, GeometryDescriptors, ModelSummary, VertexDescriptor,
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

    /// Eighty edges: forty from a box on line 6 and forty from a fillet on
    /// line 8. More than the inventory lists singly, so the shape where
    /// asking for a run's detail is the only way to tell one of an
    /// operation's edges from another.
    fn large_model() -> ExecutedModel {
        let mut model = empty_model();
        let total = DETAIL_LIMIT as u32 + 20;
        for id in 0..total {
            let (line, operation) = if id < 40 {
                (6, SemanticOperation::Box)
            } else {
                (8, SemanticOperation::Fillet)
            };
            model
                .ledger
                .record_edge(EdgeId(id), made_at(line, operation))
                .unwrap();
        }
        model.descriptors.edges = (0..total)
            .map(|id| EdgeDescriptor {
                curve_type: "line".to_string(),
                length: 10.0 + f64::from(id),
                centre: [f64::from(id), 0.0, 0.0],
            })
            .collect();
        model
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
        let tag = execution_tag(&model);
        let scope = ReferenceScope::Model { model: &model };
        assert_eq!(
            resolve_references(&format!("I rounded [edge 4 @{tag}] only."), &scope),
            vec![TopologyElement::Edge(EdgeId(4))]
        );
    }

    #[test]
    fn a_reference_tagged_with_another_execution_resolves_to_nothing() {
        // Two executions that both have an edge 4, differing only in the
        // length that execution measured it at.
        let earlier = filleted_model();
        let mut current = filleted_model();
        current.descriptors.edges[4].length = 7.5;
        let earlier_tag = execution_tag(&earlier);
        let scope = ReferenceScope::Model { model: &current };

        assert!(
            resolve_references(&format!("I rounded [edge 4 @{earlier_tag}]."), &scope).is_empty()
        );
        // And an untagged name, which no inventory ever offered.
        assert!(resolve_references("I rounded [edge 4].", &scope).is_empty());
        // The same reference under the execution that wrote it still resolves.
        assert_eq!(
            resolve_references(
                &format!("I rounded [edge 4 @{earlier_tag}]."),
                &ReferenceScope::Model { model: &earlier }
            ),
            vec![TopologyElement::Edge(EdgeId(4))]
        );
    }

    /// The tag is derived from the execution, not issued to it, so it can
    /// be recomputed from the model in hand instead of remembered — and
    /// two executions that are not the same model never share one.
    #[test]
    fn an_executions_tag_is_the_fingerprint_of_its_own_geometry() {
        let model = filleted_model();
        assert_eq!(execution_tag(&model), execution_tag(&filleted_model()));
        // Pinned, because the point of the tag is that it has no ambient
        // input: no clock, no counter, no process state. The same model
        // tags the same in every run of the application, which is what
        // lets a reference be verified against the geometry in hand
        // rather than against something remembered. A change here means
        // an ambient input has crept in.
        assert_eq!(execution_tag(&model).to_string(), "1ca03ae3c7366fe0");

        let mut moved = filleted_model();
        moved.descriptors.edges[0].centre = [1.0, 2.0, 3.0];
        assert_ne!(execution_tag(&model), execution_tag(&moved));

        let mut relined = filleted_model();
        relined.ledger.clear();
        for id in 0..6 {
            relined
                .ledger
                .record_edge(EdgeId(id), made_at(7, SemanticOperation::Box))
                .unwrap();
        }
        assert_ne!(execution_tag(&model), execution_tag(&relined));

        let mut extra = filleted_model();
        extra
            .ledger
            .record_vertex(VertexId(0), made_at(6, SemanticOperation::Box))
            .unwrap();
        assert_ne!(execution_tag(&model), execution_tag(&extra));
    }

    /// A vertex is named and resolved exactly as a face or an edge is.
    /// Nothing draws vertex markers yet, so the highlight it produces
    /// cannot be seen — that is the renderer's gap, and the convention
    /// keeps the kind rather than pretending the user cannot mean one.
    #[test]
    fn a_vertex_is_offered_and_resolves_like_any_other_element() {
        let mut model = filleted_model();
        model
            .ledger
            .record_vertex(VertexId(0), made_at(6, SemanticOperation::Box))
            .unwrap();
        model.descriptors.vertices = vec![VertexDescriptor {
            position: [1.0, 2.0, 3.0],
        }];
        let tag = execution_tag(&model);
        let scope = ReferenceScope::Model { model: &model };

        assert_eq!(
            resolve_references(&format!("The corner is [vertex 0 @{tag}]."), &scope),
            vec![TopologyElement::Vertex(VertexId(0))]
        );
        assert!(describe_elements(&model, &tag).contains(&format!(
            "- [vertex 0 @{tag}]: created by box at line 6, at (1, 2, 3)"
        )));
        // Still an exact ledger lookup, not a range.
        assert!(resolve_references(&format!("[vertex 1 @{tag}]"), &scope).is_empty());
    }

    #[test]
    fn prose_naming_geometry_without_the_convention_references_nothing() {
        let model = filleted_model();
        let tag = execution_tag(&model);
        let scope = ReferenceScope::Model { model: &model };
        assert!(
            resolve_references(
                &format!(
                    "I filleted the top edge of the box and the face beside it (edge 4 @{tag})."
                ),
                &scope
            )
            .is_empty()
        );
    }

    #[test]
    fn a_malformed_or_unknown_bracket_references_nothing() {
        let model = filleted_model();
        let tag = execution_tag(&model);
        let scope = ReferenceScope::Model { model: &model };
        for reply in [
            "[the top edge]".to_string(),
            "[edge]".to_string(),
            format!("[edge @{tag}]"),
            format!("[edge four @{tag}]"),
            format!("[edges 4 @{tag}]"),
            format!("[edge 4 and edge 5 @{tag}]"),
            format!("[loop 4 @{tag}]"),
            format!("[edge 4 {tag}]"),
            "[edge 4 @]".to_string(),
            format!("see [the docs](https://example.com/edge 4 @{tag})"),
        ] {
            assert!(
                resolve_references(&reply, &scope).is_empty(),
                "{reply} should reference nothing"
            );
        }
    }

    #[test]
    fn a_reference_the_model_does_not_contain_is_dropped() {
        let model = filleted_model();
        let tag = execution_tag(&model);
        let scope = ReferenceScope::Model { model: &model };
        assert!(resolve_references(&format!("I changed [edge 99 @{tag}]."), &scope).is_empty());
        assert!(resolve_references(&format!("I changed [face 9 @{tag}]."), &scope).is_empty());
    }

    #[test]
    fn several_references_resolve_in_order_without_repeats() {
        let model = filleted_model();
        let tag = execution_tag(&model);
        let scope = ReferenceScope::Model { model: &model };
        assert_eq!(
            resolve_references(
                &format!("[edge 5 @{tag}], [face 0 @{tag}] and [edge 5 @{tag}] again."),
                &scope
            ),
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
        let tag = execution_tag(&model);
        let reply: String = (0..(MAX_REFERENCES as u32 + 10))
            .map(|id| format!("[edge {id} @{tag}] "))
            .collect();
        assert_eq!(
            resolve_references(&reply, &ReferenceScope::Model { model: &model }).len(),
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
        let model = filleted_model();
        let tag = execution_tag(&model);
        let text = describe_elements(&model, &tag);
        assert!(text.contains(&format!(
            "- [face 0 @{tag}]: created by box at line 6, plane face, area 100 mm²"
        )));
        assert!(text.contains(&format!(
            "- [edge 4 @{tag}]: created by fillet at line 8, line edge, length 14 mm"
        )));
        assert!(text.contains(&format!(
            "- [edge 5 @{tag}]: created by fillet at line 8, line edge, length 15 mm"
        )));
        // Each element is named singly, so the model can pick one of a line's several.
        assert_eq!(text.matches("- [edge ").count(), 6);
    }

    #[test]
    fn a_model_with_no_elements_has_no_inventory() {
        let model = empty_model();
        assert_eq!(describe_elements(&model, &execution_tag(&model)), "");
    }

    #[test]
    fn a_large_model_is_summarised_by_source_as_id_ranges() {
        let model = large_model();
        let tag = execution_tag(&model);
        let text = describe_elements(&model, &tag);
        assert!(text.contains(&format!("[edge 12 @{tag}]")));
        assert!(text.contains("- created by box at line 6: edges 0–39"));
        assert!(text.contains("- created by fillet at line 8: edges 40–79"));
        assert!(!text.contains("- [edge 0]"));
        // The summary's length follows the operations, not the elements.
        assert!(text.lines().count() < 8, "{text}");
        // Detail is deferred, not withdrawn: the summary says where to
        // get the per-element measurements it leaves out.
        assert!(text.contains("inspect_elements"), "{text}");
    }

    #[test]
    fn a_run_of_a_large_models_ids_is_detailed_element_by_element_when_asked_for() {
        let model = large_model();
        let tag = execution_tag(&model);
        let text = describe_run(&model, &tag, ElementKind::Edge, 40, Some(43));
        // Every element of the run is named singly, with the measurement
        // that separates it from the rest of its operation.
        assert!(text.contains(&format!(
            "- [edge 40 @{tag}]: created by fillet at line 8, line edge, length 50 mm"
        )));
        assert!(text.contains(&format!("- [edge 43 @{tag}]")));
        assert_eq!(text.matches("- [edge ").count(), 4, "{text}");
        // And nothing outside the run.
        assert!(!text.contains("[edge 39 "), "{text}");
        assert!(!text.contains("[edge 44 "), "{text}");
    }

    #[test]
    fn one_element_is_a_run_with_no_end() {
        let model = large_model();
        let tag = execution_tag(&model);
        let text = describe_run(&model, &tag, ElementKind::Edge, 71, None);
        assert!(text.contains(&format!("- [edge 71 @{tag}]")), "{text}");
        assert_eq!(text.matches("- [edge ").count(), 1, "{text}");
    }

    #[test]
    fn a_run_longer_than_one_page_is_answered_a_page_at_a_time() {
        let model = large_model();
        let tag = execution_tag(&model);
        let text = describe_run(&model, &tag, ElementKind::Edge, 0, Some(1000));
        // The answer is bounded however wide the run asked for, and says
        // where the next page starts, so no model is too large to detail.
        assert_eq!(text.matches("- [edge ").count(), DETAIL_LIMIT, "{text}");
        assert!(
            text.contains(&format!(
                "{} more in this run; ask again from 60",
                80 - DETAIL_LIMIT
            )),
            "{text}"
        );
        let rest = describe_run(&model, &tag, ElementKind::Edge, 60, Some(1000));
        assert_eq!(
            rest.matches("- [edge ").count(),
            80 - DETAIL_LIMIT,
            "{rest}"
        );
        assert!(!rest.contains("ask again"), "{rest}");
    }

    #[test]
    fn a_run_of_ids_the_model_does_not_have_details_nothing() {
        let model = large_model();
        let tag = execution_tag(&model);
        // Past the end of the model, and a kind it has none of.
        for text in [
            describe_run(&model, &tag, ElementKind::Edge, 200, Some(300)),
            describe_run(&model, &tag, ElementKind::Vertex, 0, Some(300)),
        ] {
            assert!(!text.contains("- ["), "{text}");
            assert!(
                text.contains("Only the IDs the last run listed exist"),
                "{text}"
            );
        }
    }
}
