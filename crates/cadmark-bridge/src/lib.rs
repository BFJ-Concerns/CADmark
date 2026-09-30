//! CADmark bridge — the AI provider behind a provider-neutral seam.
//!
//! `backend` is the contract one turn of the tool-calling loop needs;
//! `openai_compatible` is the one transport, speaking the OpenAI Responses
//! protocol that every supported provider or gateway serves; `tools` are
//! the tools the loop offers; `grounding` renders spatial anchors for the
//! model; `doc_lookup` answers the documentation tool from the bundled
//! build123d corpus; `examples` is the curated few-shot library the turn
//! carries into the model's context.
//! build123d corpus; `sketch_route` reads back the sketch-or-solid route
//! the prompt asks the model to announce before it edits.

// Tests build every boundary-crossing type on its shared base with
// struct-update syntax, even when they name every field, so a field added
// later is filled in one place (crates/cadmark-core/tests/
// boundary_type_construction.rs holds them to it); clippy's complaint that
// such an update is redundant today is the point.
#![cfg_attr(test, allow(clippy::needless_update))]

pub mod backend;
pub mod config;
pub mod doc_lookup;
pub mod examples;
pub mod grounding;
mod openai_compatible;
pub mod request_log;
pub mod sketch_route;
pub mod tools;

use config::{AiConfiguration, ConfigurationError};
use doc_lookup::DocLookup;
use openai_compatible::OpenAiCompatibleClient;

/// The instructions every turn is run under.
pub const SYSTEM_PROMPT: &str = include_str!("system_prompt.md");

/// The configured model and the documentation consumer that shares its
/// client.
pub struct AiServices {
    pub model: OpenAiCompatibleClient,
    pub doc_lookup: DocLookup,
}

impl AiServices {
    /// Record every call both consumers make under `dir`.
    pub fn recording_to(self, dir: std::path::PathBuf) -> Self {
        let log = std::sync::Arc::new(request_log::RequestLog::in_directory(dir));
        let model = self.model.with_request_log(std::sync::Arc::clone(&log));
        AiServices {
            doc_lookup: DocLookup::new(model.clone()),
            model,
        }
    }

    /// Make a minimal credential-safe provider request for the ignored live smoke test.
    #[doc(hidden)]
    pub async fn smoke_test_exact_sentinel(
        &self,
        sentinel: &str,
    ) -> Result<(), backend::BackendError> {
        self.model.smoke_test_exact_sentinel(sentinel).await
    }
}

impl std::fmt::Debug for AiServices {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiServices")
            .field("model", &self.model)
            .field("doc_lookup", &self.doc_lookup)
            .finish()
    }
}

/// Resolve the credential once and construct the shared production consumers.
pub fn build_ai_services(
    configuration: AiConfiguration,
    credential: Option<String>,
) -> Result<AiServices, ConfigurationError> {
    let client = configuration.build_client(credential)?;
    Ok(AiServices {
        doc_lookup: DocLookup::new(client.clone()),
        model: client,
    })
}
