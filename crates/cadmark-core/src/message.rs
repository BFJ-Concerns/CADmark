// The conversation as the chat pane shows it and the project folder keeps
// it: what the user said and pointed at, what the AI said, what tools it
// ran along the way, and what CADmark itself reported.

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

/// One tool call the AI made during a turn, shown collapsed in chat and
/// expandable to its input and result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolActivity {
    /// The provider's ID for the call, so the conversation can be replayed
    /// to the model with each result paired to its call.
    pub call_id: String,
    pub tool: String,
    /// The arguments as the model wrote them.
    pub arguments: serde_json::Value,
    /// What the tool returned, or `None` while it is still running.
    pub output: Option<String>,
    /// Whether the tool reported a failure the model then acted on.
    pub failed: bool,
    pub started: DateTime<Utc>,
    pub finished: Option<DateTime<Utc>>,
}

/// The kind of message — determines visual treatment in the chat pane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MessageKind {
    /// Free-text from the user, typed in the chat input.
    UserChat,
    /// A spatial comment anchored to one or more elements. Carries the
    /// geometry context resolved when the comment was written.
    SpatialComment {
        anchors: Vec<GeometryContext>,
        /// Whether the AI has acted on this comment.
        applied: bool,
    },
    /// Response from the AI. While a turn streams, the text grows.
    AiResponse,
    /// A durable account of an earlier stretch of conversation. It replaces
    /// chatter only after the model has retained the decisions and open work.
    ConversationSummary,
    /// A run of consecutive tool calls in one turn, grouped so a long
    /// turn reads as one collapsed entry.
    ToolCalls(Vec<ToolActivity>),
    /// A note from CADmark itself rather than the AI: an execution failure,
    /// a provider error, or AI being unavailable.
    Notice { is_error: bool },
}

/// A single message in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub kind: MessageKind,
    pub text: String,
    pub timestamp: DateTime<Utc>,
}

impl Message {
    fn new(kind: MessageKind, text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            kind,
            text: text.into(),
            timestamp: Utc::now(),
        }
    }

    pub fn user_chat(text: impl Into<String>) -> Self {
        Self::new(MessageKind::UserChat, text)
    }

    pub fn spatial_comment(text: impl Into<String>, anchors: Vec<GeometryContext>) -> Self {
        Self::new(
            MessageKind::SpatialComment {
                anchors,
                applied: false,
            },
            text,
        )
    }

    pub fn ai_response(text: impl Into<String>) -> Self {
        Self::new(MessageKind::AiResponse, text)
    }

    pub fn conversation_summary(text: impl Into<String>) -> Self {
        Self::new(MessageKind::ConversationSummary, text)
    }

    /// A group of tool calls; its text is unused.
    pub fn tool_calls(activities: Vec<ToolActivity>) -> Self {
        Self::new(MessageKind::ToolCalls(activities), "")
    }

    /// A note from CADmark about the session: shown quietly, not as speech.
    pub fn notice(text: impl Into<String>) -> Self {
        Self::new(MessageKind::Notice { is_error: false }, text)
    }

    /// A note about something that went wrong.
    pub fn error_notice(text: impl Into<String>) -> Self {
        Self::new(MessageKind::Notice { is_error: true }, text)
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

/// The full conversation history. Serialised whole to the project folder
/// so it is there when the project is reopened.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    messages: Vec<Message>,
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, message: Message) -> MessageId {
        let id = message.id;
        self.messages.push(message);
        id
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn message_mut(&mut self, id: MessageId) -> Option<&mut Message> {
        self.messages.iter_mut().find(|message| message.id == id)
    }

    /// Append text to a streaming AI response.
    pub fn append_text(&mut self, id: MessageId, delta: &str) {
        if let Some(message) = self.message_mut(id) {
            message.text.push_str(delta);
        }
    }

    /// Mark a specific spatial comment as applied.
    pub fn mark_spatial_applied(&mut self, id: MessageId) {
        if let Some(message) = self.message_mut(id) {
            message.mark_applied();
        }
    }

    /// Remove a message, for a turn that ended with nothing to show.
    pub fn remove(&mut self, id: MessageId) {
        self.messages.retain(|message| message.id != id);
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Replace the completed portion of a conversation with the model's
    /// compact account, retaining messages which belong to the active turn.
    pub fn condense_before(&mut self, index: usize, summary: impl Into<String>) {
        let retained = self.messages.split_off(index.min(self.messages.len()));
        self.messages = vec![Message::conversation_summary(summary)];
        self.messages.extend(retained);
    }

    /// A deliberately conservative, provider-neutral estimate. Providers do
    /// not expose one common tokenizer, so this is used to start condensing
    /// early rather than to claim an exact token count.
    pub fn estimated_tokens(&self) -> usize {
        self.messages
            .iter()
            .map(|message| message.estimated_tokens())
            .sum()
    }
}

impl Message {
    fn estimated_tokens(&self) -> usize {
        let mut characters = self.text.chars().count();
        if let MessageKind::ToolCalls(activities) = &self.kind {
            for activity in activities {
                characters += activity.tool.chars().count();
                characters += activity.arguments.to_string().chars().count();
                characters += activity
                    .output
                    .as_deref()
                    .map(str::chars)
                    .map(Iterator::count)
                    .unwrap_or_default();
            }
        }
        characters.div_ceil(4)
    }
}

/// The portion of a provider context window consumed before the next turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsage {
    pub conversation_tokens: usize,
    pub reference_image_tokens: usize,
    pub window_tokens: usize,
}

