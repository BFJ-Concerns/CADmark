// GPU colour-ID picking pass — encodes each element as a unique ID
// in an offscreen Rgba8Uint render target.

struct Uniforms {
    view_proj: mat4x4<f32>,
    section_plane: vec4<f32>,
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
}

struct EdgeVertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) edge_id: f32,
    @location(1) world_pos: vec3<f32>,
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
    out.clip_pos = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.edge_id = in.edge_id;
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
    let id = u32(in.edge_id + 0.5);
    return vec4<u32>(
        id & 0xFFu,
        (id >> 8u) & 0xFFu,
        (id >> 16u) & 0xFFu,
        (id >> 24u) & 0xFFu,
    );
}
