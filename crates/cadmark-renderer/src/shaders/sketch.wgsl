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

// Picking IDs of the candidate-footprint highlight, shared with the mesh
// pass so a sketch element lights up when its line is hovered.
@group(0) @binding(1) var<storage, read> highlight_ids: array<u32>;

struct Marker {
    element_id: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    colour: vec4<f32>,
}

// Pending-comment markers: a sketch element anchored to a card takes the
// card's colour, as a solid element does.
@group(0) @binding(2) var<storage, read> markers: array<Marker>;

fn in_highlight(id: u32) -> bool {
    for (var i = 0u; i < uniforms.highlight_count; i = i + 1u) {
        if highlight_ids[i] == id {
            return true;
        }
    }
    return false;
}

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) tint: f32,
    @location(2) id: f32,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) tint: f32,
    @location(1) id: f32,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_pos = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.tint = in.tint;
    out.id = in.id;
    return out;
}

// Drafting blue, already display-encoded: the profile reads as drawing
// rather than as material.
const CURVE_COLOUR: vec3<f32> = vec3<f32>(0.11, 0.36, 0.85);
const REGION_COLOUR: vec3<f32> = vec3<f32>(0.34, 0.58, 0.95);
const CORNER_COLOUR: vec3<f32> = vec3<f32>(0.05, 0.18, 0.55);

// The selected element takes the selection colour, the hovered one the
// hover colour, so a sketch answers a click the way a solid does. The
// colours are the same ones the mesh pass uses, so the two agree on
// what "selected" looks like.
fn tint_selection(colour: vec4<f32>, id: f32) -> vec4<f32> {
    let element = u32(id + 0.5);
    if element == 0u {
        return colour;
    }
    for (var index = 0u; index < min(arrayLength(&markers), uniforms.marker_count); index++) {
        if markers[index].element_id == element {
            return vec4<f32>(
                mix(colour.rgb, markers[index].colour.rgb, markers[index].colour.a),
                max(colour.a, markers[index].colour.a),
            );
        }
    }
    if element == uniforms.selected_id {
        return vec4<f32>(
            mix(colour.rgb, uniforms.selected_colour.rgb, uniforms.selected_colour.a),
            max(colour.a, uniforms.selected_colour.a),
        );
    }
    if element == uniforms.hover_id || in_highlight(element) {
        return vec4<f32>(
            mix(colour.rgb, uniforms.hover_colour.rgb, uniforms.hover_colour.a),
            max(colour.a, uniforms.hover_colour.a),
        );
    }
    return colour;
}

@fragment
fn fs_curve(in: VertexOutput) -> @location(0) vec4<f32> {
    return tint_selection(vec4<f32>(CURVE_COLOUR, 1.0), in.id);
}

@fragment
fn fs_fill(in: VertexOutput) -> @location(0) vec4<f32> {
    if in.tint > 0.5 {
        return tint_selection(vec4<f32>(CORNER_COLOUR, 1.0), in.id);
    }
    // Regions wash their area rather than covering it, so a ghosted solid
    // still reads behind the profile.
    return tint_selection(vec4<f32>(REGION_COLOUR, 0.28), in.id);
}
