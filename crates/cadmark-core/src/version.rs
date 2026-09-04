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
    /// Every reachable microversion for selecting a design step.
    versions: Vec<Microversion>,
    /// The ancestry-safe lane used for undo and redo, newest first.
    current_lane: Vec<Microversion>,
    /// Index of the active step within `current_lane`.
    current_lane_index: usize,
}

impl VersionHistory {
    pub fn new() -> Self {
        Self {
            versions: Vec::new(),
            current_lane: Vec::new(),
            current_lane_index: 0,
        }
    }

    pub fn from_versions(versions: Vec<Microversion>) -> Self {
        Self::from_history(versions.clone(), versions)
    }

    /// Construct history with every reachable step and the lane at HEAD.
    pub fn from_history(versions: Vec<Microversion>, current_lane: Vec<Microversion>) -> Self {
        Self {
            versions,
            current_lane,
            current_lane_index: 0,
        }
    }

    /// Add a new microversion at the head. Truncates any redo history
    /// if the user was in an undone state.
    pub fn push(&mut self, version: Microversion) {
        // If we've undone past the head, a new step starts an alternative lane.
        if self.current_lane_index > 0 {
            self.current_lane.drain(..self.current_lane_index);
        }
        self.versions.insert(0, version);
        self.current_lane.insert(0, self.versions[0].clone());
        self.current_lane_index = 0;
    }

    /// Move back one version. Returns the version to restore, or None
    /// if already at the oldest.
    pub fn undo(&mut self) -> Option<&Microversion> {
        if self.current_lane_index + 1 < self.current_lane.len() {
            self.current_lane_index += 1;
            Some(&self.current_lane[self.current_lane_index])
        } else {
            None
        }
    }

    /// Move forward one version. Returns the version to restore, or None
    /// if already at the newest.
    pub fn redo(&mut self) -> Option<&Microversion> {
        if self.current_lane_index > 0 {
            self.current_lane_index -= 1;
            Some(&self.current_lane[self.current_lane_index])
        } else {
            None
        }
    }

    /// Jump directly to a version by its index within `recent()`.
    pub fn jump_to(&mut self, index: usize) -> Option<&Microversion> {
        let selected = self.versions.get(index)?;
        if let Some(lane_index) = self
            .current_lane
            .iter()
            .position(|version| version.commit_hash == selected.commit_hash)
        {
            self.current_lane_index = lane_index;
        }
        self.versions.get(index)
    }

    pub fn can_undo(&self) -> bool {
        self.current_lane_index + 1 < self.current_lane.len()
    }

    /// The version undo would restore, without moving.
    pub fn peek_undo(&self) -> Option<&Microversion> {
        self.can_undo()
            .then(|| self.current_lane.get(self.current_lane_index + 1))
            .flatten()
    }

    /// The version redo would restore, without moving.
    pub fn peek_redo(&self) -> Option<&Microversion> {
        self.can_redo()
            .then(|| self.current_lane.get(self.current_lane_index - 1))
            .flatten()
    }

    /// Index of the active version within `recent()`.
    pub fn current_index(&self) -> usize {
        self.current()
            .and_then(|current| {
                self.versions
                    .iter()
                    .position(|version| version.commit_hash == current.commit_hash)
            })
            .unwrap_or(0)
    }

    pub fn can_redo(&self) -> bool {
        self.current_lane_index > 0
    }

    /// The currently active version.
    pub fn current(&self) -> Option<&Microversion> {
        self.current_lane.get(self.current_lane_index)
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

    #[test]
    fn peeking_reports_the_neighbours_without_moving() {
        let mut history = VersionHistory::new();
        history.push(make_version("a", "first"));
        history.push(make_version("b", "second"));
        history.push(make_version("c", "third"));
        assert_eq!(history.peek_undo().unwrap().summary, "second");
        assert!(history.peek_redo().is_none());
        assert_eq!(history.current_index(), 0);

        history.undo();
        assert_eq!(history.current_index(), 1);
        assert_eq!(history.peek_undo().unwrap().summary, "first");
        assert_eq!(history.peek_redo().unwrap().summary, "third");
    }

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
    fn push_after_undo_keeps_the_abandoned_step_reachable() {
        let mut history = VersionHistory::new();
        history.push(make_version("aaa", "first"));
        history.push(make_version("bbb", "second"));
        history.push(make_version("ccc", "third"));

        // Undo back to "bbb".
        history.undo();
        assert_eq!(history.current().unwrap().commit_hash, "bbb");

        // Push a new version — "ccc" remains reachable as an alternative.
        history.push(make_version("ddd", "branch"));
        assert_eq!(history.len(), 4); // ddd, ccc, bbb, aaa
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

    #[test]
    fn jump_to_selects_requested_version() {
        let mut history = VersionHistory::new();
        history.push(make_version("aaa", "first"));
        history.push(make_version("bbb", "second"));
        history.push(make_version("ccc", "third"));

        let jumped = history.jump_to(2).unwrap();
        assert_eq!(jumped.commit_hash, "aaa");
        assert!(!history.can_undo());
        assert!(history.can_redo());
    }

    #[test]
    fn from_versions_starts_at_latest() {
        let history = VersionHistory::from_versions(vec![
            make_version("ccc", "third"),
            make_version("bbb", "second"),
            make_version("aaa", "first"),
        ]);

        assert_eq!(history.current().unwrap().commit_hash, "ccc");
        assert!(history.can_undo());
        assert!(!history.can_redo());
    }

    #[test]
    fn alternatives_are_visible_without_becoming_undo_neighbours() {
        let base = make_version("base", "base");
        let active = make_version("active", "active lane");
        let alternative = make_version("alternative", "abandoned alternative");
        let mut history = VersionHistory::from_history(
            vec![alternative.clone(), active.clone(), base.clone()],
            vec![active, base],
        );

        assert_eq!(history.recent(3).len(), 3);
        assert_eq!(history.current().unwrap().commit_hash, "active");
        assert_eq!(history.undo().unwrap().commit_hash, "base");
        assert!(!history.can_undo());
        assert_ne!(
            history.current().unwrap().commit_hash,
            alternative.commit_hash
        );
    }
}
