// GPU mesh buffers — vertex, index, and face-ID data uploaded to the GPU.

use bytemuck::{Pod, Zeroable};

/// Vertex layout for the main shaded render pass.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Face ID encoded as float for the picking pass.
    pub face_id: f32,
    /// Alignment padding after the face ID.
    pub _padding: f32,
    /// Picking ID of the completed part containing this face.
    pub part_id: f32,
    pub _part_padding: [f32; 3],
}

/// Vertex layout for wireframe edge rendering.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct EdgeVertex {
    pub position: [f32; 3],
    /// Edge ID for picking.
    pub edge_id: f32,
}

/// All GPU buffers for a single model.
pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub edge_vertex_buffer: wgpu::Buffer,
    pub edge_vertex_count: u32,
}

/// Vertex layout for the sketch profile passes — regions and corner
/// markers as triangles, curves as lines, all through the same layout.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SketchVertex {
    pub position: [f32; 3],
    /// Which part of the profile this vertex belongs to: `REGION_TINT` for
    /// an enclosed region's fill, `CORNER_TINT` for a corner marker.
    pub tint: f32,
}

/// Tint selecting an enclosed region's translucent fill.
pub const REGION_TINT: f32 = 0.0;
/// Tint selecting a corner marker's solid dot.
pub const CORNER_TINT: f32 = 1.0;

/// GPU buffers for one sketch profile. Curves are a line list; regions
/// and corner markers share one triangle list, told apart by their tint.
pub struct GpuSketch {
    pub fill_vertex_buffer: wgpu::Buffer,
    pub fill_vertex_count: u32,
    pub curve_vertex_buffer: wgpu::Buffer,
    pub curve_vertex_count: u32,
}
