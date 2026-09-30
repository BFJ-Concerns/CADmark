// CADmark core — domain types shared by every crate.
//
// Provenance ledger, geometry context, tessellated mesh, spatial comments,
// messages, and design-step metadata. No kernel, GPU, or UI dependencies.

// Tests build every boundary-crossing type on its shared base with
// struct-update syntax, even when they name every field, so a field added
// later is filled in one place (crates/cadmark-core/tests/
// boundary_type_construction.rs holds them to it); clippy's complaint that
// such an update is redundant today is the point.
#![cfg_attr(test, allow(clippy::needless_update))]

pub mod cancellation;
pub mod candidates;
pub mod context;
pub mod export;
pub mod geometry;
pub mod ledger;
pub mod limits;
pub mod mesh;
pub mod message;
pub mod model_session;
pub mod pending_comment;
pub mod sketch;
pub mod sketch_lineage;
pub mod skills;
pub mod version;
