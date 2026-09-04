// Main shaded mesh pass — a studio light rig that travels with the camera,
// so the model reads the same from every angle, plus selection and hover
// highlights.

struct Uniforms {
    view_proj: mat4x4<f32>,
    eye_pos: vec3<f32>,
    // Non-zero when the colour target is not an sRGB format, so the shader
    // must gamma-encode its linear result itself.
    encode_srgb: u32,
    // Key light direction (towards the light), world space, unit length.
    key_light_dir: vec3<f32>,
    _pad0: f32,
    // Fill light direction (towards the light), world space, unit length.
    fill_light_dir: vec3<f32>,
    _pad1: f32,
    selected_id: u32,
    hover_id: u32,
    highlight_count: u32,
    _pad3: u32,
    selected_colour: vec4<f32>,
    hover_colour: vec4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// Picking IDs of the candidate-footprint highlight. A storage binding
// because a footprint is as large as the geometry one line accounts for,
// which a uniform array could not hold.
@group(0) @binding(1) var<storage, read> highlight_ids: array<u32>;

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
    @location(1) normal: vec3<f32>,
    @location(2) face_id: f32,
    @location(3) _padding: f32,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) face_id: f32,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_pos = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.world_pos = in.position;
    out.world_normal = in.normal;
    out.face_id = in.face_id;
    return out;
}

// Machined light-grey material, linear colour.
const BASE_COLOUR: vec3<f32> = vec3<f32>(0.62, 0.64, 0.67);
// Hemisphere ambient: cool sky above, warm dim ground below.
const SKY_COLOUR: vec3<f32> = vec3<f32>(0.30, 0.33, 0.38);
const GROUND_COLOUR: vec3<f32> = vec3<f32>(0.12, 0.11, 0.10);
const KEY_COLOUR: vec3<f32> = vec3<f32>(1.00, 0.97, 0.92);
const FILL_COLOUR: vec3<f32> = vec3<f32>(0.45, 0.50, 0.60);

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_main(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    var normal = normalize(in.world_normal);
    // Both sides are lit: looking into an open shell or a section shows a
    // shaded interior, not a black one.
    if !front_facing {
        normal = -normal;
    }
    let view_dir = normalize(uniforms.eye_pos - in.world_pos);

    // Ambient from the sky/ground hemisphere, keyed on world up (Z).
    let hemisphere = normal.z * 0.5 + 0.5;
    var colour = BASE_COLOUR * mix(GROUND_COLOUR, SKY_COLOUR, hemisphere);

    // Key light: diffuse plus a tight Blinn-Phong highlight.
    let key_ndl = max(dot(normal, uniforms.key_light_dir), 0.0);
    let key_half = normalize(uniforms.key_light_dir + view_dir);
    let key_spec = pow(max(dot(normal, key_half), 0.0), 48.0) * 0.35;
    colour += BASE_COLOUR * KEY_COLOUR * key_ndl * 0.85 + KEY_COLOUR * key_spec;

    // Fill light: soft, cool, from the opposite side.
    let fill_ndl = max(dot(normal, uniforms.fill_light_dir), 0.0);
    colour += BASE_COLOUR * FILL_COLOUR * fill_ndl * 0.45;

    // Rim: lifts silhouettes so the outline stays legible against the
    // background whichever way the model is turned.
    let facing = max(dot(normal, view_dir), 0.0);
    let rim = pow(1.0 - facing, 3.0) * 0.18;
    colour += SKY_COLOUR * rim;

    // Selection and hover tints.
    let fid = u32(in.face_id + 0.5);
    if fid == uniforms.selected_id && uniforms.selected_id != 0u {
        colour = mix(colour, uniforms.selected_colour.rgb, uniforms.selected_colour.a);
    } else if fid == uniforms.hover_id && uniforms.hover_id != 0u {
        colour = mix(colour, uniforms.hover_colour.rgb, uniforms.hover_colour.a);
    } else if in_highlight(fid) {
        colour = mix(colour, uniforms.hover_colour.rgb, uniforms.hover_colour.a * 0.7);
    }

    colour = clamp(colour, vec3<f32>(0.0), vec3<f32>(1.0));
    if uniforms.encode_srgb != 0u {
        colour = linear_to_srgb(colour);
    }
    return vec4<f32>(colour, 1.0);
}
