// Git operations — microversion commits, undo/redo checkout, branch management.
//
// CADmark owns the git repository. Each AI modification creates a
// microversion commit with structured metadata in the commit message.

use std::path::Path;
use std::process::Command;

use thiserror::Error;

use cadmark_core::version::{Microversion, SnapshotInfo};

#[derive(Error, Debug)]
pub enum GitError {
    #[error("Git command failed: {0}")]
    CommandFailed(String),
    #[error("Not a git repository: {0}")]
    NotARepo(String),
    #[error("Failed to parse git output: {0}")]
    ParseError(String),
}

/// Metadata separator in commit messages.
/// Structured fields follow this marker.
const METADATA_MARKER: &str = "---cadmark---";

/// Initialise a git repository in the project directory if one doesn't exist.
pub fn ensure_repo(project_dir: &Path) -> Result<(), GitError> {
    if project_dir.join(".git").exists() {
        return Ok(());
    }

    run_git(project_dir, &["init"])?;

    // Set a local fallback identity so commits work even on machines
    // without a global git config. Harmless if one already exists —
    // local config just overrides for this repo.
    run_git(project_dir, &["config", "user.name", "CADmark"])?;
    run_git(project_dir, &["config", "user.email", "cadmark@local"])?;

    Ok(())
}

/// Create a microversion commit after a successful AI edit.
///
/// The commit message format:
/// ```text
/// <summary>
///
/// ---cadmark---
/// trigger: <user message>
/// type: microversion
/// ```
pub fn create_microversion(
    project_dir: &Path,
    summary: &str,
    trigger_message: &str,
    script_filename: &str,
) -> Result<Microversion, GitError> {
    // If HEAD is detached (e.g. after undo navigation), create a branch
    // so the new commit isn't orphaned. The branch name encodes the short
    // hash we diverged from and an attempt number, so revisiting the same
    // design step creates a distinct alternative rather than colliding.
    if is_head_detached(project_dir)? {
        let base = run_git(project_dir, &["rev-parse", "--short", "HEAD"])?;
        let branch_name = next_edit_branch_name(project_dir, base.trim())?;
        run_git(project_dir, &["checkout", "-b", &branch_name])?;
    }

    // Stage the script file.
    run_git(project_dir, &["add", script_filename])?;

    // Build the structured commit message.
    let message =
        format!("{summary}\n\n{METADATA_MARKER}\ntrigger: {trigger_message}\ntype: microversion");

    run_git(project_dir, &["commit", "-m", &message])?;

    // Get the commit hash.
    let hash = run_git(project_dir, &["rev-parse", "--short", "HEAD"])?;
    let hash = hash.trim().to_string();

    Ok(Microversion {
        commit_hash: hash,
        summary: summary.to_string(),
        trigger_message: trigger_message.to_string(),
        timestamp: chrono::Utc::now(),
        snapshot: None,
    })
}

/// Create a named snapshot (user-initiated save point).
pub fn create_snapshot(
    project_dir: &Path,
    name: &str,
    script_filename: &str,
) -> Result<Microversion, GitError> {
    run_git(project_dir, &["add", script_filename])?;

    let message = format!("Snapshot: {name}\n\n{METADATA_MARKER}\ntype: snapshot\nname: {name}");

    run_git(project_dir, &["commit", "-m", &message, "--allow-empty"])?;

    let hash = run_git(project_dir, &["rev-parse", "--short", "HEAD"])?;
    let hash = hash.trim().to_string();

    Ok(Microversion {
        commit_hash: hash,
        summary: format!("Snapshot: {name}"),
        trigger_message: String::new(),
        timestamp: chrono::Utc::now(),
        snapshot: Some(SnapshotInfo {
            name: name.to_string(),
        }),
    })
}

/// Checkout a specific commit (for undo/redo). Uses detached HEAD
/// to avoid branch confusion — the working branch pointer stays where it is.
pub fn checkout_commit(project_dir: &Path, commit_hash: &str) -> Result<(), GitError> {
    run_git(project_dir, &["checkout", commit_hash, "--detach"])?;
    Ok(())
}

