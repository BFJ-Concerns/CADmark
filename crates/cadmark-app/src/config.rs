//! Application-owned configuration discovery and service construction.

use std::path::{Path, PathBuf};

use cadmark_bridge::AiServices;
use cadmark_bridge::config::{AiConfiguration, ConfigurationError};
use thiserror::Error;

pub const CONFIG_PATH_ENV: &str = "CADMARK_CONFIG";

#[derive(Debug, Error)]
pub enum AppConfigurationError {
    #[error("AI configuration file `{0}` was not found")]
    NotFound(PathBuf),
    #[error("could not read AI configuration file `{path}`: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("AI configuration file `{path}` is invalid: {source}")]
    Invalid {
        path: PathBuf,
        source: ConfigurationError,
    },
}

pub fn load_ai_services(project_dir: &Path) -> Result<AiServices, AppConfigurationError> {
    load_ai_services_with_env(project_dir, |name| std::env::var(name).ok())
}

fn load_ai_services_with_env<F>(
    project_dir: &Path,
    environment: F,
) -> Result<AiServices, AppConfigurationError>
where
    F: Fn(&str) -> Option<String>,
{
    let path = environment(CONFIG_PATH_ENV)
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| project_dir.join("cadmark.json"));

    let contents = std::fs::read_to_string(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            AppConfigurationError::NotFound(path.clone())
        } else {
            AppConfigurationError::Read {
                path: path.clone(),
                source,
            }
        }
    })?;
    let configuration =
        AiConfiguration::parse(&contents).map_err(|source| AppConfigurationError::Invalid {
            path: path.clone(),
            source,
        })?;
    cadmark_bridge::build_ai_services_with_env(configuration, |name| environment(name)).map_err(
        |source| AppConfigurationError::Invalid {
            path: path.clone(),
            source,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configuration(model: &str) -> String {
        format!(
            r#"{{
                "ai": {{
                    "provider": "openai-compatible",
                    "base_url": "https://provider.example/v1",
                    "model": "{model}"
                }}
            }}"#
        )
    }

    #[test]
    fn uses_project_configuration_when_explicit_path_is_absent() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("cadmark.json"),
            configuration("project-model"),
        )
        .unwrap();

        let services = load_ai_services_with_env(project.path(), |_| None).unwrap();
        assert!(format!("{:?}", services.model_edit).contains("project-model"));
    }

    #[test]
    fn explicit_path_has_whole_file_precedence() {
        let project = tempfile::tempdir().unwrap();
        let explicit = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            project.path().join("cadmark.json"),
            configuration("project-model"),
        )
        .unwrap();
        std::fs::write(explicit.path(), configuration("explicit-model")).unwrap();

        let explicit_path = explicit.path().display().to_string();
        let services = load_ai_services_with_env(project.path(), |name| {
            (name == CONFIG_PATH_ENV).then(|| explicit_path.clone())
        })
        .unwrap();
        assert!(format!("{:?}", services.model_edit).contains("explicit-model"));
    }

    #[test]
    fn broken_explicit_path_does_not_fall_back() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("cadmark.json"),
            configuration("project-model"),
        )
        .unwrap();
        let missing = project.path().join("missing.json");
        let missing_text = missing.display().to_string();

        let error = load_ai_services_with_env(project.path(), |name| {
            (name == CONFIG_PATH_ENV).then(|| missing_text.clone())
        })
        .unwrap_err();
        assert!(matches!(
            error,
            AppConfigurationError::NotFound(path) if path == missing
        ));
    }

    #[test]
    fn reports_missing_project_configuration() {
        let project = tempfile::tempdir().unwrap();
        let error = load_ai_services_with_env(project.path(), |_| None).unwrap_err();
        assert!(matches!(error, AppConfigurationError::NotFound(_)));
        assert!(error.to_string().contains("cadmark.json"));
    }

    #[tokio::test]
    #[ignore = "requires the configured live gateway and machine-local credential"]
    async fn live_configured_provider_returns_exact_sentinel() {
        let project_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let services = load_ai_services(&project_dir).unwrap();
        services
            .smoke_test_exact_sentinel("CADMARK_SOL_PROVIDER_OK")
            .await
            .unwrap();
    }
}
