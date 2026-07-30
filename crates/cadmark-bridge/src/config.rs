//! AI provider configuration and credential resolution.

use std::fmt;

use reqwest::Url;
use serde::Deserialize;
use thiserror::Error;

use crate::openai_compatible::{Credential, OpenAiCompatibleClient};

const DEFAULT_TIMEOUT_SECONDS: u64 = 180;

/// Complete AI section of `cadmark.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiConfiguration {
    pub ai: AiSettings,
}

/// Provider-specific settings.
#[derive(Deserialize)]
#[serde(tag = "provider")]
pub enum AiSettings {
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible(OpenAiCompatibleSettings),
}

/// Settings for an OpenAI Responses-compatible endpoint.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAiCompatibleSettings {
    pub base_url: String,
    pub model: String,
    pub credential_env: Option<String>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub allow_insecure_http: bool,
}

fn default_timeout_seconds() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

/// A configuration error safe to show to the user.
#[derive(Debug, Error)]
pub enum ConfigurationError {
    #[error("AI configuration is not valid JSON: {0}")]
    Parse(String),
    #[error("unsupported AI provider `{0}`")]
    UnsupportedProvider(String),
    #[error("AI base URL `{0}` is invalid")]
    InvalidBaseUrl(String),
    #[error("AI base URL must use HTTPS unless `allow_insecure_http` is true")]
    InsecureHttp,
    #[error("AI model must not be blank")]
    InvalidModel,
    #[error("AI timeout must be greater than zero")]
    InvalidTimeout,
    #[error("AI credential environment variable name `{0}` is invalid")]
    InvalidCredentialEnvironment(String),
    #[error("AI credential environment variable `{0}` is missing or blank")]
    MissingCredential(String),
    #[error("could not construct the AI HTTP client")]
    ClientConstruction,
}

impl AiConfiguration {
    /// Parse a complete configuration file without accepting unknown fields.
    pub fn parse(contents: &str) -> Result<Self, ConfigurationError> {
        let value: serde_json::Value = serde_json::from_str(contents)
            .map_err(|error| ConfigurationError::Parse(error.to_string()))?;

        if let Some(provider) = value
            .get("ai")
            .and_then(|ai| ai.get("provider"))
            .and_then(serde_json::Value::as_str)
            && provider != "openai-compatible"
        {
            return Err(ConfigurationError::UnsupportedProvider(
                provider.to_string(),
            ));
        }

        serde_json::from_value(value).map_err(|error| ConfigurationError::Parse(error.to_string()))
    }

    /// Resolve machine-local credentials and construct the shared provider client.
    pub(crate) fn build_client_with_env<F>(
        self,
        environment: F,
    ) -> Result<OpenAiCompatibleClient, ConfigurationError>
    where
        F: FnOnce(&str) -> Option<String>,
    {
        match self.ai {
            AiSettings::OpenAiCompatible(settings) => settings.build_client(environment),
        }
    }
}

impl OpenAiCompatibleSettings {
    fn build_client<F>(self, environment: F) -> Result<OpenAiCompatibleClient, ConfigurationError>
    where
        F: FnOnce(&str) -> Option<String>,
    {
        let model = self.model.trim();
        if model.is_empty() {
            return Err(ConfigurationError::InvalidModel);
        }
        if self.timeout_seconds == 0 {
            return Err(ConfigurationError::InvalidTimeout);
        }

        let mut base_url = Url::parse(self.base_url.trim())
            .map_err(|_| ConfigurationError::InvalidBaseUrl(self.base_url.clone()))?;
        if !matches!(base_url.scheme(), "http" | "https")
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || base_url.host_str().is_none()
        {
            return Err(ConfigurationError::InvalidBaseUrl(self.base_url));
        }
        if base_url.scheme() == "http" && !self.allow_insecure_http {
            return Err(ConfigurationError::InsecureHttp);
        }

        let path = base_url.path().trim_end_matches('/');
        base_url.set_path(&format!("{path}/responses"));

        let credential = match self.credential_env {
            Some(name) => {
                if !valid_environment_name(&name) {
                    return Err(ConfigurationError::InvalidCredentialEnvironment(name));
                }
                let value = environment(&name)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| ConfigurationError::MissingCredential(name.clone()))?;
                Some(Credential::new(value))
            }
            None => None,
        };

        OpenAiCompatibleClient::new(
            base_url,
            model.to_string(),
            credential,
            self.timeout_seconds,
        )
        .map_err(|_| ConfigurationError::ClientConstruction)
    }
}

