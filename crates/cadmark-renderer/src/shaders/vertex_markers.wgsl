// Visible vertex markers — a screen-space disc at each of the model's
// vertices, so a vertex is something the user can see and aim at.
//
// The marker expansion comes from the shared snippet prepended at
// pipeline creation; the picking pass expands and masks the same buffer
// through the same functions. A vertex carrying a reference marker or
// sitting in a candidate's footprint takes that colour, which is what
// makes a highlighted vertex visible at all.

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
    _pad4: u32,
    _pad5: u32,
    selected_colour: vec4<f32>,
    hover_colour: vec4<f32>,
    // Section plane [nx, ny, nz, d]: a fragment is discarded when
    // dot(n, world_pos) + d < 0. All zeroes means no section.
    section_plane: vec4<f32>,
    mesh_alpha: f32,
    _pad6: f32,
    _pad7: f32,
    _pad8: f32,
    marker_size: MarkerExtent,
    // The surface colour of each part by ordinal.
    part_colours: array<vec4<f32>, PART_PALETTE_LEN>,
}

// Whether the section plane keeps `world_pos`. Mirrors
// `SectionPlane::equation` in the renderer's `section` module.
fn section_keeps(section_plane: vec4<f32>, world_pos: vec3<f32>) -> bool {
    return dot(section_plane.xyz, world_pos) + section_plane.w >= 0.0;
}

// What a ghosted solid's markers fade towards — the viewport background,
// in linear colour like the shared highlight uniforms.
const GHOST_COLOUR: vec3<f32> = vec3<f32>(0.021, 0.023, 0.030);

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// Picking IDs of the candidate-footprint highlight.
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
    @location(1) vertex_id: f32,
    // Which corner of the marker's quad this is: -1 or +1 on each axis.
    @location(2) corner: vec2<f32>,
    @location(3) part_id: f32,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) vertex_id: f32,
    @location(1) @interpolate(linear) corner: vec2<f32>,
    // The marker's own vertex, not the expanded corner: a marker is
    // clipped away with the vertex it stands for, as a whole.
    @location(2) world_pos: vec3<f32>,
    @location(3) @interpolate(flat) part_id: f32,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let centre = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.clip_pos = expand_marker(centre, in.corner, uniforms.marker_size);
    out.vertex_id = in.vertex_id;
    out.corner = in.corner;
    out.world_pos = in.position;
    out.part_id = in.part_id;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let distance = length(in.corner);
    let pixel_width = max(length(vec2<f32>(dpdx(distance), dpdy(distance))), 1e-6);
    let coverage = clamp((1.0 - distance) / pixel_width, 0.0, 1.0);
    if coverage == 0.0 || !section_keeps(uniforms.section_plane, in.world_pos) {
        discard;
    }
    let ghost = clamp(uniforms.ghost, 0.0, 1.0);
    let vid = pick_in_part(u32(in.vertex_id + 0.5), u32(in.part_id + 0.5));
    for (var index = 0u; index < min(arrayLength(&markers), uniforms.marker_count); index++) {
        if vid == markers[index].element_id && vid != 0u {
            return marker_colour(mix(markers[index].colour.rgb, GHOST_COLOUR, ghost), coverage, uniforms.encode_srgb);
        }
    }
    if vid == uniforms.selected_id && uniforms.selected_id != 0u {
        return marker_colour(mix(uniforms.selected_colour.rgb, GHOST_COLOUR, ghost), coverage, uniforms.encode_srgb);
    }
    if vid == uniforms.hover_id && uniforms.hover_id != 0u {
        return marker_colour(mix(uniforms.hover_colour.rgb, GHOST_COLOUR, ghost), coverage, uniforms.encode_srgb);
    }
    if in_highlight(vid) {
        return marker_colour(uniforms.hover_colour.rgb, coverage, uniforms.encode_srgb);
    }
    // Dark discs in linear colour, matching the wireframe.
    return marker_colour(mix(vec3<f32>(0.010023, 0.010023, 0.013412), GHOST_COLOUR, ghost), coverage, uniforms.encode_srgb);
}
