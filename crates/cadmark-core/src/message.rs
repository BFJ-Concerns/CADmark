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

    /// Mark all pending spatial comments as applied.
    pub fn mark_all_spatial_applied(&mut self) {
        for message in &mut self.messages {
            message.mark_applied();
        }
    }
}
