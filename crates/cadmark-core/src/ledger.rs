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
    BooleanFuse,
    BooleanCut,
    BooleanCommon,
    Fillet,
    Chamfer,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerValue {
    Resolved(ProvenanceEntry),
    Ambiguous(Vec<ProvenanceEntry>),
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
#[derive(Debug, Default)]
pub struct ProvenanceLedger {
    faces: HashMap<FaceId, LedgerValue>,
    edges: HashMap<EdgeId, LedgerValue>,
    vertices: HashMap<VertexId, LedgerValue>,
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
    fn lookup_missing_returns_none() {
        let ledger = ProvenanceLedger::new();
        assert!(ledger.lookup_face(FaceId(999)).is_none());
        assert!(ledger.lookup_edge(EdgeId(999)).is_none());
        assert!(ledger.lookup_vertex(VertexId(999)).is_none());
    }
}
