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
    /// Relevant build123d API documentation extracted by the doc lookup
    /// pre-processing agent. Injected into the design prompt so the AI
    /// has exact signatures and usage patterns to work from.
    pub doc_context: Option<String>,
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
            doc_context: None,
        }
    }

    /// Create a request from a spatial comment.
    pub fn from_spatial_comment(code: String, message: String, context: GeometryContext) -> Self {
        Self {
            current_code: code,
            user_message: message,
            geometry_context: Some(context),
            is_retry: false,
            traceback: None,
            doc_context: None,
        }
    }

    /// Attach documentation context from the doc lookup agent.
    pub fn with_doc_context(mut self, doc_context: String) -> Self {
        self.doc_context = Some(doc_context);
        self
    }

    /// Create a retry request after a Python execution error.
    pub fn retry_with_traceback(mut self, traceback: String) -> Self {
        self.is_retry = true;
        self.traceback = Some(traceback);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadmark_core::ledger::{
        LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
    };

    #[test]
    fn from_chat_sets_fields_correctly() {
        let req = AiRequest::from_chat("code".to_string(), "hello".to_string());
        assert_eq!(req.current_code, "code");
        assert_eq!(req.user_message, "hello");
        assert!(req.geometry_context.is_none());
        assert!(!req.is_retry);
        assert!(req.traceback.is_none());
    }

    #[test]
    fn from_spatial_comment_includes_context() {
        let ctx = GeometryContext {
            element: cadmark_core::geometry::TopologyElement::Face(cadmark_core::geometry::FaceId(
                1,
            )),
            provenance: LedgerValue::Resolved(ProvenanceEntry {
                source: SourceRef {
                    line: 10,
                    code: "box = Box(5,5,5)".to_string(),
                },
                operation: SemanticOperation::Box,
                operation_id: 1,
                relation: ProvenanceRelation::Generated,
            }),
            identification: std::collections::HashMap::new(),
        };
        let req =
            AiRequest::from_spatial_comment("code".to_string(), "round this".to_string(), ctx);
        let provenance = req.geometry_context.unwrap().provenance;
        assert_eq!(provenance.resolved().unwrap().source.line, 10);
    }

    #[test]
    fn retry_with_traceback_sets_retry_fields() {
        let req = AiRequest::from_chat("code".to_string(), "msg".to_string())
            .retry_with_traceback("NameError".to_string());
        assert!(req.is_retry);
        assert_eq!(req.traceback.as_deref(), Some("NameError"));
        // Original fields preserved.
        assert_eq!(req.current_code, "code");
        assert_eq!(req.user_message, "msg");
    }
}