impl ContextUsage {
    pub fn used_tokens(self) -> usize {
        self.conversation_tokens + self.reference_image_tokens
    }

    pub fn percent(self) -> usize {
        if self.window_tokens == 0 {
            return 100;
        }
        self.used_tokens().saturating_mul(100) / self.window_tokens
    }

    /// Start condensing well before a request can overflow. This is a context
    /// threshold, never a count of messages or turns.
    pub fn needs_condensing(self) -> bool {
        self.used_tokens().saturating_mul(4) >= self.window_tokens.saturating_mul(3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{FaceId, GeometryContext, TopologyElement};
    use crate::ledger::{
        LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
    };

    fn sample_geometry_context() -> GeometryContext {
        GeometryContext {
            element: TopologyElement::Face(FaceId(5)),
            provenance: LedgerValue::Resolved(ProvenanceEntry {
                source: SourceRef {
                    line: 10,
                    code: "box = Box(10, 10, 10)".to_string(),
                },
                operation: SemanticOperation::Box,
                operation_id: 1,
                relation: ProvenanceRelation::Generated,
            }),
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
        }
    }

    #[test]
    fn spatial_comment_lifecycle() {
        let mut msg =
            Message::spatial_comment("Make this edge sharper", vec![sample_geometry_context()]);
        assert!(!msg.is_applied());
        msg.mark_applied();
        assert!(msg.is_applied());
    }

    #[test]
    fn mark_applied_is_noop_on_non_spatial() {
        let mut msg = Message::user_chat("hello");
        msg.mark_applied();
        assert!(!msg.is_applied());
    }

    #[test]
    fn conversation_marks_only_target_spatial_comment_applied() {
        let mut conv = Conversation::new();
        conv.push(Message::user_chat("make a box"));
        let first_id = conv.push(Message::spatial_comment(
            "round this",
            vec![sample_geometry_context()],
        ));
        conv.push(Message::ai_response("Done."));
        let second_id = conv.push(Message::spatial_comment(
            "and this",
            vec![sample_geometry_context()],
        ));

        conv.mark_spatial_applied(first_id);

        let applied: Vec<_> = conv
            .messages()
            .iter()
            .filter(|m| m.is_applied())
            .map(|m| m.id)
            .collect();
        assert_eq!(applied, [first_id]);
        assert!(
            conv.messages()
                .iter()
                .any(|m| m.id == second_id && !m.is_applied())
        );
    }

    #[test]
    fn streaming_text_grows_the_targeted_response() {
        let mut conv = Conversation::new();
        let id = conv.push(Message::ai_response(""));
        conv.append_text(id, "Made ");
        conv.append_text(id, "a box.");
        assert_eq!(conv.messages()[0].text, "Made a box.");
        conv.remove(id);
        assert!(conv.is_empty());
    }

    #[test]
    fn conversations_round_trip_through_json() {
        let mut conv = Conversation::new();
        conv.push(Message::user_chat("make a box"));
        conv.push(Message::tool_calls(vec![ToolActivity {
            call_id: "call_1".into(),
            tool: "run_script".into(),
            arguments: serde_json::json!({"code": "x = 1"}),
            output: Some("ok".into()),
            failed: false,
            started: Utc::now(),
            finished: Some(Utc::now()),
        }]));
        conv.push(Message::error_notice("part.py failed to run"));
        let json = serde_json::to_string(&conv).unwrap();
        assert_eq!(serde_json::from_str::<Conversation>(&json).unwrap(), conv);
    }

    #[test]
    fn condensing_keeps_the_active_request_after_replacing_old_history() {
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat("Use a 5 mm wall."));
        conversation.push(Message::ai_response("I will use a 5 mm wall."));
        let active_start = conversation.len();
        conversation.push(Message::user_chat("Add an open top."));

        conversation.condense_before(
            active_start,
            "Decision: use a 5 mm wall. Open request: add an open top.",
        );

        assert!(matches!(
            conversation.messages()[0].kind,
            MessageKind::ConversationSummary
        ));
        assert!(conversation.messages()[0].text.contains("5 mm wall"));
        assert_eq!(conversation.messages()[1].text, "Add an open top.");
    }

    #[test]
    fn message_ids_are_unique() {
        let a = Message::user_chat("one");
        let b = Message::user_chat("two");
        assert_ne!(a.id, b.id);
    }
}
