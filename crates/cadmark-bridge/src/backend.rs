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

/// Abstract AI backend interface.
///
/// Era 0: implemented by Claude Code subprocess.
/// Era 0.1: direct API implementations for Claude, GPT, etc.
pub trait AiBackend: Send + Sync {
    /// Send a request to the AI and get a response.
    fn request(
        &self,
        request: AiRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse, BackendError>> + Send + '_>>;

    /// Check whether the backend is available and configured.
    fn is_available(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
}
