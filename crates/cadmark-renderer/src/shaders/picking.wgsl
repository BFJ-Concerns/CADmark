// GPU colour-ID picking pass — encodes each element as a unique ID
// in an offscreen Rgba8Uint render target.
//
// The marker sizing comes from the shared snippet prepended at pipeline
// creation, and the edge and vertex-marker geometry is expanded through
// the same functions the visible passes use: an element picks at exactly
// the width it is drawn.

struct Uniforms {
    view_proj: mat4x4<f32>,
    section_plane: vec4<f32>,
    marker_size: MarkerExtent,
}

fn section_keeps(section_plane: vec4<f32>, world_pos: vec3<f32>) -> bool {
    return dot(section_plane.xyz, world_pos) + section_plane.w >= 0.0;
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) face_id: f32,
    @location(3) _padding: f32,
    @location(4) part_id: f32,
    @location(5) _part_padding: vec3<f32>,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) face_id: f32,
    @location(1) world_pos: vec3<f32>,
    @location(2) part_id: f32,
}

struct EdgeVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) edge_id: f32,
    @location(2) other: vec3<f32>,
    @location(3) side: f32,
    @location(4) cap: f32,
    @location(5) end_sign: f32,
}

struct EdgeVertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) edge_id: f32,
    @location(1) world_pos: vec3<f32>,
}

struct MarkerVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) vertex_id: f32,
    @location(2) corner: vec2<f32>,
}

struct MarkerVertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) vertex_id: f32,
    @location(1) corner: vec2<f32>,
    // The marker's own vertex, not the expanded corner: a marker is
    // clipped away with the vertex it stands for, as a whole.
    @location(2) world_pos: vec3<f32>,
}

fn encode_id(id: u32) -> vec4<u32> {
    return vec4<u32>(
        id & 0xFFu,
        (id >> 8u) & 0xFFu,
        (id >> 16u) & 0xFFu,
        (id >> 24u) & 0xFFu,
    );
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_pos = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.face_id = in.face_id;
    out.world_pos = in.position;
    out.part_id = in.part_id;
    return out;
}

@fragment
fn fs_part(in: VertexOutput) -> @location(0) vec4<u32> {
    if !section_keeps(uniforms.section_plane, in.world_pos) {
        discard;
    }
    let id = u32(in.part_id + 0.5);
    return vec4<u32>(id & 0xFFu, (id >> 8u) & 0xFFu, (id >> 16u) & 0xFFu, (id >> 24u) & 0xFFu);
}

@vertex
fn vs_edge(in: EdgeVertexInput) -> EdgeVertexOutput {
    var out: EdgeVertexOutput;
    let here = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    let other = uniforms.view_proj * vec4<f32>(in.other, 1.0);
    out.clip_pos = expand_edge(here, other, in.side, in.cap, in.end_sign, uniforms.marker_size);
    out.edge_id = in.edge_id;
    out.world_pos = in.position;
    return out;
}

@vertex
fn vs_marker(in: MarkerVertexInput) -> MarkerVertexOutput {
    var out: MarkerVertexOutput;
    let centre = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.clip_pos = expand_marker(centre, in.corner, uniforms.marker_size);
    out.vertex_id = in.vertex_id;
    out.corner = in.corner;
    out.world_pos = in.position;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<u32> {
    if !section_keeps(uniforms.section_plane, in.world_pos) {
        discard;
    }
    // Encode the face ID as a 32-bit value split across RGBA channels.
    let id = u32(in.face_id + 0.5);
    return vec4<u32>(
        id & 0xFFu,
        (id >> 8u) & 0xFFu,
        (id >> 16u) & 0xFFu,
        (id >> 24u) & 0xFFu,
    );
}

@fragment
fn fs_edge(in: EdgeVertexOutput) -> @location(0) vec4<u32> {
    if !section_keeps(uniforms.section_plane, in.world_pos) {
        discard;
    }
    return encode_id(u32(in.edge_id + 0.5));
}

// A vertex marker's disc. The same mask the visible pass uses, so the
// disc the user aims at is the disc that answers.
@fragment
fn fs_marker(in: MarkerVertexOutput) -> @location(0) vec4<u32> {
    if !section_keeps(uniforms.section_plane, in.world_pos) {
        discard;
    }
    if !marker_covers(in.corner) {
        discard;
    }
    return encode_id(u32(in.vertex_id + 0.5));
}
