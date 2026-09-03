//! CADmark bridge — provider-independent AI consumers and configuration.

pub mod backend;
pub mod config;
pub mod context;
pub mod doc_lookup;
pub mod model_edit;
mod openai_compatible;

use config::{AiConfiguration, ConfigurationError};
use doc_lookup::DocLookup;
use model_edit::ModelEditBackend;

/// Both AI consumers constructed from one resolved configuration and client.
pub struct AiServices {
    pub model_edit: ModelEditBackend,
    pub doc_lookup: DocLookup,
}

impl AiServices {
    /// The model name both consumers send requests to.
    pub fn model_name(&self) -> &str {
        self.model_edit.model_name()
    }

    /// Make a minimal credential-safe provider request for the ignored live smoke test.
    #[doc(hidden)]
    pub async fn smoke_test_exact_sentinel(
        &self,
        sentinel: &str,
    ) -> Result<(), backend::BackendError> {
        self.model_edit.smoke_test_exact_sentinel(sentinel).await
    }
}

impl std::fmt::Debug for AiServices {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiServices")
            .field("model_edit", &self.model_edit)
            .field("doc_lookup", &self.doc_lookup)
            .finish()
    }
}

/// Resolve credentials once and construct the shared production consumers.
pub fn build_ai_services_with_env<F>(
    configuration: AiConfiguration,
    environment: F,
) -> Result<AiServices, ConfigurationError>
where
    F: FnOnce(&str) -> Option<String>,
{
    let client = configuration.build_client_with_env(environment)?;
    Ok(AiServices {
        model_edit: ModelEditBackend::new(client.clone()),
        doc_lookup: DocLookup::new(client),
    })
}
