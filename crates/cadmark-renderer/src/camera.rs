// Camera — orbit, pan, zoom controls for CAD viewport navigation.
//
// Right-click drag to orbit, scroll to zoom, middle-click to pan.
// Standard CAD navigation conventions. The world is Z-up, as build123d
// models are: yaw 0 and pitch 0 is the front view, looking along +Y with X
// to the right and Z up.
//
// The camera's fields are private so every route in keeps the invariants:
// pitch stays inside the poles, distance stays inside its range, and the
// clip planes bracket the model. Reads go through the accessors.

/// World up. build123d builds Z-up, so the viewport does too.
const WORLD_UP: [f32; 3] = [0.0, 0.0, 1.0];

/// Pitch stops this far short of straight up or down when orbiting, so the
/// screen's up direction never flips mid-drag.
const PITCH_MARGIN: f32 = 0.01;
const MIN_DISTANCE: f32 = 0.1;
const MAX_DISTANCE: f32 = 500.0;

/// Perspective or orthographic projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    Perspective,
    Orthographic,
}

/// The standard views a CAD viewport snaps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardView {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Isometric,
}

impl StandardView {
    /// The world-space direction the eye looks from.
    pub fn direction(self) -> [f32; 3] {
        match self {
            Self::Front => [0.0, -1.0, 0.0],
            Self::Back => [0.0, 1.0, 0.0],
            Self::Left => [-1.0, 0.0, 0.0],
            Self::Right => [1.0, 0.0, 0.0],
            Self::Top => [0.0, 0.0, 1.0],
            Self::Bottom => [0.0, 0.0, -1.0],
            Self::Isometric => normalize([1.0, -1.0, 1.0]),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Front => "Front",
            Self::Back => "Back",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::Isometric => "Isometric",
        }
    }

    pub const ALL: [StandardView; 7] = [
        Self::Front,
        Self::Back,
        Self::Left,
        Self::Right,
        Self::Top,
        Self::Bottom,
        Self::Isometric,
    ];
}

/// Camera state for the 3D viewport.
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    /// Point the camera orbits around.
    target: [f32; 3],
    /// Distance from the target.
    distance: f32,
    /// Horizontal angle in radians. Zero looks from in front (-Y); positive
    /// turns the eye anticlockwise seen from above.
    yaw: f32,
    /// Elevation in radians. Orbiting stops just short of the poles; the
    /// axis views reach them exactly.
    pitch: f32,
    /// Field of view in radians (perspective), and the angle that sizes
    /// the orthographic frustum so switching projection keeps the model
    /// the same size on screen.
    fov: f32,
    /// Near clipping plane.
    near: f32,
    /// Far clipping plane.
    far: f32,
    projection: Projection,
}

/// Directions towards the two lights of the viewport's studio rig, in world
/// space and unit length. The rig is fixed to the camera, so the model is lit
/// the same way from every viewing angle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightRig {
    /// Main light: above and to the left of the eye, slightly in front.
    pub key: [f32; 3],
    /// Soft secondary light: low and to the right, opposing the key.
    pub fill: [f32; 3],
}

/// The camera's orthonormal axes in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Basis {
    pub right: [f32; 3],
    pub up: [f32; 3],
    /// From the eye towards the target.
    pub forward: [f32; 3],
}

/// Axis-aligned bounds used to frame newly loaded geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds3 {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds3 {
    pub fn from_positions(positions: impl IntoIterator<Item = [f32; 3]>) -> Option<Self> {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut found = false;

        for position in positions {
            if !position.iter().all(|coordinate| coordinate.is_finite()) {
                return None;
            }
            found = true;
            for axis in 0..3 {
                min[axis] = min[axis].min(position[axis]);
                max[axis] = max[axis].max(position[axis]);
            }
        }

        found.then_some(Self { min, max })
    }

    fn centre(self) -> [f32; 3] {
        std::array::from_fn(|axis| (self.min[axis] + self.max[axis]) * 0.5)
    }

    fn radius(self) -> f32 {
        let half_extent: [f32; 3] =
            std::array::from_fn(|axis| (self.max[axis] - self.min[axis]) * 0.5);
        dot(half_extent, half_extent).sqrt()
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            target: [0.0, 0.0, 0.0],
            distance: 5.0,
            yaw: std::f32::consts::FRAC_PI_4,
            pitch: std::f32::consts::FRAC_PI_6,
            fov: std::f32::consts::FRAC_PI_4,
            near: 0.01,
            far: 1000.0,
            projection: Projection::Perspective,
        }
    }
}

