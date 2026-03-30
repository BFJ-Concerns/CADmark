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
    _padding: f32,
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
