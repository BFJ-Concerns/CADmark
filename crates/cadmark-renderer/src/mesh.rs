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

/// One corner of an edge segment's screen-space quad. Six of these — two
/// triangles — carry each polyline segment, and the shader expands them
/// sideways from the segment's own direction, so the drawn width is in
/// pixels rather than model units.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct EdgeVertex {
    /// The endpoint this corner sits at.
    pub position: [f32; 3],
    /// Edge ID for picking.
    pub edge_id: f32,
    /// The segment's other endpoint, which gives the screen-space direction.
    pub other: [f32; 3],
    /// -1 or +1: which side of the segment this corner expands to.
    pub side: f32,
    /// -1 or +1: which way along the segment the end cap extends.
    pub cap: f32,
    /// +1 at the segment's first endpoint, -1 at its second. The tangent
    /// this corner sees points the opposite way at each end, so the side
    /// the normal picks would flip without it and fold the quad over.
    pub end_sign: f32,
}

/// One corner of a vertex marker's screen-space quad. Six of these carry
/// each marker; the shader masks the quad down to a disc.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct MarkerVertex {
    /// The model vertex the marker is centred on.
    pub position: [f32; 3],
    /// Vertex ID for picking.
    pub vertex_id: f32,
    /// Which corner of the quad this is: -1 or +1 on each axis.
    pub corner: [f32; 2],
    /// Alignment padding to 32 bytes.
    pub _padding: [f32; 2],
}

/// All GPU buffers for a single model.
pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub edge_vertex_buffer: wgpu::Buffer,
    pub edge_vertex_count: u32,
    pub marker_vertex_buffer: wgpu::Buffer,
    pub marker_vertex_count: u32,
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
    /// The picking ID of the sketch element this vertex draws, so the
    /// visible pass can tint the selected or hovered element.
    pub id: f32,
}

/// Tint selecting an enclosed region's translucent fill.
pub const REGION_TINT: f32 = 0.0;
/// Tint selecting a corner marker's solid dot.
pub const CORNER_TINT: f32 = 1.0;

/// GPU buffers for one sketch profile. Curves are a line list; regions
/// and corner markers share one triangle list, told apart by their tint.
/// The pick buffers carry the same elements for the colour-ID pass in
/// the layouts the face, edge and marker picking pipelines read: regions
/// as triangles, curves as screen-space quads, corners as marker quads.
pub struct GpuSketch {
    pub fill_vertex_buffer: wgpu::Buffer,
    pub fill_vertex_count: u32,
    pub curve_vertex_buffer: wgpu::Buffer,
    pub curve_vertex_count: u32,
    pub pick_region_vertex_buffer: wgpu::Buffer,
    pub pick_region_vertex_count: u32,
    pub pick_curve_vertex_buffer: wgpu::Buffer,
    pub pick_curve_vertex_count: u32,
    pub pick_corner_vertex_buffer: wgpu::Buffer,
    pub pick_corner_vertex_count: u32,
}
