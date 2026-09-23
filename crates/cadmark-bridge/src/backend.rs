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
    /// The provider or model server has no capacity right now: an
    /// overloaded hosted model, or a local server with every slot busy.
    Overloaded,
    /// The credential was rejected or is missing.
    Authentication,
    /// The configured model is not served at this endpoint.
    UnknownModel,
}

impl RefusalCause {
    pub fn describe(self) -> &'static str {
        match self {
            Self::UsageLimit => "the provider is at its usage limit or cooling down",
            Self::Overloaded => "the provider is overloaded or busy; try again shortly",
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

pub use cadmark_core::message::ImageData;
pub use cadmark_core::model_session::{ModelItem, ProviderUsage, ToolCall};

/// What a request is for, named in its transcript so a turn's own calls
/// read apart from the documentation lookup and the condensation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestPurpose {
    /// One request of the turn loop.
    Turn,
    /// The documentation tool answering a question from the corpus.
    DocLookup,
    /// The conversation being condensed before it overflows.
    Condense,
    /// A probe of the endpoint: the smoke test.
    Probe,
}

impl RequestPurpose {
    /// The word a transcript file name carries.
    pub fn slug(self) -> &'static str {
        match self {
            RequestPurpose::Turn => "turn",
            RequestPurpose::DocLookup => "docs",
            RequestPurpose::Condense => "condense",
            RequestPurpose::Probe => "probe",
        }
    }
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
    pub purpose: RequestPurpose,
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
    /// The provider stopped the response at its output-token limit before
    /// the model had finished. `text` is what was written up to that point
    /// and `tool_calls` holds only the calls written in full; a call cut
    /// off part-way is dropped, because its arguments cannot be read.
    pub reached_output_limit: bool,
    /// The token counts the provider reported, where it reported any.
    pub usage: Option<ProviderUsage>,
}

/// A fragment of the model's work, delivered as it arrives so the user can
/// see the turn is alive.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamDelta {
    /// More of the model's text.
    Text(String),
    /// The model has begun a tool call; its arguments follow later.
    ToolCallStarted { name: String },
    /// The model is reasoning before it answers: a piece of the reasoning
    /// text where the provider shares it, or empty where the provider
    /// keeps the reasoning private and sends only the beat of it.
    Reasoning(String),
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
