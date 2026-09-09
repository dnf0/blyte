//! Git checkpoints around risky milestone edits.
//!
//! A checkpoint is a lightweight tag on the current commit, named for the step it
//! guards. Rollback restores the working tree to that tag *and* removes untracked
//! files: a milestone that half-created files and then failed leaves debris which
//! a plain `reset --hard` does not touch, and that debris would otherwise be
//! carried into the next milestone and into the graded diff.

use std::path::Path;
use std::process::Command;
use thiserror::Error;

/// Tag namespace for checkpoints, matching the reference implementation.
pub const CHECKPOINT_PREFIX: &str = "agent-checkpoint/step-";

#[derive(Error, Debug)]
pub enum CheckpointError {
    #[error("git command failed: {0}")]
    Git(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// The tag name guarding a given step.
pub fn checkpoint_ref(step: usize) -> String {
    format!("{}{}", CHECKPOINT_PREFIX, step)
}

fn git(repo_root: &Path, args: &[&str]) -> Result<String, CheckpointError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()?;
    if !output.status.success() {
        return Err(CheckpointError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Create (or move) the checkpoint tag for `step` at the current commit.
///
/// Re-checkpointing the same step replaces the tag rather than failing, so a
/// retried milestone guards its own starting point instead of an older one.
pub fn create_checkpoint(repo_root: &Path, step: usize) -> Result<String, CheckpointError> {
    let tag = checkpoint_ref(step);
    git(repo_root, &["tag", "-f", &tag])?;
    Ok(tag)
}

/// Reset the working tree to a checkpoint and remove untracked files.
///
/// `reference` defaults to the checkpoint for `step`. `preserve` names paths that
/// must survive the clean -- above all the state file itself, which is untracked
/// in most task repositories and would otherwise be deleted by the very command
/// meant to recover from a failure, taking the record of that failure with it.
pub fn rollback_to(
    repo_root: &Path,
    step: usize,
    reference: Option<&str>,
    preserve: &[&str],
) -> Result<String, CheckpointError> {
    let target = reference
        .map(str::to_string)
        .unwrap_or_else(|| checkpoint_ref(step));

    // Fail before touching the tree if the target does not exist, so a typo in
    // --ref cannot destroy work by resetting to something unintended.
    git(repo_root, &["rev-parse", "--verify", &target]).map_err(|_| {
        CheckpointError::Git(format!(
            "checkpoint '{}' does not exist; refusing to reset the working tree",
            target
        ))
    })?;

    git(repo_root, &["reset", "--hard", &target])?;

    let mut clean_args: Vec<String> = vec!["clean".into(), "-fd".into()];
    for path in preserve {
        clean_args.push("-e".into());
        clean_args.push((*path).to_string());
    }
    let clean_refs: Vec<&str> = clean_args.iter().map(String::as_str).collect();
    git(repo_root, &clean_refs)?;
    Ok(target)
}
