// Provenance ledger — maps rendered geometry to generating code.
//
// Transient at runtime; rebuilt by re-executing the build123d script.
// Each entry links a topological element to the source line that created it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::geometry::{EdgeId, FaceId, VertexId};

/// A reference to a line in the build123d script.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceRef {
    /// 1-indexed line number in the script.
    pub line: u32,
    /// The source code text at that line.
    pub code: String,
}

/// Records how a topological element relates to its generating operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProvenanceRelation {
    /// Directly created by the operation.
    Generated,
    /// Existed before and was modified by the operation.
    Modified,
    /// Contained by topology directly reported as generated.
    GeneratedDescendant,
    /// Contained by topology directly reported as modified.
    ModifiedDescendant,
}

/// The user-authored operation responsible for topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SemanticOperation {
    Box,
    Cylinder,
    Sphere,
    Cone,
    Torus,
    Wedge,
    Extrude,
    Revolve,
    Loft,
    Sweep,
    Thicken,
    Shell,
    Draft,
    Split,
    BooleanFuse,
    BooleanCut,
    BooleanCommon,
    Fillet,
    Chamfer,
    LocationPattern,
    Mirror,
    Rotate,
    Scale,
    /// A 2D offset of a sketch profile's outline.
    Offset,
    /// A sketch face built from drawn edges: `make_face` or `make_hull`.
    MakeFace,
}

impl SemanticOperation {
    /// Plain-language name for people reading the chat pane or overlay.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Box => "box",
            Self::Cylinder => "cylinder",
            Self::Sphere => "sphere",
            Self::Cone => "cone",
            Self::Torus => "torus",
            Self::Wedge => "wedge",
            Self::Extrude => "extrude",
            Self::Revolve => "revolve",
            Self::Loft => "loft",
            Self::Sweep => "sweep",
            Self::Thicken => "thicken",
            Self::Shell => "shell",
            Self::Draft => "draft",
            Self::Split => "split",
            Self::BooleanFuse => "union",
            Self::BooleanCut => "cut",
            Self::BooleanCommon => "intersection",
            Self::Fillet => "fillet",
            Self::Chamfer => "chamfer",
            Self::LocationPattern => "location pattern",
            Self::Mirror => "mirror",
            Self::Rotate => "rotate",
            Self::Scale => "scale",
            Self::Offset => "offset",
            Self::MakeFace => "face from edges",
        }
    }
}

impl LedgerValue {
    /// The single source line when exactly one operation claims the element.
    pub fn resolved(&self) -> Option<&ProvenanceEntry> {
        match self {
            Self::Resolved(entry) => Some(entry),
            Self::Ambiguous(_) | Self::Untraced => None,
        }
    }

    /// Every candidate source, in ledger order: one for a resolved element,
    /// several for an ambiguous one, none for an untraced one.
    pub fn candidates(&self) -> &[ProvenanceEntry] {
        match self {
            Self::Resolved(entry) => std::slice::from_ref(entry),
            Self::Ambiguous(candidates) => candidates,
            Self::Untraced => &[],
        }
    }

    /// Plain-language description of the element's source for the UI.
    pub fn describe(&self) -> String {
        match self {
            Self::Resolved(entry) => entry.describe(),
            Self::Ambiguous(candidates) => format!(
                "source is ambiguous: {}",
                candidates
                    .iter()
                    .map(ProvenanceEntry::describe)
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
            Self::Untraced => {
                "no source line (made by an operation CADmark cannot trace yet)".to_string()
            }
        }
    }
}

impl ProvenanceEntry {
    /// Plain-language description such as "created by box at line 4".
    pub fn describe(&self) -> String {
        format!(
            "{} {} at line {}",
            self.relation.display_phrase(),
            self.operation.display_name(),
            self.source.line
        )
    }
}

impl ProvenanceRelation {
    /// Plain-language phrase describing how the operation touched the element.
    pub fn display_phrase(&self) -> &'static str {
        match self {
            Self::Generated => "created by",
            Self::Modified => "modified by",
            Self::GeneratedDescendant => "part of geometry created by",
            Self::ModifiedDescendant => "part of geometry modified by",
        }
    }
}

/// A single provenance record linking an element to its generating code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceEntry {
    pub source: SourceRef,
    pub operation: SemanticOperation,
    pub operation_id: u64,
    pub relation: ProvenanceRelation,
}

