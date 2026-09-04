// The AI model as one turn of a tool-calling loop sees it: a request
// carrying the conversation so far and the tools on offer, a stream of
// deltas while the model works, and a response that is either final text
// or a set of tool calls to execute before asking again.
//
// Provider-neutral: the transport (`openai_compatible.rs`) maps this onto
// the wire; the loop (`cadmark-app`'s turn module) drives it.

use std::future::Future;
use std::pin::Pin;

use cadmark_core::cancellation::CancelFlag;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Why a provider would not serve a request, for the message the user
/// reads. A refusal names its cause; a generic failure is the last resort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalCause {
    /// A usage limit, quota, or cooling-down period.
    UsageLimit,
    /// The credential was rejected or is missing.
    Authentication,
    /// The configured model is not served at this endpoint.
    UnknownModel,
}

impl RefusalCause {
    pub fn describe(self) -> &'static str {
        match self {
            Self::UsageLimit => "the provider is at its usage limit or cooling down",
            Self::Authentication => "the provider rejected the credential",
            Self::UnknownModel => "the provider does not serve the configured model",
        }
    }
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    #[error("AI provider is not reachable: {0}")]
    Unavailable(String),
    #[error("AI provider refused the request: {}: {detail}", cause.describe())]
    Refused { cause: RefusalCause, detail: String },
    #[error("AI request failed: {0}")]
    RequestFailed(String),
    #[error("AI response could not be read: {0}")]
    ParseError(String),
    #[error("the turn was cancelled")]
    Cancelled,
}

/// An image the model reads, already encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageData {
    /// `image/png` or `image/jpeg`.
    pub media_type: String,
    /// The encoded bytes.
    pub bytes: Vec<u8>,
}

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

/// A tool call the model made: the tool's name and its JSON arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The provider's ID for the call; the result carries it back.
    pub id: String,
    pub name: String,
    /// The arguments as the model wrote them, a JSON object.
    pub arguments: serde_json::Value,
}

/// A tool offered to the model: name, purpose, and a JSON Schema for its
/// arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// One request of the loop.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    /// The system instructions.
    pub instructions: String,
    /// The conversation so far, oldest first.
    pub items: Vec<ModelItem>,
    /// The tools the model may call.
    pub tools: Vec<ToolSpec>,
}

/// What the model produced for one request. Text and tool calls may both
/// be present; an empty `tool_calls` ends the loop.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

/// A fragment of the model's work, delivered as it arrives so the user can
/// see the turn is alive.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamDelta {
    /// More of the model's text.
    Text(String),
    /// The model has begun a tool call; its arguments follow later.
    ToolCallStarted { name: String },
}

/// Receives stream deltas. Called on the requesting task; must not block.
pub type DeltaSink<'a> = &'a mut (dyn FnMut(StreamDelta) + Send);

/// A model that takes one request of the loop and streams its answer.
pub trait TurnModel: Send + Sync {
    /// The model name, for the toolbar badge.
    fn model_name(&self) -> &str;

    /// Whether the model reads image inputs. Governs whether the render
    /// and reference-image tools are offered.
    fn accepts_images(&self) -> bool;

    /// Send one request and stream its deltas to `sink` until the
    /// response is complete. Polls `cancel` while waiting on the provider
    /// and returns `BackendError::Cancelled` when it is set; a quiet
    /// stream is waited on for as long as the user allows.
    fn respond<'a>(
        &'a self,
        request: ModelRequest,
        cancel: CancelFlag,
        sink: DeltaSink<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ModelResponse, BackendError>> + Send + 'a>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_read_by_cause() {
        let error = BackendError::Refused {
            cause: RefusalCause::UsageLimit,
            detail: "HTTP 429".into(),
        };
        assert_eq!(
            error.to_string(),
            "AI provider refused the request: the provider is at its usage limit or cooling down: HTTP 429"
        );
    }
}
