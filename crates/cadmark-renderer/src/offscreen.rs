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
        uniforms: &MeshUniforms,
        clear_colour: wgpu::Color,
    ) -> Result<RenderedImage, OffscreenError> {
        let gpu_mesh: Option<GpuMesh> = mesh.map(|mesh| crate::pipeline::upload_mesh(device, mesh));

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
            gpu_mesh.as_ref(),
            clear_colour,
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

    /// A device for the GPU-backed tests, or `None` where no adapter is
    /// available. The test that needs one says so rather than passing
    /// silently.
    fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).ok()
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
        let Some((device, queue)) = gpu() else {
            panic!("no wgpu adapter available; this test needs a GPU or software adapter");
        };
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
        let clear = wgpu::Color {
            r: 0.1,
            g: 0.1,
            b: 0.1,
            a: 1.0,
        };

        let image = renderer
            .render(
                &device,
                &queue,
                Some(&cube()),
                &scene.mesh_uniforms(width as f32 / height as f32),
                clear,
            )
            .expect("render");

        assert_eq!((image.width, image.height), (width, height));
        assert_eq!(image.rgba.len(), (width * height * 4) as usize);

        // The clear colour is what an empty render leaves; anything else
        // is the model. It must occupy a real share of the frame — a
        // model in a corner or clipped away would not.
        let background = [26u8, 26, 26];
        let is_model = |pixel: &[u8]| {
            pixel[0].abs_diff(background[0]) > 6
                || pixel[1].abs_diff(background[1]) > 6
                || pixel[2].abs_diff(background[2]) > 6
        };
        let model_pixels = image.rgba.chunks_exact(4).filter(|p| is_model(p)).count();
        let coverage = model_pixels as f32 / (width * height) as f32;
        assert!(
            coverage > 0.2,
            "the model covers only {coverage:.3} of the frame"
        );

        // Shaded, not a flat silhouette: the lit faces of a cube differ
        // from one another, and the wireframe adds darker edge pixels.
        let mut levels: Vec<u8> = image
            .rgba
            .chunks_exact(4)
            .filter(|p| is_model(p))
            .map(|p| p[0])
            .collect();
        levels.sort_unstable();
        levels.dedup();
        assert!(
            levels.len() > 8,
            "only {} distinct luminance levels — the render is not shaded",
            levels.len()
        );
    }

    #[test]
    fn an_empty_render_is_the_clear_colour_alone() {
        let Some((device, queue)) = gpu() else {
            panic!("no wgpu adapter available; this test needs a GPU or software adapter");
        };
        let renderer = OffscreenRenderer::new(&device, 64, 64).expect("offscreen renderer");
        let scene = crate::pipeline::Renderer::new();
        let image = renderer
            .render(
                &device,
                &queue,
                None,
                &scene.mesh_uniforms(1.0),
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
                .chunks_exact(4)
                .all(|pixel| pixel[2] > 250 && pixel[0] < 5),
            "an empty scene rendered something other than the clear colour"
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
