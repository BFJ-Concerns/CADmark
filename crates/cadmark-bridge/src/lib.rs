// CADmark bridge — AI backend abstraction and Claude Code implementation.
//
// Defines the `AiBackend` trait for structured AI interaction.
// Era 0 implements this via Claude Code subprocess; Era 0.1 adds
// direct API implementations for multiple providers.

pub mod backend;
pub mod claude_code;
pub mod context;
pub mod doc_lookup;
