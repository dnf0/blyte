//! Worktree domain types and configurations.

use std::path::PathBuf;
use serde::{Deserialize, Serialize};

/// Configuration options for creating a new isolated worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeConfig {
    /// Root path of the primary repository.
    pub base_path: PathBuf,
    /// Name of the branch to create or checkout in the worktree.
    pub branch: String,
    /// Base branch or commit to branch off from (defaults to HEAD / default branch).
    pub base_branch: Option<String>,
    /// Target path for the worktree. If `None`, defaults to `<base_path>/.wt/<branch-sanitized>`.
    pub worktree_path: Option<PathBuf>,
    /// Cache directory names to reflink-copy (e.g. `[".venv", "target", "node_modules"]`).
    pub cache_dirs: Vec<String>,
}

/// Summary of an active worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeSummary {
    /// Worktree branch name, or `None` if detached HEAD.
    pub branch: Option<String>,
    /// Absolute filesystem path to the worktree directory.
    pub path: PathBuf,
    /// Latest commit SHA of the worktree HEAD.
    pub commit_sha: String,
    /// Whether this is the main / primary worktree.
    pub is_main: bool,
}
