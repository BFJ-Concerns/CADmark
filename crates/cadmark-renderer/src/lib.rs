// CADmark renderer — wgpu pipeline for CAD visualisation.
//
// Shaded mesh with wireframe edge overlay, GPU colour-ID picking
// for faces/edges/vertices, selection glow, and hover highlight.

pub mod camera;
pub mod markers;
pub mod mesh;
pub mod offscreen;
pub mod picking;
pub mod pipeline;
pub mod section;
#[cfg(any(test, feature = "test-device"))]
pub mod test_device;
pub mod viewport;
