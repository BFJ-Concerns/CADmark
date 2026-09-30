// The conversation as the chat pane shows it and the project folder keeps
// it: what the user said and pointed at, what the AI said, what tools it
// ran along the way, and what CADmark itself reported.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::geometry::GeometryContext;
use crate::model_session::{ModelItem, ModelSession};

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
    /// The script text a successful `run_script` executed, kept so the
    /// next request can say whether the file on disk still matches the
    /// last run. Not replayed to the model: the call's arguments already
    /// carry what the model wrote, an edit or a whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executed_source: Option<String>,
}

impl ToolActivity {
    /// A call the model has just made: started now, not yet answered. The
    /// turn fills in the answer as it arrives; a test builds on this naming
    /// only the fields it reads.
    pub fn begin(
        call_id: impl Into<String>,
        tool: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            tool: tool.into(),
            arguments,
            output: None,
            failed: false,
            started: Utc::now(),
            finished: None,
            executed_source: None,
        }
    }
}

/// An image the model reads, already encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageData {
    /// `image/png` or `image/jpeg`.
    pub media_type: String,
    /// The encoded bytes; base64 text on disk, where a model session
    /// keeps the renders the model was shown.
    #[serde(with = "base64_bytes")]
    pub bytes: Vec<u8>,
}

mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    pub fn serialize<S: serde::Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u8>, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

/// Images reserve a conservative fixed budget each, since providers do not
/// share one way of pricing an image into the context window.
pub const IMAGE_TOKENS: usize = 765;

/// An image the user attached to a message. The conversation file records
/// the file's name in the project's attachment store, never its bytes; the
/// bytes are loaded from the store when the project opens and travel with
/// the message in memory, so the model sees the image again whenever the
/// message is replayed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageAttachment {
    /// The file name in the attachment store.
    pub file: String,
    /// The name the user knows it by: the original file name, or a label
    /// for a pasted image.
    pub name: String,
    /// `image/png` or `image/jpeg`.
    pub media_type: String,
    /// The encoded bytes; empty when the store no longer has the file.
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

impl ImageAttachment {
    /// The image as the model reads it, when the bytes are present.
    pub fn image(&self) -> Option<ImageData> {
        (!self.bytes.is_empty()).then(|| ImageData {
            media_type: self.media_type.clone(),
            bytes: self.bytes.clone(),
        })
    }
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
    /// The model's reasoning before one reply: how long it took, and
    /// whatever of it the provider shares (nothing, from a provider that
    /// keeps its reasoning private). Shown so a long silence reads as work;
    /// the display record is not replayed as assistant text.
    Thinking {
        /// When the reasoning ended; `None` while it is still going.
        finished: Option<DateTime<Utc>>,
    },
    /// A note from CADmark itself rather than the AI: an execution failure,
    /// a provider error, or AI being unavailable.
    Notice { is_error: bool },
    /// A change the user made to the design outside the chat — a value set
    /// in the parameters panel, a design step undone, redone, or restored —
    /// recorded where it happened so the AI's next turn knows when and why
    /// the script moved, not only that it differs from its last run.
    DesignChange,
    /// A refused export, recorded for the user and the model’s next turn.
    ExportRefusal,
}

/// A single message in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub kind: MessageKind,
    pub text: String,
    pub timestamp: DateTime<Utc>,
    /// Images the user attached to a chat message or spatial comment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ImageAttachment>,
}

