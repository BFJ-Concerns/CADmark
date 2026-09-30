//! Renders the picking pass on the software adapter and reads the
//! colour-ID texture back, one pixel at a time. This is what proves the
//! marker pipelines put the IDs on screen where the sizing says they are:
//! the plain-data tests can only show what is fed to the GPU.
//!
//! The device is the software adapter or nothing (`test_device::software`),
//! so the result holds on a machine with no GPU and does not depend on
//! this one's driver. A missing software adapter fails the run: a pass
//! earned by the named instrument being absent would record nothing at all.
//!
//! The harness is this file's own `main` rather than libtest's, so the
//! device is acquired once, before any check runs, and the checks are
//! listed and selected the way both test runners expect.

use cadmark_core::geometry::{
    EdgeId, FaceId, PartId, PickedElement, SketchElement, SketchElementKind, TopologyElement,
    VertexId,
};
use cadmark_core::mesh::{MeshEdge, MeshVertex, TessellatedMesh};
use cadmark_core::sketch::{SketchCorner, SketchCurve, SketchProfile, SketchRegion};
use cadmark_renderer::camera::StandardView;
use cadmark_renderer::mesh::GpuMesh;
use cadmark_renderer::picking::{PickingPass, SelectionFilter};
use cadmark_renderer::pipeline::{RenderPipelines, Renderer, upload_mesh, upload_sketch};
use cadmark_renderer::section::{Axis, SectionPlane};
use cadmark_renderer::test_device::TestGpu as Gpu;
use cadmark_renderer::viewport::{copy_pick_pixel, decode_pick_result, render_picking};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 256;
const EDGE_ID: u32 = 3;
const SECOND_EDGE_ID: u32 = 7;
const SECOND_FACE_ID: u32 = 1;

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
    render_picking(&mut encoder, &pipelines, &picking, &meshes, None, filter);
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
    element.map(|pick| match pick.element() {
        PickedElement::Solid(element) => element,
        PickedElement::Sketch(element) => panic!("a solid-only scene picked {element:?}"),
    })
}

/// A rectangle drawn on the XZ plane, face-on to the front camera: four
/// curves, four corners, one region. The solid behind it is the scene's
/// backdrop face, ghosted in the viewport and excluded from picking.
fn rectangle_sketch() -> SketchProfile {
    let corners = [
        [-2.0f32, 0.0, -1.0],
        [2.0, 0.0, -1.0],
        [2.0, 0.0, 1.0],
        [-2.0, 0.0, 1.0],
    ];
    SketchProfile {
        plane: cadmark_core::sketch::SketchPlane {
            origin: [0.0; 3],
            normal: [0.0, -1.0, 0.0],
            x_axis: [1.0, 0.0, 0.0],
        },
        curves: (0..4)
            .map(|index| SketchCurve {
                curve_id: index as u32,
                points: vec![corners[index], corners[(index + 1) % 4]],
                ..SketchCurve::default()
            })
            .collect(),
        corners: corners
            .iter()
            .enumerate()
            .map(|(index, &position)| SketchCorner {
                corner_id: index as u32,
                position,
            })
            .collect(),
        regions: vec![SketchRegion {
            region_id: 0,
            vertices: corners.to_vec(),
            indices: vec![0, 1, 2, 0, 2, 3],
            ..SketchRegion::default()
        }],
    }
}

/// The picking pass over the rectangle sketch alone — no pickable solid,
/// as the viewport runs it for a sketch-only design — decoded at one
/// pixel.
fn pick_sketch_at(
    gpu: &Gpu,
    renderer: &Renderer,
    filter: SelectionFilter,
    pixel: (u32, u32),
) -> Option<PickedElement> {
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
    let sketch = upload_sketch(&gpu.device, &rectangle_sketch());

    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    render_picking(
        &mut encoder,
        &pipelines,
        &picking,
        &[],
        Some(&sketch),
        filter,
    );
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
        decode_pick_result(&data[..4], filter).map(|pick| pick.element())
    };
    picking.staging_buffer.unmap();
    element
}

fn sketch(kind: SketchElementKind, index: u32) -> Option<PickedElement> {
    Some(PickedElement::Sketch(SketchElement { kind, index }))
}

