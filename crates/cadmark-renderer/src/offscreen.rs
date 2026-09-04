// Rendering the scene to pixels a caller can read, with no window and no
// surface. The on-screen viewport's pipelines already draw into an
// offscreen colour texture before blitting; this owns the same passes
// against its own texture, sized to whatever the caller asks for, and
// reads the result back as RGBA bytes.
//
// It renders what it is handed — a mesh, uniforms and a clear colour —
// and holds nothing between calls except its GPU resources, so it never
// touches the live viewport's target, camera or timing.

use cadmark_core::mesh::TessellatedMesh;

use crate::mesh::GpuMesh;
use crate::pipeline::{MeshUniforms, RenderPipelines};
use crate::viewport::render_scene_into;

/// A row of a texture-to-buffer copy must start on a 256-byte boundary,
/// so the readback buffer is padded and the padding stripped after
/// mapping.
const COPY_ROW_ALIGNMENT: u32 = 256;

/// Bytes per pixel of the readback format.
const BYTES_PER_PIXEL: u32 = 4;

/// The format the offscreen target stores. It is not an sRGB format, so
/// the shader display-encodes its own output: build the uniforms from a
/// renderer whose `target_is_srgb` is false.
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// An image rendered off-screen: RGBA rows, top to bottom, no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedImage {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes of non-premultiplied RGBA.
    pub rgba: Vec<u8>,
}

/// What went wrong producing an offscreen image.
#[derive(Debug, thiserror::Error)]
pub enum OffscreenError {
    #[error("a render of {width}x{height} is not a usable image")]
    UnusableSize { width: u32, height: u32 },
    #[error("the rendered image could not be read back from the GPU: {0}")]
    Readback(String),
}

/// Pipelines and textures for rendering at one fixed size. Rebuilt when
/// the requested size changes; a render at the same size reuses it.
pub struct OffscreenRenderer {
    pipelines: RenderPipelines,
    colour: wgpu::Texture,
    colour_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
}

impl OffscreenRenderer {
    /// Build a renderer targeting `width` x `height` pixels.
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Result<Self, OffscreenError> {
        if width == 0 || height == 0 {
            return Err(OffscreenError::UnusableSize { width, height });
        }
        let pipelines = RenderPipelines::new(device, OFFSCREEN_FORMAT, width, height);
        let colour = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen_colour_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let colour_view = colour.create_view(&wgpu::TextureViewDescriptor::default());
        let padded_bytes_per_row = padded_bytes_per_row(width);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen_readback_buffer"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(Self {
            pipelines,
            colour,
            colour_view,
            readback,
            width,
            height,
            padded_bytes_per_row,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Draw `mesh` with `uniforms` and read the result back. Blocks until
    /// the GPU has finished, so the caller is a worker thread, never the
    /// frame loop.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: Option<&TessellatedMesh>,
        sketch: Option<&cadmark_core::sketch::SketchProfile>,
        uniforms: &MeshUniforms,
        clear_colour: wgpu::Color,
    ) -> Result<RenderedImage, OffscreenError> {
        // One mesh, uploaded as part zero: this path renders a framed
        // picture, not a pickable scene, so no part distinction is read
        // back from it.
        let gpu_mesh: Vec<GpuMesh> = mesh
            .map(|mesh| {
                // No vertex markers: this path renders a framed picture,
                // and a marker is an aiming aid for a viewport the user
                // is clicking in.
                crate::pipeline::upload_mesh(
                    device,
                    mesh,
                    cadmark_core::geometry::PartId(0),
                    &[],
                )
            })
            .into_iter()
            .collect();
        let gpu_sketch = sketch.map(|sketch| crate::pipeline::upload_sketch(device, sketch));

        queue.write_buffer(
            &self.pipelines.mesh_uniform_buffer,
            0,
            bytemuck::bytes_of(uniforms),
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("offscreen_encoder"),
        });
        render_scene_into(
            &mut encoder,
            &self.pipelines,
            &gpu_mesh,
            gpu_sketch.as_ref(),
            clear_colour,
            // The offscreen render is what the AI is shown, so it draws
            // the model the way the user has it: the same uniforms carry
            // the section plane, and the alpha they carry chooses the
            // pass, exactly as the viewport's own selector does.
            uniforms.mesh_alpha < 1.0,
            &self.colour_view,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.colour,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |status| {
            let _ = tx.send(status);
        });
        device.poll(wgpu::Maintain::Wait);
        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.readback.unmap();
                return Err(OffscreenError::Readback(error.to_string()));
            }
            Err(error) => {
                self.readback.unmap();
                return Err(OffscreenError::Readback(error.to_string()));
            }
        }
        let padded = slice.get_mapped_range();
        let rgba = unpad_rows(&padded, self.width, self.height, self.padded_bytes_per_row);
        drop(padded);
        self.readback.unmap();

        Ok(RenderedImage {
            width: self.width,
            height: self.height,
            rgba,
        })
    }
}

