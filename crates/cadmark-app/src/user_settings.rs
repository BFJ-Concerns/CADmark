//! What CADmark remembers about the user across projects, in the user's
//! configuration directory: the AI provider, the execution ceilings, and
//! the recently opened folders. No project folder needs a configuration
//! file for any of it.
//!
//! The credential is a separate file with owner-only permissions; the
//! settings file never carries it. An environment variable, when set, is
//! honoured over the stored value so a machine-wide key or a one-off run
//! needs nothing written.

use std::path::{Path, PathBuf};

use cadmark_bridge::config::AiConfiguration;
use cadmark_core::limits::ExecutionLimits;
use serde::{Deserialize, Serialize};

/// How many recent folders are kept.
const RECENT_CAPACITY: usize = 8;

/// The environment variable that overrides the stored credential.
pub const CREDENTIAL_ENV: &str = "CADMARK_AI_API_KEY";

const SETTINGS_FILE: &str = "settings.json";
const CREDENTIAL_FILE: &str = "credential";

/// A conservative default for contemporary provider context windows. The
/// provider setting is editable because compatible endpoints do not expose a
/// shared context-window capability on the wire.
pub const DEFAULT_CONTEXT_WINDOW_TOKENS: usize = 128_000;

/// Everything in the settings file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserSettings {
    /// The provider, absent until the user configures one.
    pub ai: Option<AiConfiguration>,
    pub limits: ExecutionLimits,
    /// The configured model's context window, used for early condensation.
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: usize,
    /// Recently opened project folders, most recent first.
    pub recent_projects: Vec<PathBuf>,
}

fn default_context_window_tokens() -> usize {
    DEFAULT_CONTEXT_WINDOW_TOKENS
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            ai: None,
            limits: ExecutionLimits::default(),
            context_window_tokens: default_context_window_tokens(),
            recent_projects: Vec::new(),
        }
    }
}

impl UserSettings {
    /// Move `folder` to the front of the recent list, dropping duplicates
    /// and the oldest entry beyond capacity.
    pub fn remember_project(&mut self, folder: &Path) {
        let folder = folder.to_path_buf();
        self.recent_projects.retain(|existing| existing != &folder);
        self.recent_projects.insert(0, folder);
        self.recent_projects.truncate(RECENT_CAPACITY);
    }

    /// Drop `folder` from the recent list — the start view offers it, and
    /// it is no longer a folder to open.
    pub fn forget_project(&mut self, folder: &Path) {
        self.recent_projects.retain(|existing| existing != folder);
    }

    /// The recent list without `current`, for an "open recent" menu.
    pub fn other_recent_projects(&self, current: &Path) -> Vec<PathBuf> {
        self.recent_projects
            .iter()
            .filter(|folder| folder.as_path() != current)
            .cloned()
            .collect()
    }
}

/// Where the settings live. The directory is created on first save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsStore {
    dir: PathBuf,
}

impl SettingsStore {
    /// `$XDG_CONFIG_HOME/cadmark`, falling back to `~/.config/cadmark`.
    pub fn default_location() -> Option<Self> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(Self::at(base.join("cadmark")))
    }

    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Read the settings, treating a missing file as the defaults. An
    /// unreadable file is reported so the user can fix it rather than
    /// silently losing their provider.
    pub fn load(&self) -> Result<UserSettings, String> {
        let path = self.dir.join(SETTINGS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(contents) => {
                let mut deserializer = serde_json::Deserializer::from_str(&contents);
                let settings =
                    serde_path_to_error::deserialize::<_, UserSettings>(&mut deserializer)
                        .map_err(|error| {
                            let field = known_settings_field(&error.path().to_string());
                            let parse_error = error.into_inner();
                            invalid_settings_error(&path, field, &parse_error)
                        })?;
                deserializer
                    .end()
                    .map_err(|error| invalid_settings_error(&path, None, &error))?;
                Ok(settings)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(UserSettings::default())
            }
            Err(error) => Err(format!("could not read {}: {error}", path.display())),
        }
    }

    pub fn save(&self, settings: &UserSettings) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let contents = serde_json::to_string_pretty(settings).expect("settings serialise");
        std::fs::write(self.dir.join(SETTINGS_FILE), contents)
    }

    /// The credential the provider is called with: the environment
    /// variable when set, otherwise the stored file, otherwise none.
    pub fn credential(&self) -> Option<String> {
        self.credential_with_env(|name| std::env::var(name).ok())
    }

    fn credential_with_env(&self, environment: impl Fn(&str) -> Option<String>) -> Option<String> {
        if let Some(value) = environment(CREDENTIAL_ENV).filter(|value| !value.trim().is_empty()) {
            return Some(value);
        }
        std::fs::read_to_string(self.dir.join(CREDENTIAL_FILE))
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    /// Store the credential in a file only the user can read. An empty
    /// value removes the file.
    pub fn save_credential(&self, credential: &str) -> std::io::Result<()> {
        let path = self.dir.join(CREDENTIAL_FILE);
        if credential.trim().is_empty() {
            return match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            };
        }
        std::fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)?;
            file.write_all(credential.trim().as_bytes())?;
            // The mode above applies only on creation; an existing file
            // keeps whatever it had, so set it explicitly.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&path, credential.trim())
        }
    }

    /// Whether a credential is stored on disk (as opposed to supplied by
    /// the environment), for the settings dialog.
    pub fn has_stored_credential(&self) -> bool {
        self.dir.join(CREDENTIAL_FILE).is_file()
    }
}

