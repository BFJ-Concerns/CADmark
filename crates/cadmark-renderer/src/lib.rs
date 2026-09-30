// CADmark renderer — wgpu pipeline for CAD visualisation.
//
// Shaded mesh with wireframe edge overlay, GPU colour-ID picking
// for faces/edges/vertices, selection glow, and hover highlight.

// Tests build every boundary-crossing type on its shared base with
// struct-update syntax, even when they name every field, so a field added
// later is filled in one place (crates/cadmark-core/tests/
// boundary_type_construction.rs holds them to it); clippy's complaint that
// such an update is redundant today is the point.
#![cfg_attr(test, allow(clippy::needless_update))]

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
