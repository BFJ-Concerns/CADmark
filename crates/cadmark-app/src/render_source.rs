// The render tool's production source: what the AI sees when it asks to
// look at the model it just built.
//
// The turn runs on the worker thread while the user keeps orbiting,
// clicking and typing, so nothing here touches the live viewport. The UI
// thread publishes a snapshot — the mesh on screen, the camera, the
// viewport's pixel size — into a `SceneHandle`; the worker thread takes a
// copy of that snapshot and renders it on the renderer's offscreen path,
// with its own textures and its own camera, and encodes the result as a
// PNG the transport can carry.

use std::sync::{Arc, Mutex};

use cadmark_bridge::backend::ImageData;
use cadmark_bridge::tools::RenderView;
use cadmark_core::mesh::TessellatedMesh;
use cadmark_renderer::camera::{Bounds3, Camera, StandardView};
use cadmark_renderer::offscreen::{OffscreenRenderer, RenderedImage};
use cadmark_renderer::pipeline::Renderer;

use crate::turn::RenderSource;

/// The shortest side of a render the AI is expected to judge geometry
/// from. A viewport smaller than this is scaled up rather than sent as a
/// thumbnail nothing can be read from.
const MIN_SHORT_SIDE: u32 = 512;

/// The longest side sent to a provider. Beyond this the image costs more
/// than the detail is worth.
const MAX_LONG_SIDE: u32 = 1536;

/// The background the AI's render is drawn on. Mid-grey reads against
/// both lit and shadowed faces; the offscreen target is not an sRGB
/// format, so this is the stored value directly.
const RENDER_CLEAR: wgpu::Color = wgpu::Color {
    r: 0.14,
    g: 0.15,
    b: 0.17,
    a: 1.0,
};

/// What the UI thread last showed, as the worker thread needs it.
#[derive(Clone, Default)]
struct Scene {
    /// The mesh on screen. Shared rather than copied: a turn's render
    /// must not depend on the mesh still being current when it runs.
    mesh: Option<Arc<TessellatedMesh>>,
    bounds: Option<Bounds3>,
    camera: Camera,
    /// The viewport's size in physical pixels.
    viewport_size: (u32, u32),
}

/// The UI thread's side of the snapshot: cheap to clone, written every
/// frame, read by the worker thread when a turn asks for a render.
#[derive(Clone, Default)]
pub struct SceneHandle(Arc<Mutex<Scene>>);

