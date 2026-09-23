// The conversation as the model is shown it: the exact item sequence the
// last request carried and the reply it drew, kept so the next request
// extends that sequence instead of rendering the chat afresh.
//
// Providers cache a request by its prefix. A turn whose request begins
// with the previous request's bytes pays for the new items only; a turn
// that rebuilds the history from the chat messages diverges where the
// previous turn's own blocks sat and pays for the whole conversation
// again. The chat messages stay the human record; this is the model's.

use serde::{Deserialize, Serialize};

use crate::message::{IMAGE_TOKENS, ImageData, MessageId, estimate_tokens};

/// One item of the conversation the model is shown, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ModelItem {
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
            ModelItem::Assistant { text } => estimate_tokens(text),
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
    pub cached_input_tokens: usize,
    pub output_tokens: usize,
    /// The part of `output_tokens` the model spent reasoning before it
    /// wrote anything visible.
    pub reasoning_tokens: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

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
