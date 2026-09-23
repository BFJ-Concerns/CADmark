// The conversation as the model is shown it: the exact item sequence the
// last request carried and the reply it drew, kept so the next request
// extends that sequence instead of rendering the chat afresh.
//
// Keeping an unchanged prefix permits provider cache reuse while an entry
// remains available. The chat messages are the human record; this is the
// model's, including opaque continuation data.

use serde::{Deserialize, Serialize};

use crate::message::{IMAGE_TOKENS, ImageData, MessageId, estimate_tokens};

/// One item of the conversation the model is shown, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ModelItem {
    /// A provider output item, retained unchanged for continuation, including
    /// encrypted reasoning, message metadata and tool argument spelling.
    ProviderOutput(serde_json::Value),
    /// Instructions scoped to a turn, appended without changing the prefix.
    Developer { text: String },
    /// What the user said, with any images attached.
    User {
        text: String,
        images: Vec<ImageData>,
    },
    /// What the model said.
    Assistant { text: String },
    /// A tool the model asked to run.
    ToolCall(ToolCall),
    /// What the tool returned, paired to the call by ID.
    ToolResult { call_id: String, output: String },
}

impl ModelItem {
    /// The item's weight against a context window, by the same estimate
    /// every occupancy figure in CADmark uses.
    pub fn estimated_tokens(&self) -> usize {
        match self {
            ModelItem::User { text, images } => estimate_tokens(text) + images.len() * IMAGE_TOKENS,
            ModelItem::Assistant { text } | ModelItem::Developer { text } => estimate_tokens(text),
            ModelItem::ProviderOutput(item) => estimate_tokens(&item.to_string()),
            ModelItem::ToolCall(call) => {
                estimate_tokens(&call.name) + estimate_tokens(&call.arguments.to_string())
            }
            ModelItem::ToolResult { output, .. } => estimate_tokens(output),
        }
    }
}

/// A tool call the model made: the tool's name and its JSON arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The provider's ID for the call; the result carries it back.
    pub id: String,
    pub name: String,
    /// The arguments as the model wrote them, a JSON object.
    pub arguments: serde_json::Value,
}

/// The item sequence the model last answered, and the chat message it
/// reaches. Messages after that one — a design change noted between
/// turns, the next turn's own input — are rendered and appended when the
/// next request is built, so the recorded items are never rendered twice.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelSession {
    pub items: Vec<ModelItem>,
    /// Endpoint and model that issued opaque provider output. Old sessions have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// The last chat message the items account for; `None` when no turn
    /// has recorded a session.
    pub covers: Option<MessageId>,
}

impl ModelSession {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn estimated_tokens(&self) -> usize {
        self.items.iter().map(ModelItem::estimated_tokens).sum()
    }
}

/// The token counts the provider reported for one request: what it read,
/// how much of that it served from its cache, and what the model wrote,
/// reasoning included. The one measurement CADmark's own estimates can be
/// checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub input_tokens: usize,
    /// The part of `input_tokens` the provider served from its prompt
    /// cache.
    pub cached_input_tokens: Option<usize>,
    pub output_tokens: usize,
    /// The part of `output_tokens` the model spent reasoning before it
    /// wrote anything visible.
    pub reasoning_tokens: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_reasoning_is_persisted_and_scoped_to_its_provider() {
        use crate::message::{Conversation, Message};
        let item = ModelItem::ProviderOutput(serde_json::json!({"type":"reasoning",
            "summary":[],"encrypted_content":"opaque state"}));
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat("Keep this history"));
        conversation.record_session_for(vec![item.clone()], Some("endpoint-a|model-a".into()));
        let json = serde_json::to_string(&conversation).unwrap();
        let restored: Conversation = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.for_model(Some("endpoint-a|model-a")).replay().0,
            [item]
        );
        let changed = restored.for_model(Some("endpoint-a|model-b"));
        assert!(changed.replay().0.is_empty());
        assert_eq!(changed.replay().1[0].text, "Keep this history");
        assert!(restored.estimated_tokens() > 0);
    }

    #[test]
    fn an_item_weighs_its_text_and_a_fixed_reservation_per_image() {
        let image = ImageData {
            media_type: "image/png".to_string(),
            bytes: vec![1, 2, 3],
        };
        let item = ModelItem::User {
            text: "abcdefgh".to_string(),
            images: vec![image.clone(), image],
        };
        assert_eq!(item.estimated_tokens(), 2 + 2 * IMAGE_TOKENS);
    }

    #[test]
    fn a_session_round_trips_through_json_with_its_images() {
        let session = ModelSession {
            identity: None,
            items: vec![
                ModelItem::User {
                    text: "look".to_string(),
                    images: vec![ImageData {
                        media_type: "image/png".to_string(),
                        bytes: vec![0, 255, 128],
                    }],
                },
                ModelItem::ToolCall(ToolCall {
                    id: "call_1".to_string(),
                    name: "run_script".to_string(),
                    arguments: serde_json::json!({"code": "x = 1"}),
                }),
                ModelItem::ToolResult {
                    call_id: "call_1".to_string(),
                    output: "ran".to_string(),
                },
                ModelItem::Assistant {
                    text: "done".to_string(),
                },
            ],
            covers: Some(MessageId::new()),
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(
            json.contains("\"AP+A\""),
            "image bytes travel as base64: {json}"
        );
        let back: ModelSession = serde_json::from_str(&json).unwrap();
        assert_eq!(back, session);
    }
}
