// CADmark core — domain types shared by every crate.
//
// Provenance ledger, geometry context, tessellated mesh, spatial comments,
// messages, and design-step metadata. No kernel, GPU, or UI dependencies.

pub mod cancellation;
pub mod context;
pub mod export;
pub mod geometry;
pub mod ledger;
pub mod limits;
pub mod mesh;
pub mod message;
pub mod pending_comment;
pub mod version;
