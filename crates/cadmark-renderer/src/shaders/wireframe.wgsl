// Wireframe edge overlay — draws edges as lines over the shaded mesh, with
// the selected or hovered edge picked out in its highlight colour.

struct Uniforms {
    view_proj: mat4x4<f32>,
    eye_pos: vec3<f32>,
    encode_srgb: u32,
    key_light_dir: vec3<f32>,
    _pad0: f32,
    fill_light_dir: vec3<f32>,
    _pad1: f32,
    selected_id: u32,
    hover_id: u32,
    marker_count: u32,
    _pad2: u32,
    selected_colour: vec4<f32>,
    hover_colour: vec4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct Marker {
    element_id: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    colour: vec4<f32>,
}

@group(0) @binding(1) var<storage, read> markers: array<Marker>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) edge_id: f32,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) edge_id: f32,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_pos = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.edge_id = in.edge_id;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let eid = u32(in.edge_id + 0.5);
    for (var index = 0u; index < min(arrayLength(&markers), uniforms.marker_count); index++) {
        if eid == markers[index].element_id && eid != 0u {
            return vec4<f32>(markers[index].colour.rgb, 1.0);
        }
    }
    if eid == uniforms.selected_id && uniforms.selected_id != 0u {
        return vec4<f32>(uniforms.selected_colour.rgb, 1.0);
    }
    if eid == uniforms.hover_id && uniforms.hover_id != 0u {
        return vec4<f32>(uniforms.hover_colour.rgb, 1.0);
    }
    // Dark edges, already display-encoded.
    return vec4<f32>(0.10, 0.10, 0.12, 1.0);
}