/// A sketch's region, curves and corners each answer a click, with the
/// same precedence as faces, edges and vertices: a corner over the curves
/// meeting at it, a curve over the region it bounds. The filter's three
/// toggles govern the three kinds.
fn a_sketch_region_curve_and_corner_each_answer_a_click(gpu: &Gpu) {
    let renderer = front_facing_renderer(0);
    let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;
    let all = SelectionFilter::default();

    // The middle of the rectangle is the region and nothing else.
    let centre = pixels_below(view_proj, [0.0, 0.0, 0.0], 0.0);
    assert_eq!(
        pick_sketch_at(gpu, &renderer, all, centre),
        sketch(SketchElementKind::Region, 0)
    );

    // Two pixels inside the bottom curve's midpoint is within its hit
    // quad: the curve, not the region it bounds — and the region once the
    // curve is filtered out.
    let bottom_curve = pixels_below(view_proj, [0.0, 0.0, -1.0], -2.0);
    assert_eq!(
        pick_sketch_at(gpu, &renderer, all, bottom_curve),
        sketch(SketchElementKind::Curve, 0)
    );

    // The corner where two curves meet is the corner.
    let corner = pixels_below(view_proj, [2.0, 0.0, 1.0], 0.0);
    assert_eq!(
        pick_sketch_at(gpu, &renderer, all, corner),
        sketch(SketchElementKind::Corner, 2)
    );

    // With vertices off the corner click reaches the curve under it;
    // with edges off too, the region; with everything off, nothing.
    assert_eq!(
        pick_sketch_at(
            gpu,
            &renderer,
            SelectionFilter {
                vertices: false,
                ..all
            },
            corner
        )
        .map(|element| matches!(
            element,
            PickedElement::Sketch(SketchElement {
                kind: SketchElementKind::Curve,
                ..
            })
        )),
        Some(true)
    );
    assert_eq!(
        pick_sketch_at(
            gpu,
            &renderer,
            SelectionFilter {
                vertices: false,
                edges: false,
                ..all
            },
            bottom_curve
        ),
        sketch(SketchElementKind::Region, 0)
    );
    assert_eq!(
        pick_sketch_at(
            gpu,
            &renderer,
            SelectionFilter {
                faces: false,
                edges: false,
                vertices: false,
            },
            centre
        ),
        None
    );
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

    // Two pixels off the centre line on each side: inside the picking half
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

    // Three pixels off the vertex's own point, inside the hit target's radius.
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

/// A vertex the section has cut away must take its marker with it: the
/// marker pass is expanded in screen space, so nothing about the quad's
/// own geometry clips it — only the discard keyed on the vertex's world
/// position does. With the vertex gone, the click reaches the face behind.
fn a_marker_on_a_clipped_vertex_is_neither_drawn_nor_pickable(gpu: &Gpu) {
    let mut renderer = front_facing_renderer(0);
    let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;
    let on_marker = pixels_below(view_proj, [0.0, 0.0, 0.5], -3.0);

    assert_eq!(
        pick_at(gpu, &renderer, SelectionFilter::default(), on_marker),
        Some(TopologyElement::Vertex(VertexId(0))),
        "the marker must be pickable before the section is asked to cut it away"
    );

    // The backdrop face sits at y = `behind`; the model vertex sits at
    // y = 0. A plane halfway between them keeps the face and cuts the
    // vertex, whichever side of the geometry the camera put the face on.
    let behind = scene(view_proj).0.vertices[0].position[1];
    renderer.section = SectionPlane {
        enabled: true,
        axis: Axis::Y,
        offset: behind / 2.0,
        flipped: behind > 0.0,
    };
    assert!(
        renderer.section.keeps([0.0, behind, 0.0]) && !renderer.section.keeps([0.0, 0.0, 0.5]),
        "the section must keep the face and cut the vertex for this to prove anything"
    );

    assert_eq!(
        pick_at(gpu, &renderer, SelectionFilter::default(), on_marker),
        Some(TopologyElement::Face(FaceId(0))),
        "a marker on a section-clipped vertex is still pickable: the user selects what they cannot see"
    );
}

/// Read the visible scene through the same pass used by the viewport.
fn draw_scene(
    gpu: &Gpu,
    renderer: &Renderer,
    mesh: &TessellatedMesh,
    vertices: &[[f32; 3]],
) -> Vec<u8> {
    draw_scene_format(
        gpu,
        renderer,
        mesh,
        vertices,
        wgpu::TextureFormat::Rgba8Unorm,
    )
}

fn draw_scene_format(
    gpu: &Gpu,
    renderer: &Renderer,
    mesh: &TessellatedMesh,
    vertices: &[[f32; 3]],
    format: wgpu::TextureFormat,
) -> Vec<u8> {
    let pipelines = RenderPipelines::new(&gpu.device, format, WIDTH, HEIGHT);
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("visible_marker_readback"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("visible_marker_pixels"),
        size: u64::from(WIDTH * HEIGHT * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut uniforms = renderer.mesh_uniforms((WIDTH, HEIGHT));
    uniforms.encode_srgb = u32::from(!format.is_srgb());
    gpu.queue.write_buffer(
        &pipelines.mesh_uniform_buffer,
        0,
        bytemuck::bytes_of(&uniforms),
    );
    let mesh = upload_mesh(&gpu.device, mesh, PartId(0), vertices);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    cadmark_renderer::viewport::render_scene_into(
        &mut encoder,
        &pipelines,
        &[mesh],
        None,
        wgpu::Color::WHITE,
        false,
        &target.create_view(&Default::default()),
    );
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &output,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH * 4),
                rows_per_image: Some(HEIGHT),
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit(Some(encoder.finish()));
    let (tx, rx) = std::sync::mpsc::channel();
    output
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
    gpu.device.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap().unwrap();
    let pixels = output.slice(..).get_mapped_range().to_vec();
    output.unmap();
    pixels
}

fn fine_markers_keep_generous_hit_targets(gpu: &Gpu) {
    for zoom_outs in [0, 8] {
        let renderer = front_facing_renderer(zoom_outs);
        let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;
        let (mesh, vertices) = scene(view_proj);
        let visible = draw_scene(gpu, &renderer, &mesh, &vertices);
        let mut backdrop = mesh.clone();
        backdrop.edges.clear();
        let unmarked = draw_scene(gpu, &renderer, &backdrop, &[]);
        for (point, offset, expected) in [
            ([0.0, 0.0, 0.0], 2.0, TopologyElement::Edge(EdgeId(EDGE_ID))),
            (vertices[0], -4.0, TopologyElement::Vertex(VertexId(0))),
        ] {
            let pixel = pixels_below(view_proj, point, offset);
            let index = ((pixel.1 * WIDTH + pixel.0) * 4) as usize;
            assert_eq!(
                &visible[index..index + 4],
                &unmarked[index..index + 4],
                "hit target obscures nearby surface at {pixel:?}"
            );
            assert_eq!(
                pick_at(gpu, &renderer, SelectionFilter::default(), pixel),
                Some(expected)
            );
            let centre = pixels_below(view_proj, point, 0.0);
            let index = ((centre.1 * WIDTH + centre.0) * 4) as usize;
            assert_ne!(
                &visible[index..index + 4],
                &unmarked[index..index + 4],
                "marker itself must remain visible"
            );
        }
    }
}

fn diagonal_edges_and_discs_have_a_smooth_coverage_fringe(gpu: &Gpu) {
    let renderer = front_facing_renderer(0);
    let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;
    let (mut mesh, vertices) = scene(view_proj);
    // Move the surface out of view so marker coverage is measured against
    // a constant white background, independently of the lighting shader.
    for vertex in &mut mesh.vertices {
        vertex.position[0] += 100.0;
    }
    mesh.edges[0].points = vec![[-2.0, 0.0, -0.5], [2.0, 0.0, 0.3]];
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        for (with_edge, with_vertex) in [(true, false), (false, true)] {
            let mut input = mesh.clone();
            if !with_edge {
                input.edges.clear();
            }
            let image = draw_scene_format(
                gpu,
                &renderer,
                &input,
                if with_vertex { &vertices } else { &[] },
                format,
            );
            let dark = image
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| pixel[0] <= 30)
                .count();
            let fringe = image
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| pixel[0] > 30 && pixel[0] < 250)
                .count();
            assert!(dark > 0, "the marker needs a solid centre");
            assert!(
                fringe > 4,
                "the boundary must blend rather than switch abruptly: {fringe} pixels"
            );
            assert!(image.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255));
        }
    }
}

