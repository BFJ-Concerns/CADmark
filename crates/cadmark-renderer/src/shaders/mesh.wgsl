// Main shaded mesh pass — Blinn-Phong lighting with selection glow.

struct Uniforms {
    view_proj: mat4x4<f32>,
    eye_pos: vec3<f32>,
    _pad0: f32,
    selected_id: u32,
    hover_id: u32,
    _pad1: u32,
    _pad2: u32,
    selected_colour: vec4<f32>,
    hover_colour: vec4<f32>,
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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Blinn-Phong shading.
    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let normal = normalize(in.world_normal);
    let view_dir = normalize(uniforms.eye_pos - in.world_pos);
    let half_dir = normalize(light_dir + view_dir);

    let ambient = 0.15;
    let diffuse = max(dot(normal, light_dir), 0.0) * 0.7;
    let specular = pow(max(dot(normal, half_dir), 0.0), 32.0) * 0.3;

    // Base colour — neutral grey for CAD models.
    let base_colour = vec3<f32>(0.7, 0.72, 0.75);
    var colour = base_colour * (ambient + diffuse) + vec3<f32>(specular);

    // Selection glow — additive blend on the selected face.
    let fid = u32(in.face_id + 0.5);
    if fid == uniforms.selected_id && uniforms.selected_id != 0u {
        colour = mix(colour, uniforms.selected_colour.rgb, uniforms.selected_colour.a);
    } else if fid == uniforms.hover_id && uniforms.hover_id != 0u {
        colour = mix(colour, uniforms.hover_colour.rgb, uniforms.hover_colour.a);
    }

    return vec4<f32>(colour, 1.0);
}