/// Return to the tip of the current branch (after undo navigation).
pub fn checkout_branch_tip(project_dir: &Path, branch: &str) -> Result<(), GitError> {
    run_git(project_dir, &["checkout", branch])?;
    Ok(())
}

/// List recent microversions from the current lane and CADmark-owned alternatives.
pub fn list_microversions(project_dir: &Path, count: usize) -> Result<Vec<Microversion>, GitError> {
    let mut refs = vec!["HEAD".to_string()];
    refs.extend(design_history_alternative_refs(project_dir)?);
    list_microversions_from_refs(project_dir, count, &refs)
}

/// List the microversions on the lane currently checked out by the user.
pub fn list_current_lane_microversions(
    project_dir: &Path,
    count: usize,
) -> Result<Vec<Microversion>, GitError> {
    list_microversions_from_refs(project_dir, count, &["HEAD".to_string()])
}

/// Return the local branch refs CADmark creates for edit-after-undo alternatives.
fn design_history_alternative_refs(project_dir: &Path) -> Result<Vec<String>, GitError> {
    let output = run_git(
        project_dir,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads/cadmark-edit-*",
        ],
    )?;
    Ok(output.lines().map(str::to_owned).collect())
}

fn list_microversions_from_refs(
    project_dir: &Path,
    count: usize,
    refs: &[String],
) -> Result<Vec<Microversion>, GitError> {
    // Use null byte as record separator — it cannot appear in commit text,
    // unlike the old "---END---" delimiter which could collide with user input.
    let mut args = vec![
        "log".to_string(),
        format!("-{count}"),
        "--format=%H%n%s%n%aI%n%b%x00".to_string(),
    ];
    args.extend(refs.iter().cloned());
    let arg_refs: Vec<_> = args.iter().map(String::as_str).collect();
    let log_output = run_git(project_dir, &arg_refs)?;

    let mut versions = Vec::new();

    for entry in log_output.split('\0') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }

        let lines: Vec<&str> = entry.lines().collect();
        if lines.len() < 3 {
            continue;
        }

        let hash = lines[0][..7.min(lines[0].len())].to_string();
        let summary = lines[1].to_string();
        let timestamp = chrono::DateTime::parse_from_rfc3339(lines[2])
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        // Parse structured metadata if present.
        let body = lines[3..].join("\n");
        let trigger_message = body
            .lines()
            .find(|l| l.starts_with("trigger: "))
            .map(|l| l.trim_start_matches("trigger: ").to_string())
            .unwrap_or_default();

        let is_snapshot = body.lines().any(|l| l.trim() == "type: snapshot");
        let snapshot = if is_snapshot {
            body.lines()
                .find(|l| l.starts_with("name: "))
                .map(|l| SnapshotInfo {
                    name: l.trim_start_matches("name: ").to_string(),
                })
        } else {
            None
        };

        versions.push(Microversion {
            commit_hash: hash,
            summary,
            trigger_message,
            timestamp,
            snapshot,
        });
    }

    Ok(versions)
}

/// Create a new branch from the current HEAD for exploring alternatives.
pub fn create_branch(project_dir: &Path, name: &str) -> Result<(), GitError> {
    run_git(project_dir, &["checkout", "-b", name])?;
    Ok(())
}

/// Switch to an existing branch.
pub fn switch_branch(project_dir: &Path, name: &str) -> Result<(), GitError> {
    run_git(project_dir, &["checkout", name])?;
    Ok(())
}

/// Check whether HEAD is detached (not on any branch).
fn is_head_detached(project_dir: &Path) -> Result<bool, GitError> {
    let output = run_git(project_dir, &["symbolic-ref", "-q", "HEAD"]);
    match output {
        Ok(_) => Ok(false),
        // symbolic-ref exits non-zero when HEAD is detached — that's expected.
        Err(GitError::CommandFailed(_)) => Ok(true),
        Err(e) => Err(e),
    }
}

