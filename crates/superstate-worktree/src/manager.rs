//! Worktree lifecycle manager and isolated execution environment.

use std::fs;
use std::path::PathBuf;
use worktrunk::git::{
    remove_worktree_with_cleanup, BranchDeletionMode, RemoveOptions, Repository,
};

use crate::cache::reflink_cache_dirs;
use crate::error::WorktreeError;
use crate::types::{WorktreeConfig, WorktreeSummary};

/// Orchestrator for Git worktree lifecycles and COW cache sharing.
#[derive(Debug, Clone)]
pub struct WorktreeManager {
    repo_root: PathBuf,
}

impl WorktreeManager {
    /// Initialize a new worktree manager rooted at `repo_root`.
    pub fn new(repo_root: impl Into<PathBuf>) -> Result<Self, WorktreeError> {
        let repo_root = repo_root.into();
        Ok(Self { repo_root })
    }

    /// Open repository via worktrunk.
    fn repo(&self) -> Result<Repository, WorktreeError> {
        Repository::at(&self.repo_root).map_err(|e| WorktreeError::Repository(e.to_string()))
    }

    /// List all registered worktrees in the repository.
    pub fn list(&self) -> Result<Vec<WorktreeSummary>, WorktreeError> {
        let repo = self.repo()?;
        let worktrees = repo
            .list_worktrees()
            .map_err(|e| WorktreeError::Git(e.to_string()))?;

        let primary_path = repo.primary_worktree().ok().flatten();

        let summaries = worktrees
            .iter()
            .map(|wt| {
                let is_main = if let Some(ref p) = primary_path {
                    p == &wt.path
                } else {
                    false
                };
                WorktreeSummary {
                    branch: wt.branch.clone(),
                    path: wt.path.clone(),
                    commit_sha: wt.head.clone(),
                    is_main,
                }
            })
            .collect();

        Ok(summaries)
    }

    /// Create and initialize a new isolated worktree based on `config`.
    pub fn create(&self, config: &WorktreeConfig) -> Result<IsolatedWorktree, WorktreeError> {
        let repo = self.repo()?;

        let sanitized_branch = config.branch.replace('/', "-");
        let worktree_path = match &config.worktree_path {
            Some(p) => p.clone(),
            None => self.repo_root.join(".wt").join(&sanitized_branch),
        };

        if worktree_path.exists() {
            return Err(WorktreeError::AlreadyExists {
                branch: config.branch.clone(),
                path: worktree_path,
            });
        }

        if let Some(parent) = worktree_path.parent() {
            fs::create_dir_all(parent).map_err(|e| WorktreeError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        let base_branch = match &config.base_branch {
            Some(b) => b.clone(),
            None => repo.default_branch().unwrap_or_else(|| "main".to_string()),
        };

        let wt_str = worktree_path.to_string_lossy().to_string();

        let branch_exists = repo.branch(&config.branch).exists().unwrap_or(false);

        let mut args = vec!["worktree", "add"];
        if !branch_exists {
            args.push("-b");
            args.push(&config.branch);
            args.push(&wt_str);
            args.push(&base_branch);
        } else {
            args.push(&wt_str);
            args.push(&config.branch);
        }

        repo.run_command(&args)
            .map_err(|e| WorktreeError::Git(e.to_string()))?;

        // Reflink cache directories if requested
        if !config.cache_dirs.is_empty() {
            let cache_slices: Vec<&str> = config.cache_dirs.iter().map(|s| s.as_str()).collect();
            let _ = reflink_cache_dirs(&self.repo_root, &worktree_path, &cache_slices)?;
        }

        Ok(IsolatedWorktree {
            branch: config.branch.clone(),
            path: worktree_path,
            base_branch,
            repo_root: self.repo_root.clone(),
        })
    }
}

/// An active, isolated working tree.
#[derive(Debug)]
pub struct IsolatedWorktree {
    /// Active branch name in this worktree.
    pub branch: String,
    /// Absolute path to the worktree checkout.
    pub path: PathBuf,
    /// Base branch this worktree was spawned from.
    pub base_branch: String,
    /// Root path of the primary repository.
    pub repo_root: PathBuf,
}

impl IsolatedWorktree {
    /// Merge commits from this worktree branch into the base branch in the primary repo.
    pub fn merge_into_base(&self, commit_msg: &str) -> Result<(), WorktreeError> {
        let repo = Repository::at(&self.repo_root)
            .map_err(|e| WorktreeError::Repository(e.to_string()))?;

        repo.run_command(&["merge", "--no-ff", "-m", commit_msg, &self.branch])
            .map_err(|e| WorktreeError::MergeConflict(e.to_string()))?;

        Ok(())
    }

    /// Teardown the worktree and safely remove or delete the branch.
    pub fn destroy(self, force: bool) -> Result<(), WorktreeError> {
        let repo = Repository::at(&self.repo_root)
            .map_err(|e| WorktreeError::Repository(e.to_string()))?;
        let snapshot = repo
            .capture_refs()
            .map_err(|e| WorktreeError::Git(e.to_string()))?;

        let options = RemoveOptions {
            branch: Some(self.branch.clone()),
            deletion_mode: BranchDeletionMode::ForceDelete,
            target_branch: Some(self.base_branch.clone()),
            force_worktree: force,
        };

        let output = remove_worktree_with_cleanup(&repo, &snapshot, &self.path, options)
            .map_err(|e| WorktreeError::Git(e.to_string()))?;

        if let Some(staged) = output.staged_path {
            let _ = fs::remove_dir_all(staged);
        }

        if self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }

        Ok(())
    }
}