/// The provenance state for one final topology element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LedgerValue {
    Resolved(ProvenanceEntry),
    Ambiguous(Vec<ProvenanceEntry>),
    /// The element exists in the final model but no instrumented operation
    /// claimed it, so no source line can be offered for it.
    Untraced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("provenance already recorded for {kind} {index}")]
pub struct DuplicateTopologyId {
    pub kind: &'static str,
    pub index: u32,
}

/// The provenance ledger — rebuilt each time the script executes.
///
/// Maps each topological element (face, edge, vertex) to the source line
/// that generated or last modified it. When an element has been through
/// multiple operations (e.g. a face created by a Box then modified by a
/// Fillet), the most recent operation is stored.
///
/// Serialised as entry lists rather than maps: JSON object keys are
/// strings, and the typed IDs do not round-trip as keys.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "LedgerEntries", into = "LedgerEntries")]
pub struct ProvenanceLedger {
    faces: HashMap<FaceId, LedgerValue>,
    edges: HashMap<EdgeId, LedgerValue>,
    vertices: HashMap<VertexId, LedgerValue>,
}

/// The ledger's wire shape.
#[derive(Serialize, Deserialize)]
struct LedgerEntries {
    faces: Vec<(FaceId, LedgerValue)>,
    edges: Vec<(EdgeId, LedgerValue)>,
    vertices: Vec<(VertexId, LedgerValue)>,
}

impl From<ProvenanceLedger> for LedgerEntries {
    fn from(ledger: ProvenanceLedger) -> Self {
        // Sorted so the same ledger always serialises the same way.
        let mut faces: Vec<_> = ledger.faces.into_iter().collect();
        faces.sort_by_key(|(id, _)| id.0);
        let mut edges: Vec<_> = ledger.edges.into_iter().collect();
        edges.sort_by_key(|(id, _)| id.0);
        let mut vertices: Vec<_> = ledger.vertices.into_iter().collect();
        vertices.sort_by_key(|(id, _)| id.0);
        Self {
            faces,
            edges,
            vertices,
        }
    }
}

impl From<LedgerEntries> for ProvenanceLedger {
    fn from(entries: LedgerEntries) -> Self {
        Self {
            faces: entries.faces.into_iter().collect(),
            edges: entries.edges.into_iter().collect(),
            vertices: entries.vertices.into_iter().collect(),
        }
    }
}

