//! AI provider configuration: where the model is and what it is called.
//!
//! The credential is not part of this structure. It reaches
//! `build_client` already resolved by the application (from the user's
//! credential file or an environment variable), so no configuration value
//! ever carries a secret.

use std::fmt;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::openai_compatible::{Credential, OpenAiCompatibleClient};

/// Provider settings as stored in the user's settings file.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiConfiguration {
    /// The OpenAI Responses-compatible base URL, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    pub model: String,
    /// Whether the model reads images: governs the render and
    /// reference-image tools.
    #[serde(default)]
    pub accepts_images: bool,
    /// Permit a plain-HTTP endpoint (a local model server, a LAN gateway).
    #[serde(default)]
    pub allow_insecure_http: bool,
    /// The reasoning effort asked of the model on every request
    /// (`low`, `medium`, `high`, or whatever the endpoint accepts), sent
    /// as the Responses `reasoning.effort` field. Unset, nothing is sent
    /// and the provider's default applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// A configuration error safe to show to the user.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConfigurationError {
    #[error("AI base URL is invalid")]
    InvalidBaseUrl,
    #[error("AI base URL must use HTTPS unless `allow_insecure_http` is true")]
    InsecureHttp,
    #[error("AI model must not be blank")]
    InvalidModel,
    #[error("could not construct the AI HTTP client")]
    ClientConstruction,
}

impl AiConfiguration {
    /// Validate the settings and construct the provider client.
    pub(crate) fn build_client(
        self,
        credential: Option<String>,
    ) -> Result<OpenAiCompatibleClient, ConfigurationError> {
        let model = self.model.trim();
        if model.is_empty() {
            return Err(ConfigurationError::InvalidModel);
        }

        let mut base_url =
            Url::parse(self.base_url.trim()).map_err(|_| ConfigurationError::InvalidBaseUrl)?;
        if !matches!(base_url.scheme(), "http" | "https")
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || base_url.host_str().is_none()
        {
            return Err(ConfigurationError::InvalidBaseUrl);
        }
        if base_url.scheme() == "http" && !self.allow_insecure_http {
            return Err(ConfigurationError::InsecureHttp);
        }

        let path = base_url.path().trim_end_matches('/');
        base_url.set_path(&format!("{path}/responses"));

        OpenAiCompatibleClient::new(
            base_url,
            model.to_string(),
            credential
                .filter(|value| !value.trim().is_empty())
                .map(Credential::new),
            self.accepts_images,
        )
        .map(|client| client.with_reasoning_effort(self.reasoning_effort))
        .map_err(|_| ConfigurationError::ClientConstruction)
    }
}

impl fmt::Debug for AiConfiguration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiConfiguration")
            .field("base_url", &"[CONFIGURED]")
            .field("model", &"[CONFIGURED]")
            .field("accepts_images", &self.accepts_images)
            .field("allow_insecure_http", &self.allow_insecure_http)
            .field("reasoning_effort", &self.reasoning_effort)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configuration(base_url: &str) -> AiConfiguration {
        AiConfiguration {
            base_url: base_url.to_string(),
            model: "test-model".to_string(),
            accepts_images: false,
            allow_insecure_http: false,
            reasoning_effort: None,
        }
    }

    #[test]
    fn builds_a_client_with_or_without_a_credential() {
        configuration("https://provider.example/v1")
            .build_client(None)
            .unwrap();
        let secret = "credential-that-must-not-appear";
        let client = configuration("https://provider.example/v1")
            .build_client(Some(secret.to_string()))
            .unwrap();
        assert!(!format!("{client:?}").contains(secret));
    }

    #[test]
    fn rejects_insecure_http_without_explicit_permission() {
        assert_eq!(
            configuration("http://localhost:11434/v1")
                .build_client(None)
                .unwrap_err(),
            ConfigurationError::InsecureHttp
        );
        let mut permitted = configuration("http://localhost:11434/v1");
        permitted.allow_insecure_http = true;
        permitted.build_client(None).unwrap();
    }

    #[test]
    fn rejects_invalid_urls_and_blank_models_without_echoing_them() {
        let secret = "url-secret-that-must-not-appear";
        let error = configuration(&format!("https://user:{secret}@provider.example/v1"))
            .build_client(None)
            .unwrap_err();
        assert_eq!(error, ConfigurationError::InvalidBaseUrl);
        assert!(!format!("{error:?}").contains(secret));

        assert_eq!(
            configuration("file:///tmp/ai")
                .build_client(None)
                .unwrap_err(),
            ConfigurationError::InvalidBaseUrl
        );
        let mut blank = configuration("https://provider.example/v1");
        blank.model = "  ".into();
        assert_eq!(
            blank.build_client(None).unwrap_err(),
            ConfigurationError::InvalidModel
        );
    }

    #[test]
    fn settings_round_trip_and_reject_unknown_fields() {
        let json = r#"{"base_url":"https://p.example/v1","model":"m","accepts_images":true}"#;
        let parsed: AiConfiguration = serde_json::from_str(json).unwrap();
        assert!(parsed.accepts_images);
        assert!(!parsed.allow_insecure_http);
        assert!(
            serde_json::from_str::<AiConfiguration>(
                r#"{"base_url":"x","model":"m","api_key":"s"}"#
            )
            .is_err()
        );
        assert!(!format!("{parsed:?}").contains("p.example"));
    }
}
