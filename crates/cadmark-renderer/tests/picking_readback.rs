//! Renders the picking pass on the software adapter and reads the
//! colour-ID texture back, one pixel at a time. This is what proves the
//! marker pipelines put the IDs on screen where the sizing says they are:
//! the plain-data tests can only show what is fed to the GPU.
//!
//! The adapter is forced to the fallback (CPU) one, so the result holds on
//! a machine with no GPU and does not depend on this one's driver. A
//! missing software adapter fails the run: a pass earned by the named
//! instrument being absent would record nothing at all.
//!
//! The harness is this file's own `main` rather than libtest's, because
//! the loader has to be pointed at a software rasteriser before any driver
//! opens, and `main` is the only place that ordering is guaranteed.

use cadmark_core::geometry::{EdgeId, FaceId, PartId, PickedElement, TopologyElement, VertexId};
use cadmark_core::mesh::{MeshEdge, MeshVertex, TessellatedMesh};
use cadmark_renderer::camera::StandardView;
use cadmark_renderer::mesh::GpuMesh;
use cadmark_renderer::picking::{PickingPass, SelectionFilter};
use cadmark_renderer::pipeline::{RenderPipelines, Renderer, upload_mesh};
use cadmark_renderer::viewport::{copy_pick_pixel, decode_pick_result, render_picking};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 256;
const EDGE_ID: u32 = 3;
const SECOND_EDGE_ID: u32 = 7;
const SECOND_FACE_ID: u32 = 1;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_name: String,
}

/// Point the GL loader at a software rasteriser.
///
/// wgpu can only force a fallback adapter when the loader offers a CPU
/// one. Where a proprietary vendor's EGL library is installed it answers
/// first and reports that vendor's GPU, so no fallback exists in the list
/// to force; naming Mesa's EGL vendor file instead puts llvmpipe back in
/// it. On a machine with no GPU the loader already resolves to Mesa, where
/// this is redundant rather than wrong, and a value the operator set is
/// left alone.
///
/// # Safety
///
/// Writes the process environment, so the caller must still be
/// single-threaded and must not have opened a driver yet.
unsafe fn prefer_software_loader() {
    const VENDOR_VARIABLE: &str = "__EGL_VENDOR_LIBRARY_FILENAMES";
    const MESA_VENDOR_FILES: [&str; 2] = [
        "/usr/share/glvnd/egl_vendor.d/50_mesa.json",
        "/etc/glvnd/egl_vendor.d/50_mesa.json",
    ];

    if std::env::var_os(VENDOR_VARIABLE).is_some() {
        return;
    }
    let Some(mesa) = MESA_VENDOR_FILES
        .iter()
        .find(|path| std::path::Path::new(path).exists())
    else {
        return;
    };
    unsafe { std::env::set_var(VENDOR_VARIABLE, mesa) };
}

/// The software adapter, or a failure saying so. There is no skip branch
/// on purpose: this rung's proof is specified on the software adapter, so
/// its absence is an unavailability to report rather than a run to pass.
fn software_adapter() -> Gpu {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::None,
        force_fallback_adapter: true,
        compatible_surface: None,
    }))
    .expect(
        "no software adapter: wgpu offers no fallback (CPU) adapter on this machine. \
         Install a software rasteriser — Mesa's llvmpipe (GL) or lavapipe (Vulkan) — \
         and re-run.",
    );
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("picking_readback_test"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::default(),
        },
        None,
    ))
    .expect("the software adapter would not give a device");
    Gpu {
        device,
        queue,
        adapter_name: adapter.get_info().name,
    }
}

/// A camera looking straight at the XZ plane: screen right is +X and
/// screen up is +Z, so a segment along X is a horizontal line on screen.
fn front_facing_renderer(zoom_outs: u32) -> Renderer {
    let mut renderer = Renderer::new();
    renderer.camera.look_at_standard(StandardView::Front);
    for _ in 0..zoom_outs {
        renderer.camera.zoom(-1.0);
    }
    renderer
}

