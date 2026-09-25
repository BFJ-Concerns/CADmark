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

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use cadmark_bridge::backend::ImageData;
use cadmark_bridge::tools::RenderView;
use cadmark_core::geometry::PartId;
use cadmark_core::mesh::TessellatedMesh;
use cadmark_core::sketch::SketchProfile;
use cadmark_kernel::protocol::{ExecutedModel, ExecutedPart, ModelForm};
use cadmark_renderer::camera::{Bounds3, Camera, StandardView};
use cadmark_renderer::offscreen::{OffscreenPart, OffscreenRenderer, RenderedImage};
use cadmark_renderer::pipeline::Renderer;

use crate::turn::RenderSource;

/// The size a render falls back to before the viewport has one — the
/// window has not been laid out yet, so there is no resolution to match.
const FALLBACK_SIZE: (u32, u32) = (1024, 768);

/// The background the AI's render is drawn on. Mid-grey reads against
/// both lit and shadowed faces; the offscreen target is not an sRGB
/// format, so this is the stored value directly.
const RENDER_CLEAR: wgpu::Color = wgpu::Color {
    r: 0.14,
    g: 0.15,
    b: 0.17,
    a: 1.0,
};

/// One part of the model as the worker thread needs it: named, so the AI
/// can ask for it alone; numbered, so it is drawn in the colour the
/// viewport gives it; with its bounds, so a render can frame it.
#[derive(Clone, Debug)]
pub struct ScenePart {
    pub id: u32,
    pub name: String,
    /// Shared rather than copied: a turn's render must not depend on the
    /// mesh still being current when it runs.
    pub mesh: Arc<TessellatedMesh>,
    pub bounds: Option<Bounds3>,
}

impl ScenePart {
    /// A part as the kernel produced it, its bounds taken from its mesh.
    pub fn from_executed(part: &ExecutedPart) -> Self {
        let bounds =
            Bounds3::from_positions(part.mesh.vertices.iter().map(|vertex| vertex.position));
        Self {
            id: part.id,
            name: part.name.clone(),
            mesh: Arc::new(part.mesh.clone()),
            bounds,
        }
    }
}

/// What the UI thread last showed, as the worker thread needs it.
#[derive(Clone, Default)]
struct Scene {
    /// The parts on screen, in script order.
    parts: Vec<ScenePart>,
    /// The parts the user has hidden from the Parts tab, by name — the
    /// identity the tab keeps them by across rebuilds.
    hidden: HashSet<String>,
    /// The sketch profile on screen, when the design has reached only a
    /// sketch. Published alongside the parts so a render of a sketch-only
    /// design shows the profile rather than an empty frame.
    sketch: Option<Arc<SketchProfile>>,
    sketch_bounds: Option<Bounds3>,
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

    /// Publish the parts now on screen; an empty list is a model gone.
    pub fn set_parts(&self, parts: Vec<ScenePart>) {
        self.lock().parts = parts;
    }

    /// Publish which parts the user has hidden, by name.
    pub fn set_hidden(&self, hidden: HashSet<String>) {
        self.lock().hidden = hidden;
    }