impl Camera {
    pub fn target(&self) -> [f32; 3] {
        self.target
    }

    pub fn distance(&self) -> f32 {
        self.distance
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    pub fn projection(&self) -> Projection {
        self.projection
    }

    pub fn set_projection(&mut self, projection: Projection) {
        self.projection = projection;
    }

    /// Snap to a standard view, keeping the target and distance.
    pub fn look_at_standard(&mut self, view: StandardView) {
        self.look_from(view.direction());
    }

    /// Centre and distance the camera so the complete bounding sphere is
    /// visible in the narrower viewport dimension.
    pub fn frame_bounds(&mut self, bounds: Bounds3, aspect_ratio: f32) {
        let radius = bounds.radius().max(0.001);
        let vertical_half_angle = self.fov * 0.5;
        let horizontal_half_angle = (vertical_half_angle.tan() * aspect_ratio.max(0.01)).atan();
        let limiting_half_angle = vertical_half_angle.min(horizontal_half_angle);

        self.target = bounds.centre();
        self.distance =
            (radius / limiting_half_angle.sin() * 1.15).clamp(MIN_DISTANCE, MAX_DISTANCE);
        self.near = (radius * 0.001).max(0.0001);
        self.far = (self.distance + radius * 3.0).max(self.near + 1.0);
    }

    /// Compute the camera's eye position from orbit parameters.
    pub fn eye_position(&self) -> [f32; 3] {
        let cos_pitch = self.pitch.cos();
        [
            self.target[0] + self.distance * cos_pitch * self.yaw.sin(),
            self.target[1] - self.distance * cos_pitch * self.yaw.cos(),
            self.target[2] + self.distance * self.pitch.sin(),
        ]
    }

    /// Turn the camera to look at the target from the given world-space
    /// direction, keeping the target and distance. A vertical direction
    /// gives the top or bottom view with +Y up the screen.
    pub fn look_from(&mut self, direction: [f32; 3]) {
        let direction = normalize(direction);
        self.pitch = direction[2].clamp(-1.0, 1.0).asin();
        let horizontal = (direction[0] * direction[0] + direction[1] * direction[1]).sqrt();
        self.yaw = if horizontal < 1e-4 {
            0.0
        } else {
            direction[0].atan2(-direction[1])
        };
    }

    /// Look squarely at a plane in orthographic projection, so a profile
    /// drawn on it reads at true shape rather than in perspective. The
    /// side faced is whichever of the plane's two the camera is already
    /// nearer, so framing a sketch does not turn the view inside out.
    pub fn view_plane_face_on(&mut self, normal: [f32; 3]) {
        let normal = normalize(normal);
        let towards_eye = sub(self.eye_position(), self.target);
        let direction = if dot(normal, towards_eye) < 0.0 {
            normal.map(|component| -component)
        } else {
            normal
        };
        self.look_from(direction);
        self.projection = Projection::Orthographic;
    }

    /// The camera's axes in world space. Looking straight down or up, where
    /// world up is no guide, the screen's up is the direction the eye would
    /// face from the same yaw at the horizon, so the top view has +Y up.
    pub fn basis(&self) -> Basis {
        let forward = normalize(sub(self.target, self.eye_position()));
        let up_reference = if dot(forward, WORLD_UP).abs() > 0.9999 {
            [-self.yaw.sin(), self.yaw.cos(), 0.0]
        } else {
            WORLD_UP
        };
        let right = normalize(cross(forward, up_reference));
        let up = cross(right, forward);
        Basis { right, up, forward }
    }

    /// Orbit the camera by a delta in screen-space pixels. The model
    /// follows the pointer: dragging right turns its near side to the
    /// right, dragging down tips its top towards the viewer.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        let sensitivity = 0.005;
        self.yaw -= dx * sensitivity;
        self.pitch = (self.pitch + dy * sensitivity).clamp(
            -std::f32::consts::FRAC_PI_2 + PITCH_MARGIN,
            std::f32::consts::FRAC_PI_2 - PITCH_MARGIN,
        );
    }

