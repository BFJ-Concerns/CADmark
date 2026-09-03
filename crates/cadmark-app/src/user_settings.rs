//! The list of recently opened project folders, kept in the user's
//! configuration directory so it survives restarts and applies to every
//! project.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How many folders the list keeps.
const CAPACITY: usize = 8;

/// Recently opened project folders, most recent first.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentProjects {
    #[serde(default)]
    pub folders: Vec<PathBuf>,
}

impl RecentProjects {
    /// Where the list is stored: `<config dir>/cadmark/recent-projects.json`.
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(base.join("cadmark").join("recent-projects.json"))
    }

    /// Read the list, treating a missing or unreadable file as empty.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|error| {
                log::warn!("Ignoring unreadable recent-projects list: {error}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(self).expect("recent projects serialise");
        std::fs::write(path, contents)
    }

    /// Move `folder` to the front, dropping duplicates and the oldest entry
    /// beyond capacity.
    pub fn remember(&mut self, folder: &Path) {
        let folder = folder.to_path_buf();
        self.folders.retain(|existing| existing != &folder);
        self.folders.insert(0, folder);
        self.folders.truncate(CAPACITY);
    }

    /// The list without `current`, for an "open recent" menu.
    pub fn others(&self, current: &Path) -> Vec<PathBuf> {
        self.folders
            .iter()
            .filter(|folder| folder.as_path() != current)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_moves_to_front_and_deduplicates() {
        let mut recent = RecentProjects::default();
        recent.remember(Path::new("/a"));
        recent.remember(Path::new("/b"));
        recent.remember(Path::new("/a"));
        assert_eq!(
            recent.folders,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn list_is_capped() {
        let mut recent = RecentProjects::default();
        for index in 0..20 {
            recent.remember(Path::new(&format!("/p{index}")));
        }
        assert_eq!(recent.folders.len(), CAPACITY);
        assert_eq!(recent.folders[0], PathBuf::from("/p19"));
    }

    #[test]
    fn others_excludes_the_current_project() {
        let mut recent = RecentProjects::default();
        recent.remember(Path::new("/a"));
        recent.remember(Path::new("/b"));
        assert_eq!(recent.others(Path::new("/b")), vec![PathBuf::from("/a")]);
    }

    #[test]
    fn round_trips_through_disk_and_tolerates_absence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("recent.json");
        assert_eq!(RecentProjects::load(&path), RecentProjects::default());

        let mut recent = RecentProjects::default();
        recent.remember(Path::new("/parts/bracket"));
        recent.save(&path).unwrap();
        assert_eq!(RecentProjects::load(&path), recent);

        std::fs::write(&path, "not json").unwrap();
        assert_eq!(RecentProjects::load(&path), RecentProjects::default());
    }
}