fn a_hidden_edge_does_not_show_through_the_surface(gpu: &Gpu) {
    let renderer = front_facing_renderer(0);
    let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;
    let (mut mesh, _) = scene(view_proj);
    let behind = mesh.vertices[0].position[1] * 2.0;
    mesh.edges[0].points = vec![[-1.0, behind, 0.0], [1.0, behind, 0.0]];
    let image = draw_scene(gpu, &renderer, &mesh, &[]);
    mesh.edges.clear();
    assert_eq!(image, draw_scene(gpu, &renderer, &mesh, &[]));
}

fn an_edge_on_a_sloping_face_has_no_depth_gaps(gpu: &Gpu) {
    let renderer = front_facing_renderer(0);
    let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;
    let (mut mesh, _) = scene(view_proj);
    for vertex in &mut mesh.vertices {
        vertex.position[1] = 0.4 * vertex.position[0] + 0.3 * vertex.position[2];
    }
    mesh.edges[0].points = vec![[-2.0, -0.8, 0.0], [2.0, 0.8, 0.0]];
    let image = draw_scene(gpu, &renderer, &mesh, &[]);
    mesh.edges.clear();
    let plain = draw_scene(gpu, &renderer, &mesh, &[]);
    for step in 0..30 {
        let x = -1.5 + step as f32 * 0.1;
        let pixel = pixels_below(view_proj, [x, 0.4 * x, 0.0], 0.0);
        let index = ((pixel.1 * WIDTH + pixel.0) * 4) as usize;
        assert!(
            i16::from(plain[index]) - i16::from(image[index]) > 30,
            "line disappears into its own surface at {pixel:?}: {} versus {}",
            image[index],
            plain[index]
        );
    }
}

