// Tessellated geometry as the renderer consumes it: triangles with
// per-triangle face IDs for colour picking, and edge polylines for the
// wireframe overlay. Kernel-neutral plain data, serialisable so it can
// cross the kernel worker's process boundary.

use serde::{Deserialize, Serialize};

/// A vertex in the tessellated mesh.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

/// A tessellated edge for wireframe overlay rendering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshEdge {
    /// Polyline vertices approximating the edge curve.
    pub points: Vec<[f32; 3]>,
    /// Zero-based final-topology edge identifier, the ledger's `EdgeId`.
    pub edge_id: u32,
}

/// The complete tessellated output of an executed script.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TessellatedMesh {
    /// Triangle vertices.
    pub vertices: Vec<MeshVertex>,
    /// Triangle indices — every three consecutive form a triangle.
    pub indices: Vec<u32>,
    /// Per-triangle face identifier for GPU picking, the ledger's `FaceId`.
    /// Index i corresponds to triangle i (`indices[i*3..i*3+3]`).
    pub face_ids: Vec<u32>,
    /// Edges for the wireframe overlay.
    pub edges: Vec<MeshEdge>,
}