    /// Zoom by a scroll delta. Positive zooms in.
    pub fn zoom(&mut self, delta: f32) {
        let factor = 1.0 - delta * 0.1;
        self.distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Pan the camera target in the camera's own screen plane.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let sensitivity = 0.002 * self.distance;
        let Basis { right, up, .. } = self.basis();

        for i in 0..3 {
            self.target[i] -= right[i] * dx * sensitivity;
            self.target[i] += up[i] * dy * sensitivity;
        }
    }

    /// Light directions for the current view, expressed in world space.
    pub fn light_rig(&self) -> LightRig {
        let Basis { right, up, forward } = self.basis();
        let towards_eye = [-forward[0], -forward[1], -forward[2]];

        let blend = |r: f32, u: f32, e: f32| {
            normalize(std::array::from_fn(|axis| {
                right[axis] * r + up[axis] * u + towards_eye[axis] * e
            }))
        };

        LightRig {
            key: blend(-0.55, 0.75, 0.6),
            fill: blend(0.8, -0.25, 0.35),
        }
    }

    /// Build a 4x4 view matrix (column-major) for the shader. Column `i`
    /// is the view-space image of world axis `i`: its x and y are the
    /// axis's screen direction and its z points towards the viewer.
    pub fn view_matrix(&self) -> [[f32; 4]; 4] {
        view_from_basis(self.eye_position(), self.basis())
    }

    /// Build a 4x4 projection matrix (column-major) for the current
    /// projection. The orthographic frustum is sized to what the
    /// perspective view shows at the target's distance, so toggling
    /// projection leaves the model the same size on screen.
    pub fn projection_matrix(&self, aspect_ratio: f32) -> [[f32; 4]; 4] {
        match self.projection {
            Projection::Perspective => perspective(self.fov, aspect_ratio, self.near, self.far),
            Projection::Orthographic => {
                let half_height = (self.fov * 0.5).tan() * self.distance;
                orthographic(half_height, aspect_ratio, self.near, self.far)
            }
        }
    }
}

/// View matrix from an eye position and orthonormal camera axes.
/// Column-major layout for wgpu.
fn view_from_basis(eye: [f32; 3], basis: Basis) -> [[f32; 4]; 4] {
    let Basis {
        right: s,
        up: u,
        forward: f,
    } = basis;

    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ]
}

/// Perspective projection matrix. Column-major, wgpu clip space (Z: 0..1).
fn perspective(fov: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fov / 2.0).tan();
    let range_inv = 1.0 / (near - far);

    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far * range_inv, -1.0],
        [0.0, 0.0, near * far * range_inv, 0.0],
    ]
}