    /// Publish the sketch profile now on screen, or its absence.
    pub fn set_sketch(&self, sketch: Option<Arc<SketchProfile>>) {
        let mut scene = self.lock();
        scene.sketch_bounds = sketch
            .as_ref()
            .and_then(|sketch| Bounds3::from_positions(sketch.points()));
        scene.sketch = sketch;
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
    fn model_built(&mut self, model: &ExecutedModel) {
        // The same published shape the UI thread produces, so a render in
        // the same response as the execution sees what the user is about
        // to; the hidden set stays as the user last set it.
        match &model.form {
            ModelForm::Solid(solid) => {
                self.scene.set_sketch(None);
                self.scene
                    .set_parts(solid.parts.iter().map(ScenePart::from_executed).collect());
            }
            ModelForm::Sketch(sketch) => {
                // The solid already on screen stays published: a sketch
                // draws in front of it, ghosted, rather than replacing it.
                self.scene
                    .set_sketch(Some(Arc::new(sketch.profile.clone())));
            }
        }
    }

    fn render(&mut self, view: RenderView, part: Option<&str>) -> Result<ImageData, String> {
        let scene = self.scene.snapshot();
        let parts = parts_to_draw(&scene.parts, &scene.hidden, part)?;
        // A named part is framed alone; otherwise the profile when the
        // design is a sketch, else everything drawn.
        let bounds = match (part, scene.sketch_bounds) {
            (None, Some(sketch_bounds)) => Some(sketch_bounds),
            _ => union_bounds(parts.iter().filter_map(|part| part.bounds)),
        };
        let Some(bounds) = bounds else {
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
        // A profile in front of a ghosted solid is what the viewport
        // shows, so it is what the model is shown too.
        state.ghost_solid = scene.sketch.is_some();
        // The offscreen target is not an sRGB format, so the shader
        // display-encodes its own output.
        state.target_is_srgb = false;

        let drawn: Vec<OffscreenPart<'_>> = parts
            .iter()
            .map(|part| OffscreenPart {
                id: PartId(part.id),
                mesh: &part.mesh,
            })
            .collect();
        let image = offscreen
            .render(
                &self.gpu.device,
                &self.gpu.queue,
                &drawn,
                scene.sketch.as_deref(),
                &state.mesh_uniforms((width, height)),
                RENDER_CLEAR,
            )
            .map_err(|error| error.to_string())?;

        Ok(ImageData {
            media_type: "image/png".to_string(),
            bytes: encode_png(&image).map_err(|error| error.to_string())?,
        })
    }
}

/// Which parts one render draws. Named, the one part by that name whether
/// or not the user has it hidden — asked for by name, it is wanted.
/// Unnamed, every part the user has visible, as the viewport shows them.
/// A name that matches nothing is answered with the names that exist, so
/// the model's next call can be right.
pub fn parts_to_draw<'a>(
    parts: &'a [ScenePart],
    hidden: &HashSet<String>,
    named: Option<&str>,
) -> Result<Vec<&'a ScenePart>, String> {
    let list = || {
        parts
            .iter()
            .map(|part| format!("`{}`", part.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match named.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => {
            let exact = parts.iter().find(|part| part.name == name);
            let part = exact.or_else(|| {
                // A case slip is answered with the part meant, when it is
                // unambiguous which.
                let mut loosely = parts
                    .iter()
                    .filter(|part| part.name.eq_ignore_ascii_case(name));
                loosely.next().filter(|_| loosely.next().is_none())
            });
            match part {
                Some(part) => Ok(vec![part]),
                None if parts.is_empty() => Err(format!(
                    "there is no part named `{name}`: the model has no parts on screen"
                )),
                None => Err(format!(
                    "there is no part named `{name}`; the parts are {}",
                    list()
                )),
            }
        }
        None => {
            let visible: Vec<&ScenePart> = parts
                .iter()
                .filter(|part| !hidden.contains(&part.name))
                .collect();
            if visible.is_empty() && !parts.is_empty() {
                return Err(format!(
                    "every part is hidden in the Parts tab; name one to render it alone \
                     (the parts are {})",
                    list()
                ));
            }
            Ok(visible)
        }
    }
}

/// The box around several boxes, or `None` for none.
fn union_bounds(bounds: impl IntoIterator<Item = Bounds3>) -> Option<Bounds3> {
    bounds.into_iter().reduce(|whole, next| Bounds3 {
        min: std::array::from_fn(|axis| whole.min[axis].min(next.min[axis])),
        max: std::array::from_fn(|axis| whole.max[axis].max(next.max[axis])),
    })
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

/// The pixel size one render is produced at: the viewport's own, so the
/// AI sees the model at the resolution the user is judging it at. A
/// viewport with no size yet falls back to a usable default.
pub fn render_size(viewport_size: (u32, u32)) -> (u32, u32) {
    let (width, height) = viewport_size;
    if width == 0 || height == 0 {
        return FALLBACK_SIZE;
    }
    (width, height)
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
        // Whatever the viewport's resolution, the render carries it —
        // small, large, and unusual shapes alike.
        assert_eq!(render_size((1200, 800)), (1200, 800));
        assert_eq!(render_size((160, 120)), (160, 120));
        assert_eq!(render_size((3840, 2160)), (3840, 2160));
        assert_eq!(render_size((900, 1600)), (900, 1600));
        // A viewport with no size yet still gives a usable image.
        assert_eq!(render_size((0, 0)), FALLBACK_SIZE);
        assert_eq!(render_size((800, 0)), FALLBACK_SIZE);
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

    fn part(name: &str) -> ScenePart {
        ScenePart {
            id: 0,
            name: name.to_string(),
            mesh: Arc::new(TessellatedMesh::default()),
            bounds: Some(unit_bounds()),
        }
    }

    fn names(parts: &[&ScenePart]) -> Vec<String> {
        parts.iter().map(|part| part.name.clone()).collect()
    }

    #[test]
    fn an_unnamed_render_draws_what_the_user_has_visible() {
        let parts = [part("base"), part("lid"), part("peg")];
        let hidden = HashSet::from(["lid".to_string()]);
        let drawn = parts_to_draw(&parts, &hidden, None).expect("drawable");
        assert_eq!(names(&drawn), ["base", "peg"]);
        // Nothing hidden: everything, in script order.
        let all = parts_to_draw(&parts, &HashSet::new(), None).expect("drawable");
        assert_eq!(names(&all), ["base", "lid", "peg"]);
    }

    #[test]
    fn a_named_part_is_drawn_alone_even_when_the_user_hid_it() {
        let parts = [part("base"), part("lid")];
        let hidden = HashSet::from(["lid".to_string()]);
        let drawn = parts_to_draw(&parts, &hidden, Some("lid")).expect("drawable");
        assert_eq!(names(&drawn), ["lid"]);
        // A case slip still finds the one part meant.
        let drawn = parts_to_draw(&parts, &hidden, Some("Lid")).expect("drawable");
        assert_eq!(names(&drawn), ["lid"]);
    }

    #[test]
    fn a_name_that_matches_nothing_is_answered_with_the_names_that_exist() {
        let parts = [part("base"), part("lid")];
        let error = parts_to_draw(&parts, &HashSet::new(), Some("cap")).unwrap_err();
        assert!(error.contains("`cap`"), "{error}");
        assert!(
            error.contains("`base`") && error.contains("`lid`"),
            "{error}"
        );
        // Two parts differing only in case: a loose match is ambiguous,
        // so it is refused rather than guessed.
        let parts = [part("Lid"), part("lid")];
        assert!(parts_to_draw(&parts, &HashSet::new(), Some("LID")).is_err());
    }

    #[test]
    fn a_model_the_user_has_entirely_hidden_says_so_rather_than_drawing_nothing() {
        let parts = [part("base")];
        let hidden = HashSet::from(["base".to_string()]);
        let error = parts_to_draw(&parts, &hidden, None).unwrap_err();
        assert!(
            error.contains("hidden") && error.contains("`base`"),
            "{error}"
        );
        // No parts at all is not an error here: a sketch may still be on
        // screen, and the render decides from its bounds.
        assert!(parts_to_draw(&[], &hidden, None).expect("empty").is_empty());
    }

    #[test]
    fn framing_spans_every_drawn_part() {
        let near = Bounds3 {
            min: [-1.0, -1.0, -1.0],
            max: [1.0, 1.0, 1.0],
        };
        let far = Bounds3 {
            min: [50.0, -2.0, 0.0],
            max: [52.0, 2.0, 3.0],
        };
        let whole = union_bounds([near, far]).expect("two boxes");
        assert_eq!(whole.min, [-1.0, -2.0, -1.0]);
        assert_eq!(whole.max, [52.0, 2.0, 3.0]);
        assert!(union_bounds([]).is_none());
    }
}