impl SceneHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish the model now on screen, or its absence.
    pub fn set_mesh(&self, mesh: Option<(Arc<TessellatedMesh>, Option<Bounds3>)>) {
        let mut scene = self.lock();
        match mesh {
            Some((mesh, bounds)) => {
                scene.bounds = bounds.or_else(|| {
                    Bounds3::from_positions(mesh.vertices.iter().map(|vertex| vertex.position))
                });
                scene.mesh = Some(mesh);
            }
            None => {
                scene.mesh = None;
                scene.bounds = None;
            }
        }
    }

    /// Publish the camera the user is looking through and the size of the
    /// viewport showing it.
    pub fn set_view(&self, camera: Camera, viewport_size: (u32, u32)) {
        let mut scene = self.lock();
        scene.camera = camera;
        scene.viewport_size = viewport_size;
    }

    fn snapshot(&self) -> Scene {
        self.lock().clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Scene> {
        // A poisoned lock means a UI-thread panic mid-publish; the scene
        // is plain data, so the worst it holds is a stale frame.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The GPU handles the offscreen render runs on. They are the same device
/// the viewport draws with — the wgpu handles are reference-counted, so
/// this is a share, not a second device — but the render targets textures
/// of its own.
#[derive(Clone)]
pub struct RenderGpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// The production `RenderSource`. Lives on the worker thread; keeps its
/// offscreen renderer between calls and rebuilds it when the requested
/// size changes.
pub struct ViewportRender {
    scene: SceneHandle,
    gpu: RenderGpu,
    offscreen: Option<OffscreenRenderer>,
}

impl ViewportRender {
    pub fn new(scene: SceneHandle, gpu: RenderGpu) -> Self {
        Self {
            scene,
            gpu,
            offscreen: None,
        }
    }
}

impl RenderSource for ViewportRender {
    fn render(&mut self, view: RenderView) -> Result<ImageData, String> {
        let scene = self.scene.snapshot();
        let (Some(mesh), Some(bounds)) = (scene.mesh.clone(), scene.bounds) else {
            return Err("there is no model on screen to render; run the script first".to_string());
        };

        let (width, height) = render_size(scene.viewport_size);
        let aspect = width as f32 / height as f32;
        let camera = framed_camera(&scene.camera, view, bounds, aspect);

        if self.offscreen.as_ref().map(OffscreenRenderer::size) != Some((width, height)) {
            self.offscreen = Some(
                OffscreenRenderer::new(&self.gpu.device, width, height)
                    .map_err(|error| error.to_string())?,
            );
        }
        let offscreen = self
            .offscreen
            .as_ref()
            .expect("the offscreen renderer was just built");

        let mut state = Renderer::new();
        state.camera = camera;
        // The offscreen target is not an sRGB format, so the shader
        // display-encodes its own output.
        state.target_is_srgb = false;

        let image = offscreen
            .render(
                &self.gpu.device,
                &self.gpu.queue,
                Some(mesh.as_ref()),
                &state.mesh_uniforms(aspect),
                RENDER_CLEAR,
            )
            .map_err(|error| error.to_string())?;

        Ok(ImageData {
            media_type: "image/png".to_string(),
            bytes: encode_png(&image).map_err(|error| error.to_string())?,
        })
    }
}

/// The camera one render looks through: the user's own for `current`, a
/// named direction otherwise, and in both cases distanced so the whole
/// model is in frame. An image the model overflows or sits in a corner of
/// answers nothing.
pub fn framed_camera(
    user_camera: &Camera,
    view: RenderView,
    bounds: Bounds3,
    aspect_ratio: f32,
) -> Camera {
    let mut camera = user_camera.clone();
    if let Some(standard) = standard_view(view) {
        camera.look_at_standard(standard);
    }
    camera.frame_bounds(bounds, aspect_ratio);
    camera
}

/// The standard view the tool's argument names, or `None` for the user's
/// current camera direction.
fn standard_view(view: RenderView) -> Option<StandardView> {
    match view {
        RenderView::Current => None,
        RenderView::Front => Some(StandardView::Front),
        RenderView::Back => Some(StandardView::Back),
        RenderView::Left => Some(StandardView::Left),
        RenderView::Right => Some(StandardView::Right),
        RenderView::Top => Some(StandardView::Top),
        RenderView::Bottom => Some(StandardView::Bottom),
        RenderView::Isometric => Some(StandardView::Isometric),
    }
}

/// The pixel size one render is produced at: the viewport's own, scaled
/// to stay legible and to stay affordable, keeping its shape. A viewport
/// too small to have a shape yet falls back to a usable default.
pub fn render_size(viewport_size: (u32, u32)) -> (u32, u32) {
    let (width, height) = viewport_size;
    if width == 0 || height == 0 {
        return (MIN_SHORT_SIDE * 4 / 3, MIN_SHORT_SIDE);
    }
    let short = width.min(height) as f32;
    let long = width.max(height) as f32;
    let mut scale = 1.0f32;
    if short < MIN_SHORT_SIDE as f32 {
        scale = MIN_SHORT_SIDE as f32 / short;
    }
    if long * scale > MAX_LONG_SIDE as f32 {
        scale = MAX_LONG_SIDE as f32 / long;
    }
    (
        ((width as f32 * scale).round() as u32).max(1),
        ((height as f32 * scale).round() as u32).max(1),
    )
}

/// Encode a rendered image as a PNG the provider can carry.
fn encode_png(image: &RenderedImage) -> Result<Vec<u8>, png::EncodingError> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&image.rgba)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadmark_renderer::camera::Projection;

    fn unit_bounds() -> Bounds3 {
        Bounds3 {
            min: [-1.0, -1.0, -1.0],
            max: [1.0, 1.0, 1.0],
        }
    }

    /// Where a world point lands in normalised device coordinates through
    /// this camera: (-1, -1) is one corner of the image and (1, 1) the
    /// other, so a model in frame projects inside that square.
    fn project(camera: &Camera, aspect: f32, point: [f32; 3]) -> [f32; 2] {
        let view = camera.view_matrix();
        let proj = camera.projection_matrix(aspect);
        let mul = |m: [[f32; 4]; 4], v: [f32; 4]| -> [f32; 4] {
            std::array::from_fn(|row| (0..4).map(|k| m[k][row] * v[k]).sum())
        };
        let clip = mul(proj, mul(view, [point[0], point[1], point[2], 1.0]));
        [clip[0] / clip[3], clip[1] / clip[3]]
    }

    fn corners(bounds: Bounds3) -> Vec<[f32; 3]> {
        let mut points = Vec::new();
        for x in [bounds.min[0], bounds.max[0]] {
            for y in [bounds.min[1], bounds.max[1]] {
                for z in [bounds.min[2], bounds.max[2]] {
                    points.push([x, y, z]);
                }
            }
        }
        points
    }

    #[test]
    fn every_view_frames_the_whole_model_without_shrinking_it() {
        let aspect = 4.0 / 3.0;
        // A camera parked far from a model off to one side: whatever the
        // user was looking at, the render must not inherit it.
        let mut user = Camera::default();
        user.zoom(-8.0);
        user.pan(300.0, -200.0);

        for view in [
            RenderView::Current,
            RenderView::Front,
            RenderView::Back,
            RenderView::Left,
            RenderView::Right,
            RenderView::Top,
            RenderView::Bottom,
            RenderView::Isometric,
        ] {
            let camera = framed_camera(&user, view, unit_bounds(), aspect);
            let projected: Vec<[f32; 2]> = corners(unit_bounds())
                .into_iter()
                .map(|corner| project(&camera, aspect, corner))
                .collect();

            let extent = |axis: usize| {
                let min = projected
                    .iter()
                    .map(|p| p[axis])
                    .fold(f32::INFINITY, f32::min);
                let max = projected
                    .iter()
                    .map(|p| p[axis])
                    .fold(f32::NEG_INFINITY, f32::max);
                (min, max)
            };
            let (min_x, max_x) = extent(0);
            let (min_y, max_y) = extent(1);

            assert!(
                min_x >= -1.0 && max_x <= 1.0 && min_y >= -1.0 && max_y <= 1.0,
                "{view:?}: the model overflows the image (x {min_x}..{max_x}, y {min_y}..{max_y})"
            );
            // Filling the frame: the model spans most of the narrower
            // dimension rather than sitting in a corner.
            let span = (max_x - min_x).max(max_y - min_y);
            assert!(
                span > 0.8,
                "{view:?}: the model spans only {span} of the image"
            );
        }
    }

    #[test]
    fn a_named_view_ignores_where_the_user_was_looking() {
        let mut user = Camera::default();
        user.orbit(120.0, 40.0);
        let top = framed_camera(&user, RenderView::Top, unit_bounds(), 1.0);
        let current = framed_camera(&user, RenderView::Current, unit_bounds(), 1.0);

        // Top looks straight down; the current view keeps the user's own
        // orbit angles.
        assert!((top.pitch() - std::f32::consts::FRAC_PI_2).abs() < 1e-3);
        assert!((current.pitch() - user.pitch()).abs() < 1e-6);
        assert!((current.yaw() - user.yaw()).abs() < 1e-6);
    }

    #[test]
    fn the_users_projection_is_kept() {
        let mut user = Camera::default();
        user.set_projection(Projection::Orthographic);
        let camera = framed_camera(&user, RenderView::Front, unit_bounds(), 1.0);
        assert_eq!(camera.projection(), Projection::Orthographic);
    }

    #[test]
    fn a_render_is_the_viewports_size_unless_that_is_unusable() {
        // A normal viewport is rendered at its own resolution.
        assert_eq!(render_size((1200, 800)), (1200, 800));
        // A viewport too small to judge geometry from is scaled up,
        // keeping its shape.
        let (width, height) = render_size((160, 120));
        assert_eq!(height, MIN_SHORT_SIDE);
        // Scaled up keeping its 4:3 shape, to the nearest whole pixel.
        assert!((width as i64 - (MIN_SHORT_SIDE as i64 * 4 / 3)).abs() <= 1);
        // A very large viewport is scaled down to the affordable cap.
        let (width, height) = render_size((3840, 2160));
        assert_eq!(width, MAX_LONG_SIDE);
        assert!(height < MAX_LONG_SIDE);
        // A viewport with no size yet still gives a usable image.
        assert_eq!(
            render_size((0, 0)),
            (MIN_SHORT_SIDE * 4 / 3, MIN_SHORT_SIDE)
        );
    }

    #[test]
    fn a_rendered_image_encodes_as_a_readable_png() {
        let image = RenderedImage {
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        };
        let bytes = encode_png(&image).expect("encode");
        let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
        let mut reader = decoder.read_info().expect("PNG header");
        let mut out = vec![0; reader.output_buffer_size().expect("PNG buffer size")];
        let info = reader.next_frame(&mut out).expect("PNG data");
        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(&out[..info.buffer_size()], image.rgba.as_slice());
    }
}