/// Orthographic projection matrix. Column-major, wgpu clip space (Z: 0..1).
fn orthographic(half_height: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let half_width = half_height * aspect;
    let range_inv = 1.0 / (near - far);
    [
        [1.0 / half_width, 0.0, 0.0, 0.0],
        [0.0, 1.0 / half_height, 0.0, 0.0],
        [0.0, 0.0, range_inv, 0.0],
        [0.0, 0.0, near * range_inv, 1.0],
    ]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    if len < 1e-10 {
        return [0.0, 0.0, 0.0];
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Approximate f32 equality for floating-point comparisons.
    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn approx_eq_vec(a: [f32; 3], b: [f32; 3]) -> bool {
        approx_eq(a[0], b[0]) && approx_eq(a[1], b[1]) && approx_eq(a[2], b[2])
    }

    /// A camera at `distance` from `target`, oriented by yaw and pitch.
    fn camera(target: [f32; 3], distance: f32, yaw: f32, pitch: f32) -> Camera {
        Camera {
            target,
            distance,
            yaw,
            pitch,
            ..Camera::default()
        }
    }

    #[test]
    fn eye_position_at_origin_target() {
        let cam = camera([0.0, 0.0, 0.0], 10.0, 0.0, 0.0);
        let eye = cam.eye_position();
        // yaw=0, pitch=0: the front view, eye along -Y at distance 10.
        assert!(approx_eq_vec(eye, [0.0, -10.0, 0.0]));
    }

    #[test]
    fn eye_position_with_pitch() {
        let cam = camera([0.0, 0.0, 0.0], 10.0, 0.0, std::f32::consts::FRAC_PI_4);
        let eye = cam.eye_position();
        // pitch=45deg: Z should be ~7.07, distance from origin should be ~10.
        let dist = dot(eye, eye).sqrt();
        assert!(approx_eq(dist, 10.0));
        assert!(eye[2] > 0.0); // Elevated above target.
    }

    #[test]
    fn orthographic_keeps_the_target_plane_the_same_size_as_perspective() {
        let mut cam = camera([0.0; 3], 10.0, 0.0, 0.0);
        let point = [2.0, 0.0, 1.0, 1.0];
        let project = |cam: &Camera| {
            let view = view_from_basis(cam.eye_position(), cam.basis());
            let proj = cam.projection_matrix(1.0);
            let v: [f32; 4] =
                std::array::from_fn(|row| (0..4).map(|k| view[k][row] * point[k]).sum());
            let c: [f32; 4] = std::array::from_fn(|row| (0..4).map(|k| proj[k][row] * v[k]).sum());
            [c[0] / c[3], c[1] / c[3]]
        };
        let perspective = project(&cam);
        cam.set_projection(Projection::Orthographic);
        assert_eq!(cam.projection(), Projection::Orthographic);
        let orthographic = project(&cam);
        assert!(
            approx_eq(perspective[0], orthographic[0]),
            "{perspective:?} vs {orthographic:?}"
        );
        assert!(approx_eq(perspective[1], orthographic[1]));
    }

    #[test]
    fn standard_views_look_from_their_directions() {
        let mut cam = camera([1.0, 2.0, 3.0], 10.0, 0.0, 0.0);
        for view in StandardView::ALL {
            cam.look_at_standard(view);
            let direction = view.direction();
            let expected: [f32; 3] =
                std::array::from_fn(|axis| cam.target()[axis] + direction[axis] * 10.0);
            assert!(approx_eq_vec(cam.eye_position(), expected), "{view:?}");
        }
        let iso = StandardView::Isometric.direction();
        assert!(approx_eq(dot(iso, iso), 1.0));
    }

    #[test]
    fn light_rig_follows_the_camera() {
        let mut cam = Camera::default();
        let before = cam.light_rig();
        // The key light sits above the eye line and the fill below it.
        assert!(before.key[2] > 0.0);
        assert!(before.fill[2] < before.key[2]);
        assert!(approx_eq(dot(before.key, before.key), 1.0));
        assert!(approx_eq(dot(before.fill, before.fill), 1.0));

        // Both lights face the viewer's side of the model.
        let towards_eye = normalize(sub(cam.eye_position(), cam.target()));
        assert!(dot(before.key, towards_eye) > 0.0);
        assert!(dot(before.fill, towards_eye) > 0.0);

        // Orbiting half a turn swings the rig around with the camera.
        cam.orbit(std::f32::consts::PI / 0.005, 0.0);
        let after = cam.light_rig();
        assert!(approx_eq(after.key[0], -before.key[0]));
        assert!(approx_eq(after.key[1], -before.key[1]));
        assert!(approx_eq(after.key[2], before.key[2]));
    }

    #[test]
    fn orbit_clamps_pitch() {
        let mut cam = Camera::default();
        // Orbit far enough to hit the clamp.
        cam.orbit(0.0, 100_000.0);
        assert!(cam.pitch() < std::f32::consts::FRAC_PI_2);
        assert!(cam.pitch() > std::f32::consts::FRAC_PI_2 - 0.02);

        cam.orbit(0.0, -200_000.0);
        assert!(cam.pitch() > -std::f32::consts::FRAC_PI_2);
        assert!(cam.pitch() < -std::f32::consts::FRAC_PI_2 + 0.02);
    }

    #[test]
    fn zoom_clamps_distance() {
        let mut cam = Camera::default();
        // Zoom in aggressively.
        for _ in 0..1000 {
            cam.zoom(100.0);
        }
        assert!(cam.distance() >= MIN_DISTANCE);

        // Zoom out aggressively.
        for _ in 0..1000 {
            cam.zoom(-100.0);
        }
        assert!(cam.distance() <= MAX_DISTANCE);
    }

    #[test]
    fn pan_shifts_target() {
        let mut cam = camera([0.0; 3], 5.0, 0.0, std::f32::consts::FRAC_PI_6);
        let original_target = cam.target();
        cam.pan(100.0, 0.0);
        // Panning horizontally with yaw=0 should shift target along X.
        assert!((cam.target()[0] - original_target[0]).abs() > 0.01);
    }

    #[test]
    fn axis_views_look_from_each_direction() {
        let mut cam = camera([1.0, 2.0, 3.0], 10.0, 0.0, 0.0);
        for direction in [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ] {
            cam.look_from(direction);
            let expected: [f32; 3] =
                std::array::from_fn(|axis| cam.target()[axis] + direction[axis] * 10.0);
            assert!(
                approx_eq_vec(cam.eye_position(), expected),
                "looking from {direction:?} put the eye at {:?}",
                cam.eye_position()
            );
        }
    }

    #[test]
    fn front_view_has_x_right_and_z_up() {
        let mut cam = Camera::default();
        cam.look_from([0.0, -1.0, 0.0]);
        let basis = cam.basis();
        assert!(approx_eq_vec(basis.right, [1.0, 0.0, 0.0]));
        assert!(approx_eq_vec(basis.up, [0.0, 0.0, 1.0]));
        assert!(approx_eq_vec(basis.forward, [0.0, 1.0, 0.0]));
    }

    #[test]
    fn top_view_has_x_right_and_y_up() {
        let mut cam = Camera::default();
        cam.look_from([0.0, 0.0, 1.0]);
        let basis = cam.basis();
        assert!(approx_eq_vec(basis.right, [1.0, 0.0, 0.0]));
        assert!(approx_eq_vec(basis.up, [0.0, 1.0, 0.0]));
        assert!(approx_eq_vec(basis.forward, [0.0, 0.0, -1.0]));
    }

    #[test]
    fn view_matrix_columns_are_world_axes_on_screen() {
        let mut cam = Camera::default();
        cam.look_from([0.0, -1.0, 0.0]);
        let m = cam.view_matrix();
        // In the front view, world X runs right across the screen, world Z
        // runs up it, and world Y points away from the viewer.
        assert!(approx_eq_vec([m[0][0], m[0][1], m[0][2]], [1.0, 0.0, 0.0]));
        assert!(approx_eq_vec([m[2][0], m[2][1], m[2][2]], [0.0, 1.0, 0.0]));
        assert!(approx_eq_vec([m[1][0], m[1][1], m[1][2]], [0.0, 0.0, -1.0]));
    }

    #[test]
    fn view_matrix_is_invertible() {
        let cam = Camera::default();
        let m = cam.view_matrix();
        // A valid view matrix should have a non-zero determinant.
        // Quick check: the last column should be [0, 0, 0, 1] pattern
        // for a standard affine transform.
        assert!(approx_eq(m[0][3], 0.0));
        assert!(approx_eq(m[1][3], 0.0));
        assert!(approx_eq(m[2][3], 0.0));
        assert!(approx_eq(m[3][3], 1.0));
    }

    #[test]
    fn projection_matrix_near_plane() {
        let cam = Camera::default();
        let proj = cam.projection_matrix(1.0);
        // wgpu clip space: Z maps to 0..1. At the near plane, Z should map to 0.
        // The [2][2] and [3][2] elements encode the depth mapping.
        // For a valid perspective matrix, [3][3] should be 0 (perspective divide).
        assert!(approx_eq(proj[3][3], 0.0));
    }

    #[test]
    fn helper_normalize_unit_vector() {
        let v = normalize([3.0, 4.0, 0.0]);
        let len = dot(v, v).sqrt();
        assert!(approx_eq(len, 1.0));
        assert!(approx_eq(v[0], 0.6));
        assert!(approx_eq(v[1], 0.8));
    }

    #[test]
    fn helper_normalize_zero_vector() {
        let v = normalize([0.0, 0.0, 0.0]);
        assert!(approx_eq_vec(v, [0.0, 0.0, 0.0]));
    }

    #[test]
    fn helper_cross_product() {
        let result = cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(approx_eq_vec(result, [0.0, 0.0, 1.0]));
    }

    #[test]
    fn helper_dot_product() {
        assert!(approx_eq(dot([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]), 32.0));
    }

    #[test]
    fn bounds_reject_non_finite_positions() {
        assert_eq!(
            Bounds3::from_positions([[0.0, 0.0, 0.0], [f32::NAN, 1.0, 1.0]]),
            None
        );
    }

    #[test]
    fn framing_centres_and_places_the_verifier_box_outside_the_model() {
        let bounds = Bounds3::from_positions([[-10.0, -7.5, -5.0], [10.0, 7.5, 5.0]])
            .expect("finite non-empty bounds");
        let mut camera = Camera::default();
        let initial_eye = camera.eye_position();
        assert!(
            initial_eye
                .iter()
                .enumerate()
                .all(|(axis, coordinate)| *coordinate >= bounds.min[axis]
                    && *coordinate <= bounds.max[axis])
        );

        camera.frame_bounds(bounds, 0.68);

        assert_eq!(camera.target(), [0.0, 0.0, 0.0]);
        assert!(camera.distance() > bounds.radius());
        assert!(camera.near > 0.0);
        assert!(camera.far > camera.distance() + bounds.radius());

        let eye = camera.eye_position();
        assert!(
            eye.iter()
                .enumerate()
                .any(|(axis, coordinate)| *coordinate < bounds.min[axis]
                    || *coordinate > bounds.max[axis])
        );
    }

    #[test]
    fn facing_a_plane_looks_squarely_at_it_in_orthographic() {
        let mut camera = Camera::default();
        camera.view_plane_face_on([0.0, 1.0, 0.0]);

        assert_eq!(camera.projection(), Projection::Orthographic);
        // Looking along the plane's normal: the eye sits on the normal
        // from the target, so the profile is seen at true shape.
        let eye = camera.eye_position();
        assert!(eye[0].abs() < 1e-4, "{eye:?}");
        assert!(eye[2].abs() < 1e-4, "{eye:?}");
        assert!(eye[1].abs() > 1.0, "{eye:?}");
    }

    #[test]
    fn facing_a_plane_keeps_the_side_the_camera_is_already_on() {
        let mut camera = Camera::default();
        camera.look_from([-1.0, 0.0, 0.0]);
        let before = camera.eye_position();
        // The plane's stated normal points the other way; the view must
        // not flip to the far side of the sketch.
        camera.view_plane_face_on([1.0, 0.0, 0.0]);
        assert!(
            camera.eye_position()[0].signum() == before[0].signum(),
            "the view flipped to the other side of the plane"
        );
    }
}