/// Name the next alternative branch from `base` without reusing an existing
/// design-edit branch.
fn next_edit_branch_name(project_dir: &Path, base: &str) -> Result<String, GitError> {
    let branches = list_branches(project_dir)?;
    for attempt in 1.. {
        let candidate = format!("cadmark-edit-{base}-{attempt}");
        if !branches.iter().any(|branch| branch == &candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("an unbounded branch-name sequence must find a free name")
}

/// Get the current branch name (or "HEAD" if detached).
pub fn current_branch(project_dir: &Path) -> Result<String, GitError> {
    let output = run_git(project_dir, &["branch", "--show-current"])?;
    let branch = output.trim().to_string();
    if branch.is_empty() {
        Ok("HEAD".to_string())
    } else {
        Ok(branch)
    }
}

/// List all branches.
pub fn list_branches(project_dir: &Path) -> Result<Vec<String>, GitError> {
    let output = run_git(project_dir, &["branch", "--format=%(refname:short)"])?;
    Ok(output.lines().map(|l| l.trim().to_string()).collect())
}

/// Run a git command and return stdout.
fn run_git(project_dir: &Path, args: &[&str]) -> Result<String, GitError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(project_dir)
        .output()
        .map_err(|e| GitError::CommandFailed(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GitError::CommandFailed(format!(
            "git {} failed: {stderr}",
            args.join(" ")
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a temporary directory with a git repo for testing.
    fn test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init"]).unwrap();
        // Configure git user for commits.
        run_git(dir.path(), &["config", "user.email", "test@test.com"]).unwrap();
        run_git(dir.path(), &["config", "user.name", "Test"]).unwrap();
        dir
    }

    #[test]
    fn ensure_repo_sets_fallback_identity() {
        let dir = tempfile::tempdir().unwrap();
        ensure_repo(dir.path()).unwrap();

        let name = run_git(dir.path(), &["config", "user.name"]).unwrap();
        let email = run_git(dir.path(), &["config", "user.email"]).unwrap();
        assert_eq!(name.trim(), "CADmark");
        assert_eq!(email.trim(), "cadmark@local");

        // First commit should succeed without external identity.
        fs::write(dir.path().join("test.txt"), "hello").unwrap();
        run_git(dir.path(), &["add", "test.txt"]).unwrap();
        run_git(dir.path(), &["commit", "-m", "initial"]).unwrap();
    }

    #[test]
    fn create_and_list_microversions() {
        let dir = test_repo();
        let script = "part.py";
        fs::write(dir.path().join(script), "box = Box(10, 10, 10)").unwrap();

        let v1 =
            create_microversion(dir.path(), "Create initial box", "make a box", script).unwrap();
        assert!(!v1.commit_hash.is_empty());

        fs::write(dir.path().join(script), "box = Box(20, 20, 20)").unwrap();
        let v2 =
            create_microversion(dir.path(), "Double box size", "make it bigger", script).unwrap();

        let versions = list_microversions(dir.path(), 10).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].summary, "Double box size");
        assert_eq!(versions[0].trigger_message, "make it bigger");
        assert_eq!(versions[1].summary, "Create initial box");

        assert_eq!(v2.commit_hash, versions[0].commit_hash);
    }

    #[test]
    fn completed_turn_leaves_the_working_script_at_its_recorded_step() {
        let dir = test_repo();
        let script = "part.py";
        let recorded_script = "box = Box(10, 10, 10)";
        fs::write(dir.path().join(script), recorded_script).unwrap();

        let step = create_microversion(dir.path(), "Create box", "make a box", script).unwrap();

        let committed_script = run_git(
            dir.path(),
            &["show", &format!("{}:{script}", step.commit_hash)],
        )
        .unwrap();
        assert_eq!(committed_script.trim(), recorded_script);
        assert_eq!(
            fs::read_to_string(dir.path().join(script)).unwrap(),
            committed_script
        );
    }

    #[test]
    fn undo_and_redo_restore_the_recorded_script() {
        let dir = test_repo();
        let script = "part.py";
        let branch = current_branch(dir.path()).unwrap();
        fs::write(dir.path().join(script), "box = Box(10, 10, 10)").unwrap();
        let first = create_microversion(dir.path(), "Create box", "make a box", script).unwrap();
        fs::write(dir.path().join(script), "box = Box(20, 20, 20)").unwrap();
        let second = create_microversion(dir.path(), "Widen box", "make it wider", script).unwrap();

        checkout_commit(dir.path(), &first.commit_hash).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(script)).unwrap(),
            "box = Box(10, 10, 10)"
        );

        checkout_commit(dir.path(), &second.commit_hash).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(script)).unwrap(),
            "box = Box(20, 20, 20)"
        );

        checkout_branch_tip(dir.path(), &branch).unwrap();
        assert_eq!(current_branch(dir.path()).unwrap(), branch);
    }

    #[test]
    fn create_snapshot() {
        let dir = test_repo();
        let script = "part.py";
        fs::write(dir.path().join(script), "box = Box(10, 10, 10)").unwrap();

        let snapshot = super::create_snapshot(dir.path(), "Design v1", script).unwrap();
        assert!(snapshot.snapshot.is_some());
        assert_eq!(snapshot.snapshot.unwrap().name, "Design v1");

        let versions = list_microversions(dir.path(), 10).unwrap();
        assert!(versions[0].snapshot.is_some());
    }

    #[test]
    fn named_version_is_listed_by_its_name() {
        let dir = test_repo();
        let script = "part.py";
        fs::write(dir.path().join(script), "box = Box(10, 10, 10)").unwrap();

        super::create_snapshot(dir.path(), "Before fillet", script).unwrap();

        let versions = list_microversions(dir.path(), 10).unwrap();
        assert!(versions.iter().any(|version| {
            version
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.name == "Before fillet")
        }));
    }

    #[test]
    fn commit_from_detached_head_creates_branch() {
        let dir = test_repo();
        let script = "part.py";
        fs::write(dir.path().join(script), "box = Box(10, 10, 10)").unwrap();
        let v1 = create_microversion(dir.path(), "First version", "make a box", script).unwrap();

        // Detach HEAD by checking out the commit directly.
        checkout_commit(dir.path(), &v1.commit_hash).unwrap();
        assert_eq!(current_branch(dir.path()).unwrap(), "HEAD");

        // A new microversion from detached HEAD should create a branch.
        fs::write(dir.path().join(script), "box = Box(30, 30, 30)").unwrap();
        create_microversion(dir.path(), "Divergent edit", "change it", script).unwrap();

        // HEAD should now be on a named branch, not detached.
        let branch = current_branch(dir.path()).unwrap();
        assert_ne!(branch, "HEAD", "should have re-attached to a branch");
        assert!(
            branch.starts_with("cadmark-edit-"),
            "branch name should encode the base commit"
        );
    }

    #[test]
    fn repeated_edits_from_the_same_undone_step_create_distinct_branches() {
        let dir = test_repo();
        let script = "part.py";
        fs::write(dir.path().join(script), "box = Box(10, 10, 10)").unwrap();
        let base = create_microversion(dir.path(), "First version", "make a box", script).unwrap();

        checkout_commit(dir.path(), &base.commit_hash).unwrap();
        fs::write(dir.path().join(script), "box = Box(20, 20, 20)").unwrap();
        create_microversion(dir.path(), "First alternative", "make it wider", script).unwrap();
        let first_branch = current_branch(dir.path()).unwrap();

        checkout_commit(dir.path(), &base.commit_hash).unwrap();
        fs::write(dir.path().join(script), "box = Box(30, 30, 30)").unwrap();
        let second =
            create_microversion(dir.path(), "Second alternative", "make it taller", script)
                .unwrap();
        let second_branch = current_branch(dir.path()).unwrap();

        assert_ne!(first_branch, second_branch);
        assert_eq!(second.summary, "Second alternative");
        assert!(
            list_branches(dir.path()).unwrap().contains(&second_branch),
            "the second edit must be recorded on its own branch"
        );
    }

    #[test]
    fn history_lists_steps_on_several_alternatives_from_one_undone_step() {
        let dir = test_repo();
        let script = "part.py";
        fs::write(dir.path().join(script), "box = Box(10, 10, 10)").unwrap();
        let base = create_microversion(dir.path(), "Create box", "make a box", script).unwrap();
        let base_hash = run_git(dir.path(), &["rev-parse", &base.commit_hash]).unwrap();

        let mut alternatives = Vec::new();
        for (summary, script_source) in [
            ("First alternative", "box = Box(20, 20, 20)"),
            ("Second alternative", "box = Box(30, 30, 30)"),
            ("Third alternative", "box = Box(40, 40, 40)"),
        ] {
            checkout_commit(dir.path(), &base.commit_hash).unwrap();
            fs::write(dir.path().join(script), script_source).unwrap();
            let alternative =
                create_microversion(dir.path(), summary, "try another edit", script).unwrap();
            let parent = run_git(
                dir.path(),
                &["rev-parse", &format!("{}^", alternative.commit_hash)],
            )
            .unwrap();
            assert_eq!(
                parent.trim(),
                base_hash.trim(),
                "{summary} must be a direct child of the undone base"
            );
            alternatives.push(alternative);
        }

        let versions = list_microversions(dir.path(), 10).unwrap();
        assert!(
            versions
                .iter()
                .any(|version| version.commit_hash == base.commit_hash)
        );
        for alternative in alternatives {
            assert!(
                versions
                    .iter()
                    .any(|version| version.commit_hash == alternative.commit_hash),
                "history omitted {}",
                alternative.summary
            );
        }

        checkout_commit(dir.path(), &base.commit_hash).unwrap();
        let current_lane = list_current_lane_microversions(dir.path(), 10).unwrap();
        assert_eq!(current_lane.len(), 1);
        assert_eq!(current_lane[0].commit_hash, base.commit_hash);
    }

    #[test]
    fn history_ignores_refs_cadmark_does_not_own() {
        let dir = test_repo();
        let script = "part.py";
        fs::write(dir.path().join(script), "box = Box(10, 10, 10)").unwrap();
        create_microversion(dir.path(), "Design step", "make a box", script).unwrap();

        create_branch(dir.path(), "unrelated-work").unwrap();
        fs::write(dir.path().join("notes.txt"), "not a design step").unwrap();
        run_git(dir.path(), &["add", "notes.txt"]).unwrap();
        run_git(dir.path(), &["commit", "-m", "Unrelated work"]).unwrap();
        let unrelated = run_git(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        run_git(
            dir.path(),
            &["update-ref", "refs/remotes/origin/main", unrelated.trim()],
        )
        .unwrap();
        switch_branch(dir.path(), "main").unwrap();
        run_git(dir.path(), &["tag", "unrelated-tag", unrelated.trim()]).unwrap();
        fs::write(dir.path().join("scratch.txt"), "stash content").unwrap();
        run_git(
            dir.path(),
            &["stash", "push", "-u", "-m", "not a design step"],
        )
        .unwrap();

        let versions = list_microversions(dir.path(), 10).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].summary, "Design step");
    }

    #[test]
    fn failed_step_recording_leaves_no_history_entry() {
        let dir = test_repo();
        let script = "part.py";
        fs::write(dir.path().join(script), "box = Box(10, 10, 10)").unwrap();
        create_microversion(dir.path(), "Existing step", "make a box", script).unwrap();
        let history_before = list_microversions(dir.path(), 10).unwrap();

        let result = create_microversion(
            dir.path(),
            "Missing script",
            "make a box",
            "does-not-exist.py",
        );

        assert!(result.is_err());
        let history_after = list_microversions(dir.path(), 10).unwrap();
        assert_eq!(history_after.len(), history_before.len());
        assert_eq!(history_after[0].commit_hash, history_before[0].commit_hash);
    }

    #[test]
    fn branch_operations() {
        let dir = test_repo();
        let script = "part.py";
        fs::write(dir.path().join(script), "box = Box(10, 10, 10)").unwrap();
        create_microversion(dir.path(), "Initial", "initial", script).unwrap();

        // Create a branch for alternatives.
        create_branch(dir.path(), "alt-design").unwrap();
        assert_eq!(current_branch(dir.path()).unwrap(), "alt-design");

        // Switch back.
        switch_branch(dir.path(), "main").unwrap();
        assert_eq!(current_branch(dir.path()).unwrap(), "main");

        let branches = list_branches(dir.path()).unwrap();
        assert!(branches.contains(&"main".to_string()));
        assert!(branches.contains(&"alt-design".to_string()));
    }
}
