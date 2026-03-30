// CADmark kernel — PyO3 bridge to build123d via OCP.
//
// Isolates all Python execution from UI and rendering threads.
// Handles script execution, OCP instrumentation for provenance,
// and tessellation extraction for the renderer.

pub mod execution;
pub mod provenance;
pub mod tessellation;