impl ProvenanceLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear the ledger for a fresh script execution.
    pub fn clear(&mut self) {
        self.faces.clear();
        self.edges.clear();
        self.vertices.clear();
    }

    // -- Face provenance --

    pub fn record_face(
        &mut self,
        id: FaceId,
        value: LedgerValue,
    ) -> Result<(), DuplicateTopologyId> {
        record(&mut self.faces, id, value, "face", id.0)
    }

    pub fn lookup_face(&self, id: FaceId) -> Option<&LedgerValue> {
        self.faces.get(&id)
    }

    // -- Edge provenance --

    pub fn record_edge(
        &mut self,
        id: EdgeId,
        value: LedgerValue,
    ) -> Result<(), DuplicateTopologyId> {
        record(&mut self.edges, id, value, "edge", id.0)
    }

    pub fn lookup_edge(&self, id: EdgeId) -> Option<&LedgerValue> {
        self.edges.get(&id)
    }

    // -- Vertex provenance --

    pub fn record_vertex(
        &mut self,
        id: VertexId,
        value: LedgerValue,
    ) -> Result<(), DuplicateTopologyId> {
        record(&mut self.vertices, id, value, "vertex", id.0)
    }

    pub fn lookup_vertex(&self, id: VertexId) -> Option<&LedgerValue> {
        self.vertices.get(&id)
    }

    pub fn face_ids(&self) -> impl Iterator<Item = FaceId> + '_ {
        self.faces.keys().copied()
    }

    pub fn edge_ids(&self) -> impl Iterator<Item = EdgeId> + '_ {
        self.edges.keys().copied()
    }

    pub fn vertex_ids(&self) -> impl Iterator<Item = VertexId> + '_ {
        self.vertices.keys().copied()
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// Total number of entries across all element types.
    pub fn len(&self) -> usize {
        self.faces.len() + self.edges.len() + self.vertices.len()
    }

    /// Number of elements across all types that no operation claimed.
    pub fn untraced_count(&self) -> usize {
        self.faces
            .values()
            .chain(self.edges.values())
            .chain(self.vertices.values())
            .filter(|value| matches!(value, LedgerValue::Untraced))
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn record<K: std::hash::Hash + Eq>(
    entries: &mut HashMap<K, LedgerValue>,
    id: K,
    value: LedgerValue,
    kind: &'static str,
    index: u32,
) -> Result<(), DuplicateTopologyId> {
    use std::collections::hash_map::Entry;

    match entries.entry(id) {
        Entry::Vacant(slot) => {
            slot.insert(value);
            Ok(())
        }
        Entry::Occupied(_) => Err(DuplicateTopologyId { kind, index }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(line: u32, code: &str) -> SourceRef {
        SourceRef {
            line,
            code: code.to_string(),
        }
    }

    #[test]
    fn record_and_lookup_face() {
        let mut ledger = ProvenanceLedger::new();
        let id = FaceId(0);
        let entry = ProvenanceEntry {
            source: source(5, "box = Box(10, 10, 10)"),
            operation: SemanticOperation::Box,
            operation_id: 1,
            relation: ProvenanceRelation::Generated,
        };
        ledger
            .record_face(id, LedgerValue::Resolved(entry.clone()))
            .unwrap();

        let LedgerValue::Resolved(found) = ledger.lookup_face(id).unwrap() else {
            panic!("expected resolved provenance");
        };
        assert_eq!(found.source.line, 5);
        assert_eq!(found.relation, ProvenanceRelation::Generated);
    }

    #[test]
    fn duplicate_topology_id_is_rejected() {
        let mut ledger = ProvenanceLedger::new();
        let id = FaceId(3);

        // Initially generated by Box.
        ledger
            .record_face(
                id,
                LedgerValue::Resolved(ProvenanceEntry {
                    source: source(2, "box = Box(10, 10, 10)"),
                    operation: SemanticOperation::Box,
                    operation_id: 1,
                    relation: ProvenanceRelation::Generated,
                }),
            )
            .unwrap();

        let error = ledger.record_face(
            id,
            LedgerValue::Resolved(ProvenanceEntry {
                source: source(3, "fillet = Fillet(box, 1.0)"),
                operation: SemanticOperation::Fillet,
                operation_id: 2,
                relation: ProvenanceRelation::Modified,
            }),
        );
        assert_eq!(
            error,
            Err(DuplicateTopologyId {
                kind: "face",
                index: 3
            })
        );
    }

    #[test]
    fn clear_empties_all() {
        let mut ledger = ProvenanceLedger::new();
        ledger
            .record_face(
                FaceId(0),
                LedgerValue::Resolved(ProvenanceEntry {
                    source: source(1, "x"),
                    operation: SemanticOperation::Box,
                    operation_id: 1,
                    relation: ProvenanceRelation::Generated,
                }),
            )
            .unwrap();
        ledger
            .record_edge(
                EdgeId(0),
                LedgerValue::Resolved(ProvenanceEntry {
                    source: source(1, "x"),
                    operation: SemanticOperation::Box,
                    operation_id: 1,
                    relation: ProvenanceRelation::Generated,
                }),
            )
            .unwrap();
        assert_eq!(ledger.len(), 2);

        ledger.clear();
        assert!(ledger.is_empty());
    }

    #[test]
    fn a_ledger_round_trips_through_json() {
        let mut ledger = ProvenanceLedger::new();
        ledger
            .record_face(
                FaceId(3),
                LedgerValue::Resolved(ProvenanceEntry {
                    source: source(2, "Box(1, 1, 1)"),
                    operation: SemanticOperation::Box,
                    operation_id: 1,
                    relation: ProvenanceRelation::Generated,
                }),
            )
            .unwrap();
        ledger
            .record_edge(EdgeId(0), LedgerValue::Untraced)
            .unwrap();
        let json = serde_json::to_string(&ledger).unwrap();
        assert_eq!(
            serde_json::from_str::<ProvenanceLedger>(&json).unwrap(),
            ledger
        );
    }

    #[test]
    fn lookup_missing_returns_none() {
        let ledger = ProvenanceLedger::new();
        assert!(ledger.lookup_face(FaceId(999)).is_none());
        assert!(ledger.lookup_edge(EdgeId(999)).is_none());
        assert!(ledger.lookup_vertex(VertexId(999)).is_none());
    }
}
