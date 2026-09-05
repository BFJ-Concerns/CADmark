// Wireframe edge overlay — draws each edge as a screen-space quad over
// the shaded mesh, so it holds a constant width at any camera distance,
// with the selected or hovered edge picked out in its highlight colour.
//
// The marker expansion comes from the shared snippet prepended at
// pipeline creation. The picking pass expands the same buffer through the
// same function with a wider hit target.

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
    // Section plane [nx, ny, nz, d]: a fragment is discarded when
    // dot(n, world_pos) + d < 0. All zeroes means no section.
    section_plane: vec4<f32>,
    // Opacity of the shaded surface.
    mesh_alpha: f32,
    _pad6: f32,
    _pad7: f32,
    _pad8: f32,
    marker_size: MarkerExtent,
}

// Whether the section plane keeps `world_pos`. Mirrors
// `SectionPlane::equation` in the renderer's `section` module.
fn section_keeps(section_plane: vec4<f32>, world_pos: vec3<f32>) -> bool {
    return dot(section_plane.xyz, world_pos) + section_plane.w >= 0.0;
}

// What a ghosted solid's edges fade towards — the viewport background,
// display-encoded like the rest of this shader's output.
const GHOST_COLOUR: vec3<f32> = vec3<f32>(0.157, 0.165, 0.188);

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// Picking IDs of the candidate-footprint highlight. A storage binding
// because a footprint is as large as the geometry one line accounts for,
// which a uniform array could not hold.
@group(0) @binding(1) var<storage, read> highlight_ids: array<u32>;

struct Marker {
    element_id: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    colour: vec4<f32>,
}

@group(0) @binding(2) var<storage, read> markers: array<Marker>;

// Whether this element belongs to the highlighted candidate's footprint.
fn in_highlight(id: u32) -> bool {
    if id == 0u {
        return false;
    }
    for (var i = 0u; i < uniforms.highlight_count; i = i + 1u) {
        if highlight_ids[i] == id {
            return true;
        }
    }
    return false;
}


struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) edge_id: f32,
    // The segment's other endpoint, for the screen-space direction.
    @location(2) other: vec3<f32>,
    @location(3) side: f32,
    @location(4) cap: f32,
    @location(5) end_sign: f32,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) edge_id: f32,
    @location(1) world_pos: vec3<f32>,
    // Screen-linear coordinates in units of the expanded half-width.
    @location(2) @interpolate(linear) stroke: vec2<f32>,
    @location(3) @interpolate(flat) segment_length: f32,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let here = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    let other = uniforms.view_proj * vec4<f32>(in.other, 1.0);
    out.clip_pos = expand_edge(here, other, in.side, in.cap, in.end_sign, uniforms.marker_size);
    let k = max(uniforms.marker_size.edge_half_width_ndc, vec2<f32>(1e-9));
    let a = here.xy / max(abs(here.w), 1e-6);
    let b = other.xy / max(abs(other.w), 1e-6);
    out.segment_length = length((b - a) / k);
    out.stroke = vec2<f32>(in.side, select(out.segment_length - in.cap, in.cap, in.end_sign > 0.0));
    out.edge_id = in.edge_id;
    out.world_pos = in.position;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // A capsule with a one-pixel coverage fringe, evaluated before any
    // discard so derivatives remain defined across neighbouring fragments.
    let beyond_end = max(max(-in.stroke.y, in.stroke.y - in.segment_length), 0.0);
    let distance = length(vec2<f32>(in.stroke.x, beyond_end));
    let pixel_width = max(length(vec2<f32>(dpdx(distance), dpdy(distance))), 1e-6);
    let coverage = clamp((1.0 - distance) / pixel_width, 0.0, 1.0);
    if coverage == 0.0 || !section_keeps(uniforms.section_plane, in.world_pos) {
        discard;
    }
    let ghost = clamp(uniforms.ghost, 0.0, 1.0);
    let eid = u32(in.edge_id + 0.5);
    for (var index = 0u; index < min(arrayLength(&markers), uniforms.marker_count); index++) {
        if eid == markers[index].element_id && eid != 0u {
            return vec4<f32>(mix(markers[index].colour.rgb, GHOST_COLOUR, ghost), coverage);
        }
    }
    if eid == uniforms.selected_id && uniforms.selected_id != 0u {
        return vec4<f32>(mix(uniforms.selected_colour.rgb, GHOST_COLOUR, ghost), coverage);
    }
    if eid == uniforms.hover_id && uniforms.hover_id != 0u {
        return vec4<f32>(mix(uniforms.hover_colour.rgb, GHOST_COLOUR, ghost), coverage);
    }
    if in_highlight(eid) {
        return vec4<f32>(uniforms.hover_colour.rgb, coverage);
    }
    // Dark edges, already display-encoded.
    return vec4<f32>(mix(vec3<f32>(0.10, 0.10, 0.12), GHOST_COLOUR, ghost), coverage);
}
