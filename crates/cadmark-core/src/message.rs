// Message types for the chat pane.
//
// Three distinct visual types: user chat, spatial comments (with
// geometry chip), and AI responses. Spatial comments transition
// to "Applied" state after the AI acts on them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::geometry::GeometryContext;

/// Unique identifier for a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub Uuid);

impl MessageId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}

/// The kind of message — determines visual treatment in the chat pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageKind {
    /// Free-text from the user, typed in the chat input.
    UserChat,
    /// A spatial comment anchored to geometry. Includes the geometry
    /// context that was selected when the comment was written.
    SpatialComment {
        context: GeometryContext,
        /// Whether the AI has acted on this comment.
        applied: bool,
    },
    /// Response from the AI.
    AiResponse,
}

/// A single message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub kind: MessageKind,
    pub text: String,
    pub timestamp: DateTime<Utc>,
}

impl Message {
    pub fn user_chat(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            kind: MessageKind::UserChat,
            text: text.into(),
            timestamp: Utc::now(),
        }
    }

    pub fn spatial_comment(text: impl Into<String>, context: GeometryContext) -> Self {
        Self {
            id: MessageId::new(),
            kind: MessageKind::SpatialComment {
                context,
                applied: false,
            },
            text: text.into(),
            timestamp: Utc::now(),
        }
    }

    pub fn ai_response(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            kind: MessageKind::AiResponse,
            text: text.into(),
            timestamp: Utc::now(),
        }
    }

    /// Mark a spatial comment as applied (AI has acted on it).
    pub fn mark_applied(&mut self) {
        if let MessageKind::SpatialComment { applied, .. } = &mut self.kind {
            *applied = true;
        }
    }

    pub fn is_applied(&self) -> bool {
        matches!(
            &self.kind,
            MessageKind::SpatialComment { applied: true, .. }
        )
    }
}

/// The full conversation history.
#[derive(Debug, Default)]
pub struct Conversation {
    messages: Vec<Message>,
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Mark a specific spatial comment as applied.
    pub fn mark_spatial_applied(&mut self, id: MessageId) {
        for message in &mut self.messages {
            if message.id == id {
                message.mark_applied();
                break;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{FaceId, GeometryContext, TopologyElement};
    use crate::ledger::{ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef};

    fn sample_geometry_context() -> GeometryContext {
        GeometryContext {
            element: TopologyElement::Face(FaceId(5)),
            provenance: ProvenanceEntry {
                source: SourceRef {
                    line: 10,
                    code: "box = Box(10, 10, 10)".to_string(),
                },
                operation: SemanticOperation::Box,
                operation_id: 1,
                relation: ProvenanceRelation::Generated,
            },
            identification: Default::default(),
        }
    }

    #[test]
    fn spatial_comment_lifecycle() {
        let mut msg = Message::spatial_comment("Make this edge sharper", sample_geometry_context());
        assert!(!msg.is_applied());

        msg.mark_applied();
        assert!(msg.is_applied());
    }

    #[test]
    fn mark_applied_is_noop_on_non_spatial() {
        let mut msg = Message::user_chat("hello");
        msg.mark_applied(); // Should not panic.
        assert!(!msg.is_applied());
    }

    #[test]
    fn conversation_marks_only_target_spatial_comment_applied() {
        let mut conv = Conversation::new();
        conv.push(Message::user_chat("make a box"));
        let first = Message::spatial_comment("round this", sample_geometry_context());
        let first_id = first.id;
        conv.push(first);
        conv.push(Message::ai_response("Done."));
        let second = Message::spatial_comment("and this", sample_geometry_context());
        let second_id = second.id;
        conv.push(second);

        conv.mark_spatial_applied(first_id);

        // Only the targeted spatial comment should be applied.
        let applied_count = conv.messages().iter().filter(|m| m.is_applied()).count();
        assert_eq!(applied_count, 1);
        assert!(
            conv.messages()
                .iter()
                .any(|m| m.id == first_id && m.is_applied())
        );
        assert!(
            conv.messages()
                .iter()
                .any(|m| m.id == second_id && !m.is_applied())
        );

        // Non-spatial messages are unaffected.
        assert!(!conv.messages()[0].is_applied());
        assert!(!conv.messages()[2].is_applied());
    }

    #[test]
    fn message_ids_are_unique() {
        let a = Message::user_chat("one");
        let b = Message::user_chat("two");
        assert_ne!(a.id, b.id);
    }
}