fn known_settings_field(path: &str) -> Option<&'static str> {
    match path {
        "ai" => Some("ai"),
        "ai.base_url" => Some("ai.base_url"),
        "ai.model" => Some("ai.model"),
        "ai.accepts_images" => Some("ai.accepts_images"),
        "ai.allow_insecure_http" => Some("ai.allow_insecure_http"),
        "limits" => Some("limits"),
        "limits.wall_clock" | "limits.wall_clock.secs" | "limits.wall_clock.nanos" => {
            Some("limits.wall_clock")
        }
        "limits.memory_bytes" => Some("limits.memory_bytes"),
        "context_window_tokens" => Some("context_window_tokens"),
        path if path.starts_with("recent_projects[") => Some("recent_projects"),
        "recent_projects" => Some("recent_projects"),
        _ => None,
    }
}

fn invalid_settings_error(path: &Path, field: Option<&str>, error: &serde_json::Error) -> String {
    let location = format!("line {}, column {}", error.line(), error.column());
    match error.classify() {
        serde_json::error::Category::Data => match field {
            Some(field) => format!(
                "{} has an invalid value for {field} near {location}; correct the field type and try again",
                path.display(),
            ),
            None => format!(
                "{} contains an invalid setting near {location}; correct the field type and try again",
                path.display(),
            ),
        },
        serde_json::error::Category::Syntax | serde_json::error::Category::Eof => match field {
            Some(field) => format!(
                "{} has invalid JSON for {field} near {location}; correct the JSON syntax and try again",
                path.display(),
            ),
            None => format!(
                "{} contains invalid settings near {location}; correct the JSON syntax and try again",
                path.display(),
            ),
        },
        serde_json::error::Category::Io => format!(
            "{} contains invalid settings near {location}; correct the JSON syntax and try again",
            path.display(),
        ),
    }
}

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SettingsStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::at(dir.path().join("nested").join("cadmark"));
        (dir, store)
    }

    #[test]
    fn missing_settings_are_the_defaults_and_round_trip_once_saved() {
        let (_dir, store) = store();
        assert_eq!(store.load().unwrap(), UserSettings::default());

        let mut settings = UserSettings {
            ai: Some(AiConfiguration {
                base_url: "https://provider.example/v1".into(),
                model: "m".into(),
                accepts_images: true,
                allow_insecure_http: false,
            }),
            ..UserSettings::default()
        };
        settings.limits.wall_clock = std::time::Duration::from_secs(30);
        settings.remember_project(Path::new("/parts/bracket"));
        store.save(&settings).unwrap();
        assert_eq!(store.load().unwrap(), settings);
    }

    #[test]
    fn an_unreadable_settings_file_is_reported_not_replaced() {
        let (_dir, store) = store();
        store.save(&UserSettings::default()).unwrap();
        std::fs::write(store.dir.join(SETTINGS_FILE), "not json").unwrap();
        assert!(store.load().unwrap_err().contains(SETTINGS_FILE));
    }

    #[test]
    fn malformed_settings_are_actionable_without_echoing_their_contents() {
        let (_dir, store) = store();
        let cases = [
            (
                r#"{"context_window_tokens":"wrong-type-sentinel"}"#,
                "wrong-type-sentinel",
                Some("context_window_tokens"),
                "field type",
            ),
            (
                r#"{"unknown-key-sentinel":[1,}"#,
                "unknown-key-sentinel",
                None,
                "JSON syntax",
            ),
            (
                r#"{"ai":{"base_url":"https://provider.example/v1","model":"m","unknown-data-key-sentinel":"value"}}"#,
                "unknown-data-key-sentinel",
                None,
                "field type",
            ),
            (
                r#"{"context_window_tokens":"malformed-literal-sentinel"#,
                "malformed-literal-sentinel",
                Some("context_window_tokens"),
                "JSON syntax",
            ),
            (
                r#"{"context_window_tokens":128000} trailing-junk-sentinel"#,
                "trailing-junk-sentinel",
                None,
                "JSON syntax",
            ),
        ];

        for (contents, sentinel, field, repair) in cases {
            std::fs::create_dir_all(&store.dir).unwrap();
            std::fs::write(store.dir.join(SETTINGS_FILE), contents).unwrap();

            let error = store.load().unwrap_err();

            assert!(error.contains("settings.json"));
            assert!(!error.contains(sentinel));
            assert!(error.contains("line 1"));
            assert!(error.contains("correct"));
            assert!(error.contains(repair));
            if let Some(field) = field {
                assert!(error.contains(field));
            }
        }
    }

    #[test]
    fn the_credential_file_is_owner_only_and_the_environment_wins() {
        let (_dir, store) = store();
        assert_eq!(store.credential_with_env(|_| None), None);

        store.save_credential("  stored-secret \n").unwrap();
        assert!(store.has_stored_credential());
        let mode = std::fs::metadata(store.dir.join(CREDENTIAL_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            store.credential_with_env(|_| None).as_deref(),
            Some("stored-secret")
        );
        assert_eq!(
            store
                .credential_with_env(|name| (name == CREDENTIAL_ENV).then(|| "env-secret".into()))
                .as_deref(),
            Some("env-secret")
        );

        store.save_credential("").unwrap();
        assert!(!store.has_stored_credential());
        assert_eq!(store.credential_with_env(|_| None), None);
    }

    #[test]
    fn the_real_environment_credential_wins_without_changing_the_test_process() {
        const CHILD: &str = "CADMARK_PROVIDER_SETTINGS_ENV_CHILD";
        const STORE: &str = "CADMARK_PROVIDER_SETTINGS_ENV_STORE";
        const EXPECTED: &str = "test-only-environment-token";

        if std::env::var_os(CHILD).is_some() {
            let store = SettingsStore::at(std::env::var_os(STORE).map(PathBuf::from).unwrap());
            assert_eq!(store.credential().as_deref(), Some(EXPECTED));
            return;
        }

        let (directory, store) = store();
        store.save_credential("test-only-stored-token").unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "user_settings::tests::the_real_environment_credential_wins_without_changing_the_test_process"])
            .env(CHILD, "1")
            .env(STORE, &store.dir)
            .env(CREDENTIAL_ENV, EXPECTED)
            .status()
            .unwrap();
        assert!(status.success());
        drop(directory);
    }

    #[test]
    fn recent_projects_move_to_the_front_and_are_capped() {
        let mut settings = UserSettings::default();
        settings.remember_project(Path::new("/a"));
        settings.remember_project(Path::new("/b"));
        settings.remember_project(Path::new("/a"));
        assert_eq!(
            settings.recent_projects,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
        assert_eq!(
            settings.other_recent_projects(Path::new("/a")),
            vec![PathBuf::from("/b")]
        );
        for index in 0..20 {
            settings.remember_project(Path::new(&format!("/p{index}")));
        }
        assert_eq!(settings.recent_projects.len(), RECENT_CAPACITY);

        // A folder the start view offered but could not open leaves the
        // list, and the rest of it is untouched.
        let remaining = settings.recent_projects.len();
        let gone = settings.recent_projects[1].clone();
        settings.forget_project(&gone);
        assert!(!settings.recent_projects.contains(&gone));
        assert_eq!(settings.recent_projects.len(), remaining - 1);
    }
}
