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
            Ok(contents) => serde_json::from_str(&contents)
                .map_err(|error| format!("{} is not readable: {error}", path.display())),
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
        assert!(store.load().unwrap_err().contains("not readable"));
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
    }
}