impl Message {
    fn new(kind: MessageKind, text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            kind,
            text: text.into(),
            timestamp: Utc::now(),
            attachments: Vec::new(),
        }
    }

    pub fn user_chat(text: impl Into<String>) -> Self {
        Self::new(MessageKind::UserChat, text)
    }

    /// The message with these images attached.
    pub fn with_attachments(mut self, attachments: Vec<ImageAttachment>) -> Self {
        self.attachments = attachments;
        self
    }

    /// The attached images the model can read: those whose bytes are
    /// present.
    pub fn images(&self) -> Vec<ImageData> {
        self.attachments
            .iter()
            .filter_map(ImageAttachment::image)
            .collect()
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

    /// The model's reasoning before a reply, begun now; its text grows as
    /// the provider shares any.
    pub fn thinking() -> Self {
        Self::new(MessageKind::Thinking { finished: None }, "")
    }

    /// A note from CADmark about the session: shown quietly, not as speech.
    pub fn notice(text: impl Into<String>) -> Self {
        Self::new(MessageKind::Notice { is_error: false }, text)
    }

    /// A note about something that went wrong.
    pub fn error_notice(text: impl Into<String>) -> Self {
        Self::new(MessageKind::Notice { is_error: true }, text)
    }

    /// A change the user made outside the chat, for the AI to read as well
    /// as the user.
    pub fn design_change(text: impl Into<String>) -> Self {
        Self::new(MessageKind::DesignChange, text)
    }

    /// The export gate’s refusal, with its cause unchanged.
    pub fn export_refusal(text: impl Into<String>) -> Self {
        Self::new(MessageKind::ExportRefusal, text)
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
    /// The item sequence the model last answered, so the next request
    /// extends it byte for byte. Absent in a conversation no turn has
    /// recorded one for, where the messages are rendered afresh.
    #[serde(default)]
    session: ModelSession,
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

    pub fn messages_mut(&mut self) -> &mut [Message] {
        &mut self.messages
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

    /// Remove a message, for a turn that ended with nothing to show. A
    /// session that reached the removed message now reaches the one
    /// before it, so the messages after it are still rendered once.
    pub fn remove(&mut self, id: MessageId) {
        if self.session.covers == Some(id) {
            self.session.covers = self
                .messages
                .iter()
                .position(|message| message.id == id)
                .and_then(|index| index.checked_sub(1))
                .map(|index| self.messages[index].id);
            if self.session.covers.is_none() {
                self.session = ModelSession::default();
            }
        }
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
    /// The recorded session described the replaced messages, so it goes
    /// with them; the turn that condensed records the new one when it ends.
    pub fn condense_before(&mut self, index: usize, summary: impl Into<String>) {
        let retained = self.messages.split_off(index.min(self.messages.len()));
        self.messages = vec![Message::conversation_summary(summary)];
        self.messages.extend(retained);
        self.session = ModelSession::default();
    }

    /// The items the next request begins with, and the messages still to
    /// be rendered after them: the recorded session and what came after
    /// the message it reaches, or nothing and every message when no
    /// session is recorded or its message is gone.
    pub fn replay(&self) -> (&[ModelItem], &[Message]) {
        let reached = self
            .session
            .covers
            .and_then(|id| self.messages.iter().position(|message| message.id == id));
        match reached {
            Some(index) if !self.session.is_empty() => {
                (&self.session.items, &self.messages[index + 1..])
            }
            _ => (&[], &self.messages),
        }
    }

    /// Record the item sequence a turn ended with as the session the next
    /// request extends. It accounts for every message now in the
    /// conversation, this turn's own included.
    pub fn record_session(&mut self, items: Vec<ModelItem>) {
        self.record_session_for(items, None);
    }

    pub fn record_session_for(&mut self, items: Vec<ModelItem>, identity: Option<String>) {
        self.session = ModelSession {
            items,
            identity,
            covers: self.messages.last().map(|message| message.id),
        };
    }

    /// Reconstruct the human history when changing provider or model: opaque
    /// reasoning is meaningful only to the backend that issued it.
    pub fn for_model(&self, identity: Option<&str>) -> Self {
        let mut conversation = self.clone();
        if self.session.identity.as_deref() != identity {
            conversation.session = ModelSession::default();
        }
        conversation
    }

    pub fn session(&self) -> &ModelSession {
        &self.session
    }

    /// A deliberately conservative, provider-neutral estimate of what the
    /// next request's history weighs: the recorded session and the
    /// messages rendered after it. Providers do not expose one common
    /// tokenizer, so this is used to start condensing early rather than to
    /// claim an exact token count.
    pub fn estimated_tokens(&self) -> usize {
        let (items, messages) = self.replay();
        items.iter().map(ModelItem::estimated_tokens).sum::<usize>()
            + messages
                .iter()
                .map(|message| message.estimated_tokens())
                .sum::<usize>()
    }
}

/// A deliberately conservative, provider-neutral token estimate for a
/// piece of text: four characters to the token. Providers do not expose one
/// common tokenizer, so every occupancy figure in CADmark is this estimate,
/// used to act early rather than to claim an exact count.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

impl Message {
    fn estimated_tokens(&self) -> usize {
        // Provider reasoning is counted in the saved model session, not twice
        // through its separate display record.
        if let MessageKind::Thinking { .. } = &self.kind {
            return 0;
        }
        let image_tokens = self.attachments.len() * IMAGE_TOKENS;
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
        characters.div_ceil(4) + image_tokens
    }
}

/// The portion of a provider context window the next request will occupy,
/// as it is assembled: the saved conversation with the images attached to
/// it, the images going with this request, and everything else a request
/// carries — the instructions, the tool definitions, the current script,
/// the examples, and the words being sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsage {
    pub conversation_tokens: usize,
    /// The images attached to the request being sent, at the fixed
    /// per-image reservation.
    pub image_tokens: usize,
    /// What the request carries besides the conversation. Condensing the
    /// conversation does not reduce this.
    pub request_tokens: usize,
    pub window_tokens: usize,
}

impl ContextUsage {
    pub fn used_tokens(self) -> usize {
        self.conversation_tokens + self.image_tokens + self.request_tokens
    }

    pub fn percent(self) -> usize {
        if self.window_tokens == 0 {
            return 100;
        }
        self.used_tokens().saturating_mul(100) / self.window_tokens
    }

    fn over_three_quarters(tokens: usize, window: usize) -> bool {
        tokens.saturating_mul(4) >= window.saturating_mul(3)
    }

    /// Start condensing well before a request can overflow. This is a context
    /// threshold, never a count of messages or turns. Condensing is only
    /// asked for when the conversation is a large enough share of the
    /// window that shortening it can matter: when the fixed request alone
    /// is what fills the window, condensing every turn would cost a model
    /// call and change nothing.
    pub fn needs_condensing(self) -> bool {
        Self::over_three_quarters(self.used_tokens(), self.window_tokens)
            && self.conversation_tokens.saturating_mul(10) >= self.window_tokens
    }

    /// The request would not fit even with no conversation at all: the
    /// script, instructions and images alone are at the threshold. The user
    /// needs to know, because no amount of condensing helps.
    pub fn request_alone_is_over_budget(self) -> bool {
        Self::over_three_quarters(self.request_tokens + self.image_tokens, self.window_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{FaceId, GeometryContext, PickedElement, TopologyElement};
    use crate::ledger::{
        LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
    };

    fn sample_geometry_context() -> GeometryContext {
        GeometryContext::new(
            PickedElement::Solid(TopologyElement::Face(FaceId(5))),
            LedgerValue::Resolved(ProvenanceEntry {
                source: SourceRef {
                    line: 10,
                    code: "box = Box(10, 10, 10)".to_string(),
                },
                operation: SemanticOperation::Box,
                operation_id: 1,
                relation: ProvenanceRelation::Generated,
            }),
        )
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
    fn export_refusal_survives_conversation_json_round_trip() {
        let mut conversation = Conversation::new();
        conversation.push(Message::export_refusal(
            "STEP export refused: missing extrusion face.",
        ));
        let json = serde_json::to_string(&conversation).unwrap();
        let decoded = serde_json::from_str::<Conversation>(&json);
        assert!(decoded.is_ok(), "refusal could not be read: {decoded:?}");
        assert_eq!(decoded.unwrap(), conversation);
    }

    #[test]
    fn export_refusal_kind_preserves_loading_older_conversation_files() {
        let json = r#"{"messages":[{"id":"00000000-0000-0000-0000-000000000001","kind":"UserChat","text":"make a box","timestamp":"2026-09-01T12:00:00Z"}]}"#;
        let decoded = serde_json::from_str::<Conversation>(json);
        assert!(
            decoded.is_ok(),
            "older conversation could not be read: {decoded:?}"
        );
        let conversation = decoded.unwrap();
        assert_eq!(conversation.messages()[0].kind, MessageKind::UserChat);
        assert_eq!(conversation.messages()[0].text, "make a box");
        assert!(conversation.messages()[0].attachments.is_empty());
        assert!(conversation.session().is_empty());
    }

    #[test]
    fn conversations_round_trip_through_json() {
        let mut conv = Conversation::new();
        conv.push(Message::user_chat("make a box"));
        conv.push(Message::tool_calls(vec![ToolActivity {
            output: Some("ok".into()),
            finished: Some(Utc::now()),
            executed_source: Some("x = 1".into()),
            ..ToolActivity::begin("call_1", "run_script", serde_json::json!({"code": "x = 1"}))
        }]));
        conv.push(Message::error_notice("part.py failed to run"));
        let json = serde_json::to_string(&conv).unwrap();
        assert_eq!(serde_json::from_str::<Conversation>(&json).unwrap(), conv);
    }

    #[test]
    fn attachments_are_saved_by_file_name_and_weigh_as_images() {
        let attachment = ImageAttachment {
            file: "20260922-101500-flange.png".into(),
            name: "Flange".into(),
            media_type: "image/png".into(),
            bytes: vec![1, 2, 3],
        };
        let message = Message::user_chat("match this").with_attachments(vec![attachment.clone()]);
        assert_eq!(message.images(), [attachment.image().unwrap()]);
        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("20260922-101500-flange.png"));
        assert!(!json.contains("bytes"));
        let reloaded: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(reloaded.attachments[0].file, attachment.file);
        assert!(reloaded.attachments[0].bytes.is_empty());
        assert!(reloaded.images().is_empty());

        let plain = Message::user_chat("match this");
        assert!(
            !serde_json::to_string(&plain)
                .unwrap()
                .contains("attachments")
        );
        assert_eq!(
            message.estimated_tokens(),
            plain.estimated_tokens() + IMAGE_TOKENS
        );
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

    fn assistant(text: &str) -> ModelItem {
        ModelItem::Assistant {
            text: text.to_string(),
        }
    }

    #[test]
    fn a_recorded_session_is_replayed_and_only_later_messages_are_rendered() {
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat("Make a box."));
        conversation.push(Message::ai_response("Done."));
        conversation.record_session(vec![assistant("the exact items sent")]);
        let later = conversation.push(Message::design_change("Undid the box."));

        let (items, messages) = conversation.replay();
        assert_eq!(items, &[assistant("the exact items sent")]);
        assert_eq!(
            messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![later]
        );
        assert_eq!(
            conversation.estimated_tokens(),
            assistant("the exact items sent").estimated_tokens()
                + conversation.messages()[2].estimated_tokens()
        );
    }

    #[test]
    fn without_a_session_every_message_is_rendered() {
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat("Make a box."));
        let (items, messages) = conversation.replay();
        assert!(items.is_empty());
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn removing_the_message_a_session_reaches_moves_it_to_the_one_before() {
        let mut conversation = Conversation::new();
        let first = conversation.push(Message::user_chat("Make a box."));
        let blank = conversation.push(Message::ai_response(""));
        conversation.record_session(vec![assistant("sent")]);
        let after = conversation.push(Message::design_change("Undid the box."));

        conversation.remove(blank);

        assert_eq!(conversation.session().covers, Some(first));
        let (items, messages) = conversation.replay();
        assert_eq!(items.len(), 1);
        assert_eq!(
            messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![after]
        );
    }

    #[test]
    fn removing_the_only_message_a_session_reaches_drops_the_session() {
        let mut conversation = Conversation::new();
        let only = conversation.push(Message::ai_response(""));
        conversation.record_session(vec![assistant("sent")]);
        conversation.remove(only);
        assert!(conversation.session().is_empty());
        assert!(conversation.replay().0.is_empty());
    }

    #[test]
    fn condensing_drops_the_session_the_replaced_messages_described() {
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat("Use a 5 mm wall."));
        conversation.push(Message::ai_response("I will use a 5 mm wall."));
        conversation.record_session(vec![assistant("sent")]);
        let active_start = conversation.len();
        conversation.push(Message::user_chat("Add an open top."));

        conversation.condense_before(active_start, "Decision: 5 mm wall.");

        assert!(conversation.session().is_empty());
        let (items, messages) = conversation.replay();
        assert!(items.is_empty());
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn condensing_is_asked_for_only_when_shortening_the_conversation_can_help() {
        let usage = |conversation_tokens, request_tokens| ContextUsage {
            conversation_tokens,
            image_tokens: 0,
            request_tokens,
            window_tokens: 1_000,
        };
        // A long conversation past the threshold condenses.
        assert!(usage(800, 0).needs_condensing());
        // The request's own weight counts towards the threshold.
        assert!(usage(400, 400).needs_condensing());
        assert!(!usage(400, 300).needs_condensing());
        // A request that fills the window by itself is not a condensing
        // problem; the conversation is too small for condensing to matter.
        assert!(!usage(50, 900).needs_condensing());
        assert!(usage(50, 900).request_alone_is_over_budget());
        assert!(!usage(400, 400).request_alone_is_over_budget());
        assert_eq!(usage(400, 400).percent(), 80);
    }

    #[test]
    fn message_ids_are_unique() {
        let a = Message::user_chat("one");
        let b = Message::user_chat("two");
        assert_ne!(a.id, b.id);
    }
}
