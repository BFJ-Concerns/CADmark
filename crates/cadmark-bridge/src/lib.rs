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

pub mod backend;
pub mod config;
pub mod doc_lookup;
pub mod examples;
pub mod grounding;
mod openai_compatible;
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

#[cfg(test)]
mod tests {
    use super::SYSTEM_PROMPT;

    #[test]
    fn modelling_prompt_keeps_build123d_idioms_available() {
        // This is deliberately a bounded regression check, not an attempt to
        // decide whether arbitrary prose restricts build123d. The prompt is
        // the production instructions consumer receives, and these are the
        // two retired restrictions C36 names.
        let prompt = SYSTEM_PROMPT
            .to_ascii_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            prompt.contains("builder mode with context managers")
                && prompt.contains("algebra mode with operators")
                && prompt.contains("or the direct api"),
            "the production prompt must offer builder, algebra, and direct-API idioms"
        );
        assert!(
            !prompt.contains("import with `from build123d import *`"),
            "the production prompt must not reinstate the retired wildcard-import rule"
        );
        assert!(
            !prompt.contains("the script must leave a completed `buildpart` in the namespace")
                && !prompt
                    .contains("every part the user asked for is a `buildpart` at the top level"),
            "the production prompt must not reinstate the retired BuildPart-only rule"
        );
    }
}
