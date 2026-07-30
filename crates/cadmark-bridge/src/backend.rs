// AI backend trait — the abstraction that allows swapping providers.

use std::future::Future;
use std::pin::Pin;

use thiserror::Error;

use crate::context::AiRequest;

#[derive(Error, Debug)]
pub enum BackendError {
    #[error("AI backend not available: {0}")]
    Unavailable(String),
    #[error("AI request failed: {0}")]
    RequestFailed(String),
    #[error("AI response could not be parsed: {0}")]
    ParseError(String),
    #[error("AI request timed out")]
    Timeout,
}

/// Response from the AI backend.
#[derive(Debug, Clone)]
pub struct AiResponse {
    /// The modified build123d source code.
    pub code: String,
    /// A one-line summary of the change (for microversion commit messages).
    pub summary: String,
    /// The full conversational response text (shown in chat).
    pub message: String,
}

/// Abstract model-edit backend interface.
pub trait AiBackend: Send + Sync {
    /// Send a request to the AI and get a response.
    fn request(
        &self,
        request: AiRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse, BackendError>> + Send + '_>>;
}
