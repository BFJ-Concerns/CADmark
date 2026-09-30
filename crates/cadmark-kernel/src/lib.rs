// CADmark kernel — the build123d modelling kernel behind a process boundary.
//
// The application talks to `worker::KernelWorker`, which runs the
// `cadmark-kernel-worker` binary as a confined child process and speaks
// the JSON-line `protocol` to it. Everything else in this crate —
// execution, OCP instrumentation for provenance, tessellation,
// measurement, export — runs inside that child. No type crossing the
// boundary names build123d, OCP, or Python.

// Tests build every boundary-crossing type on its shared base with
// struct-update syntax, even when they name every field, so a field added
// later is filled in one place (crates/cadmark-core/tests/
// boundary_type_construction.rs holds them to it); clippy's complaint that
// such an update is redundant today is the point.
#![cfg_attr(test, allow(clippy::needless_update))]

mod bounds;
pub mod execution;
pub mod export;
pub mod measurement;
pub mod protocol;
pub mod provenance;
mod python_runtime;
pub mod sandbox;
pub mod sketch_lineage;
pub mod sketch_tessellation;
pub mod tessellation;
pub mod worker;