/// Where a world point lands, in pixels, and how deep it is in NDC.
fn project(view_proj: [[f32; 4]; 4], point: [f32; 3]) -> (f32, f32, f32) {
    let mut clip = [0.0f32; 4];
    for (row, slot) in clip.iter_mut().enumerate() {
        *slot = (0..3).map(|k| view_proj[k][row] * point[k]).sum::<f32>() + view_proj[3][row];
    }
    let ndc = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
    (
        (ndc[0] + 1.0) * 0.5 * WIDTH as f32,
        (1.0 - ndc[1]) * 0.5 * HEIGHT as f32,
        ndc[2],
    )
}

/// A backdrop face, an edge along X, and one topological vertex. The
/// face sits on whichever side of the geometry is further from the
/// camera, so anything the marker passes draw is genuinely in front of it.
fn scene(view_proj: [[f32; 4]; 4]) -> (TessellatedMesh, Vec<[f32; 3]>) {
    let candidate = [[0.0, 1.0, 0.0], [0.0, -1.0, 0.0]];
    let behind = if project(view_proj, candidate[0]).2 > project(view_proj, candidate[1]).2 {
        candidate[0][1]
    } else {
        candidate[1][1]
    };

    let corner = |x: f32, z: f32| MeshVertex {
        position: [x, behind, z],
        normal: [0.0, 1.0, 0.0],
    };
    let mesh = TessellatedMesh {
        vertices: vec![
            corner(-4.0, -4.0),
            corner(4.0, -4.0),
            corner(4.0, 4.0),
            corner(-4.0, 4.0),
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
        face_ids: vec![0, 0],
        edges: vec![MeshEdge {
            points: vec![[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            edge_id: EDGE_ID,
        }],
    };
    // The vertex sits above the edge, clear of it at the default camera.
    (mesh, vec![[0.0, 0.0, 0.5]])
}

/// Render the picking pass and decode the element under one pixel.
fn pick_at(
    gpu: &Gpu,
    renderer: &Renderer,
    filter: SelectionFilter,
    pixel: (u32, u32),
) -> Option<TopologyElement> {
    pick_at_in(gpu, renderer, filter, pixel, |view_proj| {
        vec![scene(view_proj)]
    })
}

/// The same readback over a caller-chosen set of parts, each uploaded as
/// its own `GpuMesh` so the pick runs through the per-part loop.
fn pick_at_in(
    gpu: &Gpu,
    renderer: &Renderer,
    filter: SelectionFilter,
    pixel: (u32, u32),
    parts: impl Fn([[f32; 4]; 4]) -> Vec<(TessellatedMesh, Vec<[f32; 3]>)>,
) -> Option<TopologyElement> {
    let mut pipelines =
        RenderPipelines::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm, WIDTH, HEIGHT);
    pipelines.resize(&gpu.device, WIDTH, HEIGHT);
    let picking = PickingPass::new(&gpu.device, WIDTH, HEIGHT);

    let uniforms = renderer.simple_uniforms((WIDTH, HEIGHT));
    gpu.queue.write_buffer(
        &pipelines.picking_uniform_buffer,
        0,
        bytemuck::bytes_of(&uniforms),
    );

    let meshes: Vec<GpuMesh> = parts(uniforms.view_proj)
        .iter()
        .enumerate()
        .map(|(index, (mesh, vertex_positions))| {
            upload_mesh(&gpu.device, mesh, PartId(index as u32), vertex_positions)
        })
        .collect();

    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    render_picking(&mut encoder, &pipelines, &picking, &meshes, &meshes, filter);
    copy_pick_pixel(
        &mut encoder,
        &picking,
        &picking.staging_buffer,
        pixel.0,
        pixel.1,
    );
    gpu.queue.submit(Some(encoder.finish()));

    let slice = picking.staging_buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    gpu.device.poll(wgpu::Maintain::Wait);
    let element = {
        let data = slice.get_mapped_range();
        decode_pick_result(&data[..4], filter)
    };
    picking.staging_buffer.unmap();
    // The scene is a solid alone, so anything decoding as a sketch
    // element would mean the ID ranges had collided.
    element.map(|element| match element {
        PickedElement::Solid(element) => element,
        PickedElement::Sketch(element) => panic!("a solid-only scene picked {element:?}"),
    })
}

/// The pixel a given number of screen pixels below a world point. The
/// edge is probed below its centre line and the marker above it, so the
/// two never contend for the same pixel however far the camera pulls back.
fn pixels_below(view_proj: [[f32; 4]; 4], point: [f32; 3], pixels: f32) -> (u32, u32) {
    let (x, y, _) = project(view_proj, point);
    (x.round() as u32, (y + pixels).round() as u32)
}

fn an_edge_is_picked_beside_the_line_itself_not_only_on_it(gpu: &Gpu) {
    let renderer = front_facing_renderer(0);
    let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;

    // Two pixels off the centre line on each side: inside the drawn half
    // width, and entirely outside a one-pixel line primitive. Both sides,
    // because a quad folded over covers only one of them.
    for offset in [2.0, -2.0] {
        let pixel = pixels_below(view_proj, [0.0, 0.0, 0.0], offset);
        assert_eq!(
            pick_at(gpu, &renderer, SelectionFilter::default(), pixel),
            Some(TopologyElement::Edge(EdgeId(EDGE_ID))),
            "not hittable {offset} pixels from the centre line"
        );
    }
}

fn an_edge_keeps_its_screen_width_as_the_model_shrinks(gpu: &Gpu) {
    // The same offset in pixels, at cameras an order of magnitude apart:
    // a width measured in world units cannot pass both.
    for zoom_outs in [0, 24] {
        let renderer = front_facing_renderer(zoom_outs);
        let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;
        // Below the line only: pulling the camera back walks the vertex
        // marker down towards the edge's upper side, and this test is
        // about the edge's own width, not which marker wins.
        let pixel = pixels_below(view_proj, [0.0, 0.0, 0.0], 2.0);
        assert_eq!(
            pick_at(gpu, &renderer, SelectionFilter::default(), pixel),
            Some(TopologyElement::Edge(EdgeId(EDGE_ID))),
            "the edge was unhittable two pixels off centre after {zoom_outs} zoom-outs (distance {})",
            renderer.camera.distance()
        );
    }
}

fn a_vertex_marker_wins_over_the_face_behind_it(gpu: &Gpu) {
    let renderer = front_facing_renderer(0);
    let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;

    // Three pixels off the vertex's own point, inside the marker's radius.
    let pixel = pixels_below(view_proj, [0.0, 0.0, 0.5], -3.0);
    assert_eq!(
        pick_at(gpu, &renderer, SelectionFilter::default(), pixel),
        Some(TopologyElement::Vertex(VertexId(0)))
    );
}

fn a_disabled_kind_lets_the_click_through_to_the_face_behind(gpu: &Gpu) {
    let renderer = front_facing_renderer(0);
    let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;
    let face = Some(TopologyElement::Face(FaceId(0)));

    let on_edge = pixels_below(view_proj, [0.0, 0.0, 0.0], 2.0);
    assert_eq!(
        pick_at(
            gpu,
            &renderer,
            SelectionFilter {
                edges: false,
                ..SelectionFilter::default()
            },
            on_edge
        ),
        face,
        "with edges off the click must reach the face, not empty space"
    );

    let on_marker = pixels_below(view_proj, [0.0, 0.0, 0.5], -3.0);
    assert_eq!(
        pick_at(
            gpu,
            &renderer,
            SelectionFilter {
                vertices: false,
                ..SelectionFilter::default()
            },
            on_marker
        ),
        face
    );
}

/// A second part, offset along screen-right, with its own face and edge
/// IDs. Its backdrop face sits behind its edge exactly as the first
/// part's does.
fn second_part(view_proj: [[f32; 4]; 4]) -> (TessellatedMesh, Vec<[f32; 3]>) {
    const OFFSET: f32 = 2.5;
    let (mut mesh, vertices) = scene(view_proj);
    for vertex in &mut mesh.vertices {
        vertex.position[0] += OFFSET;
    }
    for edge in &mut mesh.edges {
        for point in &mut edge.points {
            point[0] += OFFSET;
        }
        edge.edge_id = SECOND_EDGE_ID;
    }
    mesh.face_ids = vec![SECOND_FACE_ID; mesh.face_ids.len()];
    (
        mesh,
        vertices
            .into_iter()
            .map(|mut position| {
                position[0] += OFFSET;
                position
            })
            .collect(),
    )
}

/// The filter is one decision for the whole scene, not the active part's
/// alone: with edges off, a click on a *second* part's edge must still
/// fall through to that part's face.
fn a_disabled_kind_falls_through_on_every_part_not_just_the_first(gpu: &Gpu) {
    let renderer = front_facing_renderer(2);
    let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;
    let scene = |view_proj| vec![self::scene(view_proj), second_part(view_proj)];

    let on_second_edge = pixels_below(view_proj, [2.5, 0.0, 0.0], 2.0);
    assert_eq!(
        pick_at_in(
            gpu,
            &renderer,
            SelectionFilter::default(),
            on_second_edge,
            scene
        ),
        Some(TopologyElement::Edge(EdgeId(SECOND_EDGE_ID))),
        "the second part's edge must be hittable before the filter is asked to hide it"
    );
    assert_eq!(
        pick_at_in(
            gpu,
            &renderer,
            SelectionFilter {
                edges: false,
                ..SelectionFilter::default()
            },
            on_second_edge,
            scene
        ),
        Some(TopologyElement::Face(FaceId(SECOND_FACE_ID))),
        "with edges off the click must reach the second part's own face"
    );
}

/// One named check and the function that runs it.
type Check = (&'static str, fn(&Gpu));

/// Runs every check against one software device and reports them the way
/// libtest would, so a failure is as visible in the gate's output as any
/// other test's.
fn main() {
    // Sound only while the process is single-threaded, which is why this
    // is `main` and not a lazily-initialised helper inside a check.
    unsafe { prefer_software_loader() };

    let gpu = software_adapter();
    println!(
        "\nrunning 5 tests on software adapter: {}",
        gpu.adapter_name
    );

    let checks: [Check; 5] = [
        (
            "an_edge_is_picked_beside_the_line_itself_not_only_on_it",
            an_edge_is_picked_beside_the_line_itself_not_only_on_it,
        ),
        (
            "an_edge_keeps_its_screen_width_as_the_model_shrinks",
            an_edge_keeps_its_screen_width_as_the_model_shrinks,
        ),
        (
            "a_vertex_marker_wins_over_the_face_behind_it",
            a_vertex_marker_wins_over_the_face_behind_it,
        ),
        (
            "a_disabled_kind_lets_the_click_through_to_the_face_behind",
            a_disabled_kind_lets_the_click_through_to_the_face_behind,
        ),
        (
            "a_disabled_kind_falls_through_on_every_part_not_just_the_first",
            a_disabled_kind_falls_through_on_every_part_not_just_the_first,
        ),
    ];

    let mut failed = 0;
    for (name, check) in checks {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&gpu)));
        if outcome.is_ok() {
            println!("test {name} ... ok");
        } else {
            failed += 1;
            println!("test {name} ... FAILED");
        }
    }

    let verdict = if failed == 0 { "ok" } else { "FAILED" };
    println!(
        "\ntest result: {verdict}. {} passed; {failed} failed\n",
        checks.len() - failed
    );
    if failed > 0 {
        std::process::exit(1);
    }
}
