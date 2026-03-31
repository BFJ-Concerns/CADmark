// Fullscreen blit — copies the offscreen viewport texture onto the
// egui render target. Uses a single oversized triangle (3 vertices,
// no vertex buffer) to avoid quad seam artefacts.

@group(0) @binding(0) var t_viewport: texture_2d<f32>;
@group(0) @binding(1) var s_viewport: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Oversized triangle covering the full clip space.
    var pos = array<vec2f, 3>(
        vec2f(-1.0, -1.0),
        vec2f( 3.0, -1.0),
        vec2f(-1.0,  3.0),
    );
    var uv = array<vec2f, 3>(
        vec2f(0.0, 1.0),
        vec2f(2.0, 1.0),
        vec2f(0.0, -1.0),
    );

    var out: VertexOutput;
    out.position = vec4f(pos[idx], 0.0, 1.0);
    out.uv = uv[idx];
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    return textureSample(t_viewport, s_viewport, in.uv);
}
