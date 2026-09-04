// Sketch profile overlay — the curves, corners and enclosed regions of a
// design that has reached only a sketch, drawn flat on the sketch's own
// plane and always in front of whatever solid is behind it.

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
    highlight_count: u32,
    _pad3: u32,
    marker_count: u32,
    ghost: f32,
    selected_part_id: u32,
    hover_part_id: u32,
    selected_colour: vec4<f32>,
    hover_colour: vec4<f32>,
    section_plane: vec4<f32>,
    mesh_alpha: f32,
    _pad6: f32,
    _pad7: f32,
    _pad8: f32,
    marker_size: MarkerExtent,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) tint: f32,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) tint: f32,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_pos = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.tint = in.tint;
    return out;
}

// Drafting blue, already display-encoded: the profile reads as drawing
// rather than as material.
const CURVE_COLOUR: vec3<f32> = vec3<f32>(0.11, 0.36, 0.85);
const REGION_COLOUR: vec3<f32> = vec3<f32>(0.34, 0.58, 0.95);
const CORNER_COLOUR: vec3<f32> = vec3<f32>(0.05, 0.18, 0.55);

@fragment
fn fs_curve(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(CURVE_COLOUR, 1.0);
}

@fragment
fn fs_fill(in: VertexOutput) -> @location(0) vec4<f32> {
    if in.tint > 0.5 {
        return vec4<f32>(CORNER_COLOUR, 1.0);
    }
    // Regions wash their area rather than covering it, so a ghosted solid
    // still reads behind the profile.
    return vec4<f32>(REGION_COLOUR, 0.28);
}
