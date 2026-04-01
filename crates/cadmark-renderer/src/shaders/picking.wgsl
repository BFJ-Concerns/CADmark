// GPU colour-ID picking pass — encodes each element as a unique ID
// in an offscreen Rgba8Uint render target.

struct Uniforms {
    view_proj: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) face_id: f32,
    @location(3) _padding: f32,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) face_id: f32,
}

struct EdgeVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) edge_id: f32,
}

struct EdgeVertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) edge_id: f32,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_pos = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.face_id = in.face_id;
    return out;
}

@vertex
fn vs_edge(in: EdgeVertexInput) -> EdgeVertexOutput {
    var out: EdgeVertexOutput;
    out.clip_pos = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.edge_id = in.edge_id;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<u32> {
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
    let id = u32(in.edge_id + 0.5);
    return vec4<u32>(
        id & 0xFFu,
        (id >> 8u) & 0xFFu,
        (id >> 16u) & 0xFFu,
        (id >> 24u) & 0xFFu,
    );
}
