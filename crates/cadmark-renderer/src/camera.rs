// Camera — orbit, pan, zoom controls for CAD viewport navigation.
//
// Right-click drag to orbit, scroll to zoom, middle-click to pan.
// Standard CAD navigation conventions.

/// Camera state for the 3D viewport.
#[derive(Debug, Clone)]
pub struct Camera {
    /// Point the camera orbits around.
    pub target: [f32; 3],
    /// Distance from the target.
    pub distance: f32,
    /// Horizontal angle in radians (azimuth).
    pub yaw: f32,
    /// Vertical angle in radians (elevation), clamped to avoid gimbal lock.
    pub pitch: f32,
    /// Field of view in radians.
    pub fov: f32,
    /// Near clipping plane.
    pub near: f32,
    /// Far clipping plane.
    pub far: f32,
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
        }
    }
}

impl Camera {
    /// Compute the camera's eye position from orbit parameters.
    pub fn eye_position(&self) -> [f32; 3] {
        let cos_pitch = self.pitch.cos();
        [
            self.target[0] + self.distance * cos_pitch * self.yaw.sin(),
            self.target[1] + self.distance * self.pitch.sin(),
            self.target[2] + self.distance * cos_pitch * self.yaw.cos(),
        ]
    }

    /// Orbit the camera by a delta in screen-space pixels.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        let sensitivity = 0.005;
        self.yaw += dx * sensitivity;
        self.pitch = (self.pitch + dy * sensitivity).clamp(
            -std::f32::consts::FRAC_PI_2 + 0.01,
            std::f32::consts::FRAC_PI_2 - 0.01,
        );
    }

    /// Zoom by a scroll delta. Positive zooms in.
    pub fn zoom(&mut self, delta: f32) {
        let factor = 1.0 - delta * 0.1;
        self.distance = (self.distance * factor).clamp(0.1, 500.0);
    }

    /// Pan the camera target in the camera's local XY plane.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let sensitivity = 0.002 * self.distance;
        // Camera right vector (simplified — ignores roll).
        let right = [self.yaw.cos(), 0.0, -self.yaw.sin()];
        // Camera up is world Y in this simplified model.
        let up = [0.0, 1.0, 0.0];

        for i in 0..3 {
            self.target[i] -= right[i] * dx * sensitivity;
            self.target[i] += up[i] * dy * sensitivity;
        }
    }

    /// Build a 4x4 view matrix (column-major) for the shader.
    pub fn view_matrix(&self) -> [[f32; 4]; 4] {
        let eye = self.eye_position();
        look_at(eye, self.target, [0.0, 1.0, 0.0])
    }

    /// Build a 4x4 perspective projection matrix (column-major).
    pub fn projection_matrix(&self, aspect_ratio: f32) -> [[f32; 4]; 4] {
        perspective(self.fov, aspect_ratio, self.near, self.far)
    }
}

/// Simple look-at matrix. Column-major layout for wgpu.
fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> [[f32; 4]; 4] {
    let f = normalize(sub(target, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);

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
