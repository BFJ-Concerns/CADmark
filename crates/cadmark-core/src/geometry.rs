// Geometry context types — the structured format sent to the AI
// when the user clicks an element in the viewport.

use serde::{Deserialize, Serialize};

use crate::ledger::ProvenanceEntry;

/// A topological element the user can select in the viewport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TopologyElement {
    Face(FaceId),
    Edge(EdgeId),
    Vertex(VertexId),
}

/// Unique identifier for a face in the rendered mesh.
/// Assigned during tessellation and used for GPU picking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FaceId(pub u32);

/// Unique identifier for an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeId(pub u32);

/// Unique identifier for a vertex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VertexId(pub u32);

/// Screen-space position for overlay placement.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScreenPosition {
    pub x: f32,
    pub y: f32,
}

/// The current selection state in the viewport.
#[derive(Debug, Clone, Default)]
pub enum SelectionState {
    #[default]
    None,
    /// Element under the cursor — preview highlight, not yet clicked.
    Hovering(TopologyElement),
    /// Element the user has clicked — glow effect active.
    Selected(TopologyElement),
}

/// Geometry context packaged for the AI.
/// Stable output format regardless of which identification strategy produced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometryContext {
    /// The selected element type and ID.
    pub element: TopologyElement,
    /// Required, resolved construction-time provenance.
    pub provenance: ProvenanceEntry,
    /// Experimental identification data — strategies can attach arbitrary
    /// key-value pairs here without changing the outer format.
    #[serde(default)]
    pub identification: std::collections::HashMap<String, String>,
}
