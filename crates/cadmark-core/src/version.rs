// Microversion metadata — structured commit information for undo/redo.
//
// Each AI operation creates a microversion (git commit). User-created
// named versions (snapshots) mark meaningful design states.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A microversion in the design history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Microversion {
    /// Git commit hash (short form).
    pub commit_hash: String,
    /// One-line AI-generated summary of the change.
    pub summary: String,
    /// The user message that triggered this change (truncated for display).
    pub trigger_message: String,
    pub timestamp: DateTime<Utc>,
    /// Whether this is a user-created named snapshot.
    pub snapshot: Option<SnapshotInfo>,
}

/// Metadata for a user-created named version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub name: String,
}

/// Navigation state through the version history.
#[derive(Debug)]
pub struct VersionHistory {
    /// All microversions, most recent first.
    versions: Vec<Microversion>,
    /// Index of the currently active version (0 = most recent).
    current_index: usize,
}

impl VersionHistory {
    pub fn new() -> Self {
        Self {
            versions: Vec::new(),
            current_index: 0,
        }
    }

    /// Add a new microversion at the head. Truncates any redo history
    /// if the user was in an undone state.
    pub fn push(&mut self, version: Microversion) {
        // If we've undone past the head, discard the undone versions.
        if self.current_index > 0 {
            self.versions.drain(..self.current_index);
            self.current_index = 0;
        }
        self.versions.insert(0, version);
    }

    /// Move back one version. Returns the version to restore, or None
    /// if already at the oldest.
    pub fn undo(&mut self) -> Option<&Microversion> {
        if self.current_index + 1 < self.versions.len() {
            self.current_index += 1;
            Some(&self.versions[self.current_index])
        } else {
            None
        }
    }

    /// Move forward one version. Returns the version to restore, or None
    /// if already at the newest.
    pub fn redo(&mut self) -> Option<&Microversion> {
        if self.current_index > 0 {
            self.current_index -= 1;
            Some(&self.versions[self.current_index])
        } else {
            None
        }
    }

    pub fn can_undo(&self) -> bool {
        self.current_index + 1 < self.versions.len()
    }

    pub fn can_redo(&self) -> bool {
        self.current_index > 0
    }

    /// The currently active version.
    pub fn current(&self) -> Option<&Microversion> {
        self.versions.get(self.current_index)
    }

    /// Recent versions for the undo dropdown (most recent first).
    pub fn recent(&self, count: usize) -> &[Microversion] {
        let end = count.min(self.versions.len());
        &self.versions[..end]
    }

    pub fn len(&self) -> usize {
        self.versions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }
}

impl Default for VersionHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_version(hash: &str, summary: &str) -> Microversion {
        Microversion {
            commit_hash: hash.to_string(),
            summary: summary.to_string(),
            trigger_message: "test".to_string(),
            timestamp: Utc::now(),
            snapshot: None,
        }
    }

    #[test]
    fn empty_history_cannot_undo_or_redo() {
        let history = VersionHistory::new();
        assert!(!history.can_undo());
        assert!(!history.can_redo());
        assert!(history.is_empty());
    }

    #[test]
    fn push_and_undo() {
        let mut history = VersionHistory::new();
        history.push(make_version("aaa", "first"));
        history.push(make_version("bbb", "second"));
        history.push(make_version("ccc", "third"));

        assert_eq!(history.current().unwrap().commit_hash, "ccc");
        assert!(history.can_undo());
        assert!(!history.can_redo());

        let undone = history.undo().unwrap();
        assert_eq!(undone.commit_hash, "bbb");
        assert!(history.can_redo());

        let undone2 = history.undo().unwrap();
        assert_eq!(undone2.commit_hash, "aaa");
        assert!(!history.can_undo()); // At the oldest.
    }

    #[test]
    fn redo_after_undo() {
        let mut history = VersionHistory::new();
        history.push(make_version("aaa", "first"));
        history.push(make_version("bbb", "second"));

        history.undo();
        let redone = history.redo().unwrap();
        assert_eq!(redone.commit_hash, "bbb");
        assert!(!history.can_redo());
    }

    #[test]
    fn push_after_undo_truncates_redo_history() {
        let mut history = VersionHistory::new();
        history.push(make_version("aaa", "first"));
        history.push(make_version("bbb", "second"));
        history.push(make_version("ccc", "third"));

        // Undo back to "bbb".
        history.undo();
        assert_eq!(history.current().unwrap().commit_hash, "bbb");

        // Push a new version — "ccc" should be gone.
        history.push(make_version("ddd", "branch"));
        assert_eq!(history.len(), 3); // ddd, bbb, aaa
        assert_eq!(history.current().unwrap().commit_hash, "ddd");
        assert!(!history.can_redo());
    }

    #[test]
    fn recent_returns_correct_slice() {
        let mut history = VersionHistory::new();
        history.push(make_version("aaa", "first"));
        history.push(make_version("bbb", "second"));
        history.push(make_version("ccc", "third"));

        let recent = history.recent(2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].commit_hash, "ccc");
        assert_eq!(recent[1].commit_hash, "bbb");
    }
}
