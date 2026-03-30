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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProvenanceKind {
    /// Directly created by the operation.
    Generated,
    /// Existed before and was modified by the operation.
    Modified,
}

/// A single provenance record linking an element to its generating code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceEntry {
    pub source: SourceRef,
    pub kind: ProvenanceKind,
}

/// The provenance ledger — rebuilt each time the script executes.
///
/// Maps each topological element (face, edge, vertex) to the source line
/// that generated or last modified it. When an element has been through
/// multiple operations (e.g. a face created by a Box then modified by a
/// Fillet), the most recent operation is stored.
#[derive(Debug, Default)]
pub struct ProvenanceLedger {
    faces: HashMap<FaceId, ProvenanceEntry>,
    edges: HashMap<EdgeId, ProvenanceEntry>,
    vertices: HashMap<VertexId, ProvenanceEntry>,
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

    pub fn record_face(&mut self, id: FaceId, entry: ProvenanceEntry) {
        self.faces.insert(id, entry);
    }

    pub fn lookup_face(&self, id: FaceId) -> Option<&ProvenanceEntry> {
        self.faces.get(&id)
    }

    // -- Edge provenance --

    pub fn record_edge(&mut self, id: EdgeId, entry: ProvenanceEntry) {
        self.edges.insert(id, entry);
    }

    pub fn lookup_edge(&self, id: EdgeId) -> Option<&ProvenanceEntry> {
        self.edges.get(&id)
    }

    // -- Vertex provenance --

    pub fn record_vertex(&mut self, id: VertexId, entry: ProvenanceEntry) {
        self.vertices.insert(id, entry);
    }

    pub fn lookup_vertex(&self, id: VertexId) -> Option<&ProvenanceEntry> {
        self.vertices.get(&id)
    }

    /// Total number of entries across all element types.
    pub fn len(&self) -> usize {
        self.faces.len() + self.edges.len() + self.vertices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