/// The row stride a texture-to-buffer copy of `width` RGBA pixels needs.
pub fn padded_bytes_per_row(width: u32) -> u32 {
    let unpadded = width * BYTES_PER_PIXEL;
    unpadded.div_ceil(COPY_ROW_ALIGNMENT) * COPY_ROW_ALIGNMENT
}

/// Strip the copy alignment padding from the end of every row.
fn unpad_rows(padded: &[u8], width: u32, height: u32, padded_bytes_per_row: u32) -> Vec<u8> {
    let row_bytes = (width * BYTES_PER_PIXEL) as usize;
    let stride = padded_bytes_per_row as usize;
    let mut rgba = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as usize {
        let start = row * stride;
        rgba.extend_from_slice(&padded[start..start + row_bytes]);
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_padding_rounds_up_to_the_copy_alignment() {
        // 64 RGBA pixels is exactly one aligned row; one more pixel needs
        // a second.
        assert_eq!(padded_bytes_per_row(64), 256);
        assert_eq!(padded_bytes_per_row(65), 512);
        assert_eq!(padded_bytes_per_row(1), 256);
    }

    /// A device for the rendering tests: the real adapter where there is
    /// one, the software fallback otherwise. A runner with neither fails
    /// the test rather than passing without rendering anything.
    fn gpu() -> (wgpu::Device, wgpu::Queue) {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(async {
            for force_fallback_adapter in [false, true] {
                let adapter = instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::LowPower,
                        compatible_surface: None,
                        force_fallback_adapter,
                    })
                    .await;
                if adapter.is_some() {
                    return adapter;
                }
            }
            None
        })
        .expect("no GPU adapter and no software fallback: install a Vulkan ICD or lavapipe");
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
            .expect("the adapter gave no device")
    }

    /// A unit cube centred on the origin, with its twelve edges as the
    /// wireframe overlay.
    fn cube() -> TessellatedMesh {
        use cadmark_core::mesh::{MeshEdge, MeshVertex};

        let corners = [
            [-0.5f32, -0.5, -0.5],
            [0.5, -0.5, -0.5],
            [0.5, 0.5, -0.5],
            [-0.5, 0.5, -0.5],
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
            [0.5, 0.5, 0.5],
            [-0.5, 0.5, 0.5],
        ];
        // Per-face quads, so each face carries its own normal and ID.
        let faces: [([usize; 4], [f32; 3]); 6] = [
            ([0, 3, 2, 1], [0.0, 0.0, -1.0]),
            ([4, 5, 6, 7], [0.0, 0.0, 1.0]),
            ([0, 1, 5, 4], [0.0, -1.0, 0.0]),
            ([2, 3, 7, 6], [0.0, 1.0, 0.0]),
            ([1, 2, 6, 5], [1.0, 0.0, 0.0]),
            ([3, 0, 4, 7], [-1.0, 0.0, 0.0]),
        ];

        let mut mesh = TessellatedMesh::default();
        for (face_id, (quad, normal)) in faces.iter().enumerate() {
            let base = mesh.vertices.len() as u32;
            for &corner in quad {
                mesh.vertices.push(MeshVertex {
                    position: corners[corner],
                    normal: *normal,
                });
            }
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            mesh.face_ids
                .extend_from_slice(&[face_id as u32, face_id as u32]);
        }
        for (edge_id, (a, b)) in [
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 0),
            (4, 5),
            (5, 6),
            (6, 7),
            (7, 4),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ]
        .into_iter()
        .enumerate()
        {
            mesh.edges.push(MeshEdge {
                points: vec![corners[a], corners[b]],
                edge_id: edge_id as u32,
            });
        }
        mesh
    }

    #[test]
    fn a_rendered_model_fills_the_image_and_is_shaded() {
        let (device, queue) = gpu();
        let (width, height) = (320u32, 240u32);
        let renderer = OffscreenRenderer::new(&device, width, height).expect("offscreen renderer");

        let mut scene = crate::pipeline::Renderer::new();
        scene.target_is_srgb = false;
        let bounds = crate::camera::Bounds3::from_positions(
            cube().vertices.iter().map(|vertex| vertex.position),
        )
        .expect("cube bounds");
        scene
            .camera
            .frame_bounds(bounds, width as f32 / height as f32);
        // Light grey, well away from both the shaded faces and the dark
        // edge colour, so each is countable in the readback.
        let clear = wgpu::Color {
            r: 0.6,
            g: 0.6,
            b: 0.6,
            a: 1.0,
        };

        let image = renderer
            .render(
                &device,
                &queue,
                Some(&cube()),
                None,
                &scene.mesh_uniforms((width, height)),
                clear,
            )
            .expect("render");

        assert_eq!((image.width, image.height), (width, height));
        assert_eq!(image.rgba.len(), (width * height * 4) as usize);

        // The clear colour is what an empty render leaves; anything else
        // is the model. It must occupy a real share of the frame — a
        // model in a corner or clipped away would not.
        let background = [153u8, 153, 153];
        let is_model = |pixel: &[u8]| {
            pixel[0].abs_diff(background[0]) > 6
                || pixel[1].abs_diff(background[1]) > 6
                || pixel[2].abs_diff(background[2]) > 6
        };
        let model_pixels = image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| is_model(p.as_slice()))
            .count();
        let coverage = model_pixels as f32 / (width * height) as f32;
        assert!(
            coverage > 0.2,
            "the model covers only {coverage:.3} of the frame"
        );

        // Shaded, not a flat silhouette: the lit faces of a cube differ
        // from one another, and the wireframe adds darker edge pixels.
        let mut levels: Vec<u8> = image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| is_model(p.as_slice()))
            .map(|p| p[0])
            .collect();
        levels.sort_unstable();
        levels.dedup();
        assert!(
            levels.len() > 8,
            "only {} distinct luminance levels — the render is not shaded",
            levels.len()
        );

        // Edges drawn, not just shaded faces: the wireframe overlay
        // paints its own dark colour, which no face shade reaches.
        let edge = [26u8, 26, 31];
        let edge_pixels = image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| {
                p[0].abs_diff(edge[0]) <= 2
                    && p[1].abs_diff(edge[1]) <= 2
                    && p[2].abs_diff(edge[2]) <= 2
            })
            .count();
        assert!(
            edge_pixels > 50,
            "only {edge_pixels} edge-coloured pixels — the wireframe overlay is missing"
        );
    }

    #[test]
    fn an_empty_render_is_the_clear_colour_alone() {
        let (device, queue) = gpu();
        let renderer = OffscreenRenderer::new(&device, 64, 64).expect("offscreen renderer");
        let scene = crate::pipeline::Renderer::new();
        let image = renderer
            .render(
                &device,
                &queue,
                None,
                None,
                &scene.mesh_uniforms((512, 512)),
                wgpu::Color {
                    r: 0.0,
                    g: 0.0,
                    b: 1.0,
                    a: 1.0,
                },
            )
            .expect("render");
        assert!(
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[2] > 250 && pixel[0] < 5),
            "an empty scene rendered something other than the clear colour"
        );
    }

    /// A 4-by-2 rectangle on the XY plane, drawn as four curves round one
    /// filled region with a corner at each end.
    fn rectangle_profile() -> cadmark_core::sketch::SketchProfile {
        use cadmark_core::sketch::{SketchCorner, SketchCurve, SketchProfile, SketchRegion};

        let corners = [
            [-2.0f32, -1.0, 0.0],
            [2.0, -1.0, 0.0],
            [2.0, 1.0, 0.0],
            [-2.0, 1.0, 0.0],
        ];
        SketchProfile {
            plane: Default::default(),
            curves: (0..4)
                .map(|index| SketchCurve {
                    curve_id: index as u32,
                    points: vec![corners[index], corners[(index + 1) % 4]],
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
            }],
        }
    }

    #[test]
    fn a_sketch_only_model_draws_its_profile_where_the_solid_would_be() {
        let (device, queue) = gpu();
        let (width, height) = (200u32, 200u32);
        let renderer = OffscreenRenderer::new(&device, width, height).expect("offscreen renderer");

        let profile = rectangle_profile();
        let mut scene = crate::pipeline::Renderer::new();
        scene.camera.view_plane_face_on(profile.plane.normal);
        let bounds = crate::camera::Bounds3::from_positions(profile.points()).expect("bounds");
        scene.camera.frame_bounds(bounds, 1.0);

        let image = renderer
            .render(
                &device,
                &queue,
                None,
                Some(&profile),
                &scene.mesh_uniforms((512, 512)),
                wgpu::Color {
                    r: 0.6,
                    g: 0.6,
                    b: 0.6,
                    a: 1.0,
                },
            )
            .expect("render");

        let pixels = image.rgba.as_chunks::<4>().0;
        let at = |x: u32, y: u32| pixels[(y * width + x) as usize];
        // The middle of the frame is inside the region: the wash reads
        // bluer than the grey it covers, which an empty render never does.
        let centre = at(width / 2, height / 2);
        assert!(
            centre[2] > centre[0] + 12,
            "the centre pixel {centre:?} is not the region wash"
        );
        // A corner of the frame is outside the profile entirely.
        let corner = at(2, 2);
        assert!(
            corner[2].abs_diff(corner[0]) < 6,
            "the frame's corner {corner:?} is not the background"
        );
        // The curves are drawn, and darker than the wash.
        let drawn = pixels
            .iter()
            .filter(|pixel| pixel[2] > 100 && pixel[0] < 60)
            .count();
        assert!(
            drawn > 100,
            "only {drawn} curve or corner pixels — the profile's lines are missing"
        );
    }

    /// The see-through view has to actually let the background through.
    ///
    /// A solid cube drawn opaque covers the background completely where
    /// it sits; drawn see-through, the same pixels must move back
    /// towards the background without disappearing into it. Comparing
    /// one interior pixel against both is what separates "blended" from
    /// "the opaque pass ran anyway".
    #[test]
    fn a_see_through_solid_lets_the_background_through() {
        let (device, queue) = gpu();
        let (width, height) = (160u32, 160u32);
        let renderer = OffscreenRenderer::new(&device, width, height).expect("offscreen renderer");

        let mut scene = crate::pipeline::Renderer::new();
        let bounds = crate::camera::Bounds3::from_positions(
            cube().vertices.iter().map(|vertex| vertex.position),
        )
        .expect("cube bounds");
        scene.camera.frame_bounds(bounds, 1.0);
        scene
            .camera
            .look_at_standard(crate::camera::StandardView::Front);
        let background = [40u8, 42, 48];
        let clear = wgpu::Color {
            r: f64::from(background[0]) / 255.0,
            g: f64::from(background[1]) / 255.0,
            b: f64::from(background[2]) / 255.0,
            a: 1.0,
        };
        let draw = |scene: &crate::pipeline::Renderer| {
            renderer
                .render(
                    &device,
                    &queue,
                    Some(&cube()),
                    None,
                    &scene.mesh_uniforms((512, 512)),
                    clear,
                )
                .expect("render")
        };
        // The image centre: the cube is framed there, so this pixel is
        // the front face in both renders.
        let centre = |image: &RenderedImage| {
            let index = ((height / 2) * width + width / 2) as usize * 4;
            [
                image.rgba[index],
                image.rgba[index + 1],
                image.rgba[index + 2],
            ]
        };
        let distance = |pixel: [u8; 3]| -> u32 {
            (0..3)
                .map(|channel| u32::from(pixel[channel].abs_diff(background[channel])))
                .sum()
        };

        let opaque = distance(centre(&draw(&scene)));
        assert!(opaque > 60, "the opaque cube is not covering the centre");

        scene.transparent = true;
        let see_through = distance(centre(&draw(&scene)));

        // Both the near and the far face land on this pixel — the pass
        // culls nothing, so an open shell's far side is still revealed —
        // and each blends in turn. Two layers at the pass's own opacity
        // leave the centre a little over half the opaque reading, so the
        // bar is set below that rather than at one layer's share.
        assert!(
            see_through * 4 < opaque * 3,
            "the see-through surface reads at {see_through} against the opaque {opaque} — the background is not showing through"
        );
        assert!(
            see_through > opaque / 20,
            "the see-through surface vanished entirely at {see_through}; it must still be visibly there"
        );
    }

    /// The section is meant to be *looked through*, so the proof that
    /// matters is pixels: half the solid must stop being drawn.
    ///
    /// The cube is framed head-on and cut along X through its centre.
    /// Counting how many pixels differ from the background before and
    /// after gives the cut's own effect, and the surviving pixels must
    /// all sit on one side of the image — a shader that discarded
    /// nothing, or discarded everywhere, fails both halves of this.
    #[test]
    fn a_section_plane_stops_half_the_solid_being_drawn() {
        let (device, queue) = gpu();
        let (width, height) = (160u32, 160u32);
        let renderer = OffscreenRenderer::new(&device, width, height).expect("offscreen renderer");

        let mut scene = crate::pipeline::Renderer::new();
        let bounds = crate::camera::Bounds3::from_positions(
            cube().vertices.iter().map(|vertex| vertex.position),
        )
        .expect("cube bounds");
        scene.camera.frame_bounds(bounds, 1.0);
        scene
            .camera
            .look_at_standard(crate::camera::StandardView::Front);
        let background = [40u8, 42, 48];
        let clear = wgpu::Color {
            r: f64::from(background[0]) / 255.0,
            g: f64::from(background[1]) / 255.0,
            b: f64::from(background[2]) / 255.0,
            a: 1.0,
        };
        let draw = |scene: &crate::pipeline::Renderer| {
            renderer
                .render(
                    &device,
                    &queue,
                    Some(&cube()),
                    None,
                    &scene.mesh_uniforms((512, 512)),
                    clear,
                )
                .expect("render")
        };

        // Which pixels are the solid rather than the background, as a
        // column-indexed grid.
        let drawn = |image: &RenderedImage| -> Vec<(u32, u32)> {
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .filter(|(_, pixel)| {
                    (0..3).any(|channel| pixel[channel].abs_diff(background[channel]) > 12)
                })
                .map(|(index, _)| (index as u32 % width, index as u32 / width))
                .collect()
        };

        let whole = drawn(&draw(&scene));
        assert!(
            whole.len() > 2000,
            "only {} pixels of solid before the cut — the scene is not set up",
            whole.len()
        );

        scene.section.enabled = true;
        scene
            .section
            .cut_along(crate::section::Axis::X, Some(bounds));
        let cut = drawn(&draw(&scene));

        assert!(
            !cut.is_empty(),
            "the section discarded the whole model, not half of it"
        );
        assert!(
            cut.len() * 4 < whole.len() * 3,
            "{} pixels survived the cut against {} before it — nothing was clipped away",
            cut.len(),
            whole.len()
        );

        // Everything left standing is on one side of the cut. The plane
        // is axis-aligned and the view is head-on, so the boundary is a
        // column: the kept pixels must not straddle the whole width the
        // uncut solid covered.
        let span = |pixels: &[(u32, u32)]| {
            let columns: Vec<u32> = pixels.iter().map(|(x, _)| *x).collect();
            let (low, high) = (
                *columns.iter().min().expect("pixels"),
                *columns.iter().max().expect("pixels"),
            );
            high - low
        };
        assert!(
            span(&cut) * 3 < span(&whole) * 2,
            "the kept pixels span {} columns against the uncut {} — the survivors are not one side of a cut",
            span(&cut),
            span(&whole)
        );

        // Flipping keeps the other half: the two halves must not overlap.
        scene.section.flip();
        let flipped = drawn(&draw(&scene));
        let kept: std::collections::HashSet<_> = cut.iter().copied().collect();
        let overlap = flipped.iter().filter(|pixel| kept.contains(pixel)).count();
        assert!(
            !flipped.is_empty() && overlap * 10 < flipped.len(),
            "{overlap} of {} flipped pixels coincide with the {} kept before the flip — the flip did not swap halves",
            flipped.len(),
            cut.len()
        );
    }

    #[test]
    fn a_ghosted_solid_fades_towards_the_background() {
        let (device, queue) = gpu();
        let (width, height) = (160u32, 160u32);
        let renderer = OffscreenRenderer::new(&device, width, height).expect("offscreen renderer");

        let mut scene = crate::pipeline::Renderer::new();
        let bounds = crate::camera::Bounds3::from_positions(
            cube().vertices.iter().map(|vertex| vertex.position),
        )
        .expect("cube bounds");
        scene.camera.frame_bounds(bounds, 1.0);
        // The viewport background itself, so what is measured is how far
        // the solid stands out from it.
        let background = [40u8, 42, 48];
        let clear = wgpu::Color {
            r: f64::from(background[0]) / 255.0,
            g: f64::from(background[1]) / 255.0,
            b: f64::from(background[2]) / 255.0,
            a: 1.0,
        };

        let lit = renderer
            .render(
                &device,
                &queue,
                Some(&cube()),
                None,
                &scene.mesh_uniforms((512, 512)),
                clear,
            )
            .expect("render");
        scene.ghost_solid = true;
        let ghosted = renderer
            .render(
                &device,
                &queue,
                Some(&cube()),
                None,
                &scene.mesh_uniforms((512, 512)),
                clear,
            )
            .expect("render");

        let contrast = |image: &RenderedImage| {
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| {
                    (0..3)
                        .map(|channel| u64::from(pixel[channel].abs_diff(background[channel])))
                        .sum::<u64>()
                })
                .sum::<u64>()
        };
        let (lit, ghosted) = (contrast(&lit), contrast(&ghosted));
        assert!(
            ghosted * 3 < lit,
            "the ghosted solid stands out {ghosted} against the lit solid's {lit} — it has not faded back"
        );
        assert!(
            ghosted > lit / 40,
            "the ghosted solid vanished into the background entirely"
        );
    }

    #[test]
    fn unpadding_keeps_only_each_rows_pixels() {
        // Two rows of one pixel each, padded to a 256-byte stride: the
        // pixels are the first four bytes of each row and the rest is
        // whatever the copy left behind.
        let mut padded = vec![0xEEu8; 512];
        padded[0..4].copy_from_slice(&[1, 2, 3, 4]);
        padded[256..260].copy_from_slice(&[5, 6, 7, 8]);
        assert_eq!(unpad_rows(&padded, 1, 2, 256), vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
