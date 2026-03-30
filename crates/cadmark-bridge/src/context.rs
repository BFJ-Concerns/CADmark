// AI request context — the structured input sent to the backend.
//
// Packages geometry context, current code, and conversation history
// into a stable format regardless of which identification strategy
// produced the geometry context.

use cadmark_core::geometry::GeometryContext;
use serde::{Deserialize, Serialize};

/// A complete request to the AI backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiRequest {
    /// The current build123d source code.
    pub current_code: String,
    /// The user's message or spatial comment text.
    pub user_message: String,
    /// Geometry context if this is a spatial comment.
    pub geometry_context: Option<GeometryContext>,
    /// Whether this is a retry after a Python error.
    pub is_retry: bool,
    /// Python traceback from the failed execution, if retrying.
    pub traceback: Option<String>,
}

impl AiRequest {
    /// Create a request from a chat message (no geometry context).
    pub fn from_chat(code: String, message: String) -> Self {
        Self {
            current_code: code,
            user_message: message,
            geometry_context: None,
            is_retry: false,
            traceback: None,
        }
    }

    /// Create a request from a spatial comment.
    pub fn from_spatial_comment(
        code: String,
        message: String,
        context: GeometryContext,
    ) -> Self {
        Self {
            current_code: code,
            user_message: message,
            geometry_context: Some(context),
            is_retry: false,
            traceback: None,
        }
    }

    /// Create a retry request after a Python execution error.
    pub fn retry_with_traceback(mut self, traceback: String) -> Self {
        self.is_retry = true;
        self.traceback = Some(traceback);
        self
    }
}