fn valid_environment_name(name: &str) -> bool {
    let mut characters = name.chars();
    matches!(characters.next(), Some('_' | 'A'..='Z' | 'a'..='z'))
        && characters.all(|character| matches!(character, '_' | 'A'..='Z' | 'a'..='z' | '0'..='9'))
}

impl fmt::Debug for AiConfiguration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiConfiguration")
            .field("ai", &self.ai)
            .finish()
    }
}

impl fmt::Debug for AiSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenAiCompatible(settings) => formatter
                .debug_tuple("OpenAiCompatible")
                .field(settings)
                .finish(),
        }
    }
}

impl fmt::Debug for OpenAiCompatibleSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiCompatibleSettings")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("credential_env", &self.credential_env)
            .field("timeout_seconds", &self.timeout_seconds)
            .field("allow_insecure_http", &self.allow_insecure_http)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(extra: &str) -> String {
        format!(
            r#"{{
                "ai": {{
                    "provider": "openai-compatible",
                    "base_url": "https://provider.example/v1",
                    "model": "test-model"
                    {extra}
                }}
            }}"#
        )
    }

    #[test]
    fn parses_unauthenticated_configuration() {
        let configuration = AiConfiguration::parse(&config("")).unwrap();
        configuration.build_client_with_env(|_| None).unwrap();
    }

    #[test]
    fn resolves_named_credential_without_exposing_it() {
        let secret = "credential-that-must-not-appear";
        let configuration =
            AiConfiguration::parse(&config(r#", "credential_env": "CADMARK_TEST_KEY""#)).unwrap();
        let client = configuration
            .build_client_with_env(|name| (name == "CADMARK_TEST_KEY").then(|| secret.to_string()))
            .unwrap();

        assert!(!format!("{client:?}").contains(secret));
    }

    #[test]
    fn rejects_unknown_and_embedded_secret_fields() {
        let unknown = AiConfiguration::parse(&config(r#", "modle": "typo""#)).unwrap_err();
        assert!(unknown.to_string().contains("unknown field"));

        let embedded = AiConfiguration::parse(&config(r#", "api_key": "secret""#)).unwrap_err();
        assert!(embedded.to_string().contains("unknown field"));
        assert!(!format!("{embedded:?}").contains("secret"));
    }

    #[test]
    fn rejects_unsupported_provider() {
        let input = config("").replace("openai-compatible", "other-provider");
        let error = AiConfiguration::parse(&input).unwrap_err();
        assert!(matches!(
            error,
            ConfigurationError::UnsupportedProvider(provider) if provider == "other-provider"
        ));
    }

    #[test]
    fn rejects_insecure_http_without_explicit_permission() {
        let input = config("").replace("https://", "http://");
        let error = AiConfiguration::parse(&input)
            .unwrap()
            .build_client_with_env(|_| None)
            .unwrap_err();
        assert!(matches!(error, ConfigurationError::InsecureHttp));
    }

    #[test]
    fn rejects_invalid_values_and_missing_credential() {
        let invalid_url = config("").replace("https://provider.example/v1", "file:///tmp/ai");
        assert!(matches!(
            AiConfiguration::parse(&invalid_url)
                .unwrap()
                .build_client_with_env(|_| None),
            Err(ConfigurationError::InvalidBaseUrl(_))
        ));

        let blank_model = config("").replace("test-model", "   ");
        assert!(matches!(
            AiConfiguration::parse(&blank_model)
                .unwrap()
                .build_client_with_env(|_| None),
            Err(ConfigurationError::InvalidModel)
        ));

        let zero_timeout = config(r#", "timeout_seconds": 0"#);
        assert!(matches!(
            AiConfiguration::parse(&zero_timeout)
                .unwrap()
                .build_client_with_env(|_| None),
            Err(ConfigurationError::InvalidTimeout)
        ));

        let missing =
            AiConfiguration::parse(&config(r#", "credential_env": "CADMARK_MISSING_KEY""#))
                .unwrap()
                .build_client_with_env(|_| None)
                .unwrap_err();
        assert!(matches!(
            missing,
            ConfigurationError::MissingCredential(name) if name == "CADMARK_MISSING_KEY"
        ));
    }

    #[test]
    fn rejects_invalid_credential_environment_name() {
        for name in ["", "1TOKEN", "TOKEN-NAME", "TOKEN VALUE"] {
            let error =
                AiConfiguration::parse(&config(&format!(r#", "credential_env": "{name}""#)))
                    .unwrap()
                    .build_client_with_env(|_| None)
                    .unwrap_err();
            assert!(matches!(
                error,
                ConfigurationError::InvalidCredentialEnvironment(_)
            ));
        }
    }
}