fn surface_markers_keep_their_full_picking_area(gpu: &Gpu) {
    for framed in [false, true] {
        for projection in [
            cadmark_renderer::camera::Projection::Perspective,
            cadmark_renderer::camera::Projection::Orthographic,
        ] {
            let mut renderer = front_facing_renderer(0);
            if framed {
                renderer.camera.frame_bounds(
                    cadmark_renderer::camera::Bounds3 {
                        min: [-2.0; 3],
                        max: [2.0; 3],
                    },
                    WIDTH as f32 / HEIGHT as f32,
                );
            }
            renderer.camera.set_projection(projection);
            for (slope, vertex) in [(0.6, [0.0, 0.6, 1.0]), (2.5, [1.0, 0.0, 0.0])] {
                let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;
                let (mut mesh, _) = scene(view_proj);
                // The line has constant depth. Only the underlying face slopes:
                // an offset based on the line's own depth gradient cannot cover this.
                for vertex in &mut mesh.vertices {
                    vertex.position[1] = slope * vertex.position[2];
                }
                mesh.edges[0].points = vec![[-2.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
                for (point, offsets, expected) in [
                    (
                        [0.0, 0.0, 0.0],
                        [-2.0, 2.0],
                        TopologyElement::Edge(EdgeId(EDGE_ID)),
                    ),
                    (vertex, [-4.0, 4.0], TopologyElement::Vertex(VertexId(0))),
                ] {
                    for offset in offsets {
                        assert_eq!(
                            pick_at_in(
                                gpu,
                                &renderer,
                                SelectionFilter::default(),
                                pixels_below(view_proj, point, offset),
                                |_| vec![(mesh.clone(), vec![vertex])]
                            ),
                            Some(expected.clone()),
                            "surface clips the target at offset {offset}"
                        );
                    }
                }
            }
        }
    }
}

fn a_grazing_surface_does_not_reveal_hidden_markers(gpu: &Gpu) {
    for framed in [false, true] {
        for projection in [
            cadmark_renderer::camera::Projection::Perspective,
            cadmark_renderer::camera::Projection::Orthographic,
        ] {
            let mut renderer = front_facing_renderer(0);
            if framed {
                renderer.camera.frame_bounds(
                    cadmark_renderer::camera::Bounds3 {
                        min: [-2.0; 3],
                        max: [2.0; 3],
                    },
                    WIDTH as f32 / HEIGHT as f32,
                );
            }
            renderer.camera.set_projection(projection);
            let view_proj = renderer.simple_uniforms((WIDTH, HEIGHT)).view_proj;
            let (mut mesh, _) = scene(view_proj);
            for vertex in &mut mesh.vertices {
                vertex.position[2] *= 0.025;
                vertex.position[1] = 20.0 * vertex.position[2];
            }
            let behind =
                if project(view_proj, [0.0, 0.3, 0.0]).2 > project(view_proj, [0.0, 0.0, 0.0]).2 {
                    0.3
                } else {
                    -0.3
                };
            mesh.edges[0].points = vec![[-1.0, behind, 0.0], [1.0, behind, 0.0]];
            let vertex = [0.0, behind, 0.0];
            let image = draw_scene(gpu, &renderer, &mesh, &[vertex]);
            let mut plain = mesh.clone();
            plain.edges.clear();
            let background = draw_scene(gpu, &renderer, &plain, &[]);
            let pixel = pixels_below(view_proj, vertex, 0.0);
            let index = ((pixel.1 * WIDTH + pixel.0) * 4) as usize;
            assert_eq!(
                &image[index..index + 4],
                &background[index..index + 4],
                "hidden markers show through a grazing face"
            );
            assert_eq!(
                pick_at_in(gpu, &renderer, SelectionFilter::default(), pixel, |_| vec![
                    (mesh.clone(), vec![vertex])
                ]),
                Some(TopologyElement::Face(FaceId(0))),
                "a grazing face must occlude hidden markers"
            );
        }
    }
}

type Check = (&'static str, fn(&Gpu));

/// Runs the checks against one software device and reports them the way
/// libtest would, so a failure is as visible in the gate's output as any
/// other test's.
///
/// The harness answers the libtest command line that both `cargo test`
/// and cargo-nextest drive it with: `--list --format terse` prints one
/// `name: test` line per check (and nothing under `--ignored`, since none
/// is), positional names select checks by substring or, with `--exact`,
/// by whole name, and `--nocapture` is accepted and ignored because the
/// output is never captured. Nextest lists the binary once and then runs
/// each check in its own process, each acquiring the device afresh.
fn main() {
    let checks: [Check; 13] = [
        (
            "a_sketch_region_curve_and_corner_each_answer_a_click",
            a_sketch_region_curve_and_corner_each_answer_a_click,
        ),
        (
            "a_grazing_surface_does_not_reveal_hidden_markers",
            a_grazing_surface_does_not_reveal_hidden_markers,
        ),
        (
            "surface_markers_keep_their_full_picking_area",
            surface_markers_keep_their_full_picking_area,
        ),
        (
            "an_edge_on_a_sloping_face_has_no_depth_gaps",
            an_edge_on_a_sloping_face_has_no_depth_gaps,
        ),
        (
            "diagonal_edges_and_discs_have_a_smooth_coverage_fringe",
            diagonal_edges_and_discs_have_a_smooth_coverage_fringe,
        ),
        (
            "a_hidden_edge_does_not_show_through_the_surface",
            a_hidden_edge_does_not_show_through_the_surface,
        ),
        (
            "fine_markers_keep_generous_hit_targets",
            fine_markers_keep_generous_hit_targets,
        ),
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
        (
            "a_marker_on_a_clipped_vertex_is_neither_drawn_nor_pickable",
            a_marker_on_a_clipped_vertex_is_neither_drawn_nor_pickable,
        ),
    ];

    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let ignored_only = flag("--ignored");

    if flag("--list") {
        if !ignored_only {
            for (name, _) in checks {
                println!("{name}: test");
            }
        }
        return;
    }

    let filters: Vec<&str> = args
        .iter()
        .filter(|arg| !arg.starts_with("--"))
        .map(String::as_str)
        .collect();
    let exact = flag("--exact");
    let selected: Vec<Check> = if ignored_only {
        Vec::new()
    } else {
        checks
            .into_iter()
            .filter(|(name, _)| {
                filters.is_empty()
                    || filters.iter().any(|filter| {
                        if exact {
                            name == filter
                        } else {
                            name.contains(filter)
                        }
                    })
            })
            .collect()
    };
    if selected.is_empty() {
        println!("\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed\n");
        return;
    }

    let gpu = cadmark_renderer::test_device::software();
    println!(
        "\nrunning {} tests on software adapter: {}",
        selected.len(),
        gpu.adapter.name
    );

    let mut failed = 0;
    for (name, check) in &selected {
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
        selected.len() - failed
    );
    if failed > 0 {
        std::process::exit(1);
    }
}
