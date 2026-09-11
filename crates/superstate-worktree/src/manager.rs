//! Worktree lifecycle manager and isolated execution environment.

use std::fs;
use std::path::{Path, PathBuf};
use worktrunk::git::{remove_worktree_with_cleanup, BranchDeletionMode, RemoveOptions, Repository};

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

    /// Get the default or base branch name for the repository.
    pub fn default_branch(&self) -> String {
        self.repo()
            .ok()
            .and_then(|r| r.default_branch())
            .unwrap_or_else(|| "main".to_string())
    }

    /// Discover the currently active branch in the repository root.
    ///
    /// Runs `git -C <repo_root> rev-parse --abbrev-ref HEAD` to detect the checked-out branch.
    /// If the HEAD is detached or the command fails, falls back to the default branch.
    ///
    /// # Returns
    /// The trimmed active branch name on success, or the repository default branch fallback.
    ///
    /// # Errors
    /// Returns `WorktreeError` if an unrecoverable error occurs (not triggered under standard fallback behavior).
    pub fn current_branch(&self) -> Result<String, WorktreeError> {
        // Query Git for the currently active symbolic ref at the repository root.
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output();

        // Inspect command result, handling detached HEAD or command failures gracefully.
        match output {
            Ok(out) if out.status.success() => {
                let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
                // If HEAD is detached, rev-parse outputs literal "HEAD". Fall back to default branch.
                if branch.is_empty() || branch == "HEAD" {
                    Ok(self.default_branch())
                } else {
                    Ok(branch)
                }
            }
            // If git command fails or repo is invalid, fall back to default branch.
            _ => Ok(self.default_branch()),
        }
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

    /// Provision or reuse an isolated worktree for a specific milestone task.
    ///
    /// When `base_branch` is a feature branch (not "main" or "master"), the worktree
    /// branch and checkout directory are namespaced by the sanitized feature branch slug
    /// (`wt/<sanitized_base_branch>-<task_slug>`), unlocking concurrent isolated worktrees
    /// across independent agents in the same repository.
    ///
    /// # Parameters
    /// - `task_slug`: Identifier for the task (sanitized to remove slashes and whitespace).
    /// - `base_branch`: Optional base branch to spawn from; defaults to current active branch or repo default.
    ///
    /// # Returns
    /// An `IsolatedWorktree` handle pointing to the task worktree.
    ///
    /// # Errors
    /// Returns `WorktreeError` if repo discovery or git worktree creation fails.
    pub fn ensure_task_worktree(
        &self,
        task_slug: &str,
        base_branch: Option<&str>,
    ) -> Result<IsolatedWorktree, WorktreeError> {
        // Resolve base branch: explicit branch if provided, otherwise active feature branch or default.
        let resolved_base_branch = match base_branch {
            Some(b) => b.to_string(),
            None => self
                .current_branch()
                .unwrap_or_else(|_| self.default_branch()),
        };

        let (target_branch, target_path) =
            task_worktree_names(&self.repo_root, task_slug, Some(&resolved_base_branch));

        // If path already exists, return the worktree handle idempotently.
        if target_path.exists() {
            return Ok(IsolatedWorktree {
                branch: target_branch,
                path: target_path,
                base_branch: resolved_base_branch,
                repo_root: self.repo_root.clone(),
            });
        }

        // Configure default cache directories for copy-on-write sharing.
        let default_cache_dirs = vec![
            ".venv".to_string(),
            "target".to_string(),
            "node_modules".to_string(),
        ];

        let config = WorktreeConfig {
            base_path: self.repo_root.clone(),
            branch: target_branch,
            base_branch: Some(resolved_base_branch),
            worktree_path: Some(target_path),
            cache_dirs: default_cache_dirs,
        };

        self.create(&config)
    }
}

/// Helper to determine the target worktree branch name and directory path for a task.
///
/// When `base_branch` is a feature branch (not "main", "master", "wt/*", or "superstate/*"),
/// the worktree branch and path are namespaced by the sanitized base branch slug:
/// - Branch: `wt/<sanitized_base_branch>-<sanitized_task_slug>`
/// - Path: `<repo_root>/.wt/<sanitized_base_branch>-<sanitized_task_slug>`
///
/// For trunk branches ("main", "master") or default:
/// - Branch: `wt/<sanitized_task_slug>`
/// - Path: `<repo_root>/.wt/<sanitized_task_slug>`
pub fn task_worktree_names(
    repo_root: &Path,
    task_slug: &str,
    base_branch: Option<&str>,
) -> (String, PathBuf) {
    let sanitized_slug = sanitize_slug(task_slug);
    let is_feature = match base_branch {
        Some(b) => {
            let b_trimmed = b.trim();
            !matches!(b_trimmed, "main" | "master")
                && !b_trimmed.starts_with("wt/")
                && !b_trimmed.starts_with("superstate/")
        }
        None => false,
    };

    if is_feature {
        let base_slug = sanitize_slug(base_branch.unwrap());
        let full_slug = format!("{}-{}", base_slug, sanitized_slug);
        (
            format!("wt/{}", full_slug),
            repo_root.join(".wt").join(full_slug),
        )
    } else {
        (
            format!("wt/{}", sanitized_slug),
            repo_root.join(".wt").join(sanitized_slug),
        )
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

        // Ensure primary repo is on base_branch before merging
        let current_branch = repo
            .require_current_branch("merge")
            .map_err(|e| WorktreeError::Repository(e.to_string()))?;
        if current_branch != self.base_branch {
            repo.run_command(&["checkout", &self.base_branch])
                .map_err(|e| {
                    WorktreeError::Repository(format!(
                        "Primary repo is on branch '{}', failed checking out base '{}': {}",
                        current_branch, self.base_branch, e
                    ))
                })?;
        }

        if let Err(e) = repo.run_command(&["merge", "--no-ff", "-m", commit_msg, &self.branch]) {
            let err_msg = e.to_string();
            // Git exit code 2 indicates conflicting untracked files or dirty working tree.
            // Check if untracked files in the primary working tree conflict with files
            // committed in the branch being merged. The worktree's committed version is authoritative.
            if err_msg.contains("exit 2") {
                let status_output = repo
                    .run_command(&["status", "--porcelain"])
                    .unwrap_or_default();
                let untracked_files: Vec<String> = status_output
                    .lines()
                    .filter(|l| l.starts_with("?? "))
                    .map(|l| l[3..].trim().to_string())
                    .collect();

                let diff_cmd = format!("{}...{}", self.base_branch, self.branch);
                let branch_files_output = repo
                    .run_command(&["diff", "--name-only", &diff_cmd])
                    .unwrap_or_default();
                let branch_files: Vec<&str> =
                    branch_files_output.lines().map(|l| l.trim()).collect();

                let mut cleaned_any = false;
                for u in &untracked_files {
                    if branch_files.contains(&u.as_str()) {
                        let target = self.repo_root.join(u);
                        if target.is_file() {
                            let _ = fs::remove_file(&target);
                            cleaned_any = true;
                        } else if target.is_dir() {
                            let _ = fs::remove_dir_all(&target);
                            cleaned_any = true;
                        }
                    }
                }

                if cleaned_any {
                    if let Err(retry_err) =
                        repo.run_command(&["merge", "--no-ff", "-m", commit_msg, &self.branch])
                    {
                        let _ = repo.run_command(&["merge", "--abort"]);
                        return Err(WorktreeError::MergeConflict(retry_err.to_string()));
                    } else {
                        return Ok(());
                    }
                }
            }

            // Clean up any conflicted merge state so primary repo is not left dirty
            let _ = repo.run_command(&["merge", "--abort"]);
            return Err(WorktreeError::MergeConflict(err_msg));
        }

        Ok(())
    }

    /// Merge commits from this worktree branch into the base branch, then remove worktree.
    ///
    /// # Parameters
    /// - `commit_msg`: Commit message used when merging into the base branch.
    ///
    /// # Returns
    /// Automatically stage and commit any dirty changes remaining in the worktree.
    ///
    /// # Returns
    /// `Ok(true)` if changes were committed, `Ok(false)` if working tree was already clean.
    pub fn auto_commit_if_dirty(&self, commit_msg: &str) -> Result<bool, WorktreeError> {
        let repo =
            Repository::at(&self.path).map_err(|e| WorktreeError::Repository(e.to_string()))?;
        let status = repo
            .run_command(&["status", "--porcelain"])
            .map_err(|e| WorktreeError::Git(e.to_string()))?;
        if !status.trim().is_empty() {
            repo.run_command(&["add", "-A"])
                .map_err(|e| WorktreeError::Git(e.to_string()))?;
            repo.run_command(&["commit", "-m", commit_msg])
                .map_err(|e| WorktreeError::Git(e.to_string()))?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Merge commits from this worktree branch into the base branch, then remove worktree.
    /// Automatically stages and commits any remaining uncommitted changes before merging.
    ///
    /// # Parameters
    /// - `commit_msg`: Commit message used when merging into the base branch.
    ///
    /// # Returns
    /// `Ok(())` on successful merge and cleanup.
    ///
    /// # Errors
    /// Returns `WorktreeError` if merge conflict arises or worktree deletion fails.
    pub fn merge_and_cleanup(self, commit_msg: &str) -> Result<(), WorktreeError> {
        // Automatically stage and commit any uncommitted changes left by the agent
        let _ = self.auto_commit_if_dirty(commit_msg);

        // Merge worktree branch into base branch in the primary repo.
        self.merge_into_base(commit_msg)?;

        // Tear down worktree checkout and branch. All committed changes are
        // now safely merged into the base branch, so force removal of the
        // worktree directory to ensure untracked test/build artifacts do not
        // wedge the state machine.
        self.destroy(true)?;

        Ok(())
    }

    /// Abort task worktree without merging, force-deleting worktree and branch.
    ///
    /// # Returns
    /// `Ok(())` on successful deletion.
    ///
    /// # Errors
    /// Returns `WorktreeError` if worktree teardown fails.
    pub fn abort(self) -> Result<(), WorktreeError> {
        // Force-delete worktree checkout and branch, discarding unmerged changes.
        self.destroy(true)?;

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

/// Sanitize task slug by replacing slashes and whitespace with hyphens and trimming.
pub fn sanitize_slug(slug: &str) -> String {
    let trimmed = slug.trim();
    let mut result = String::new();
    let mut prev_dash = false;

    // Traverse characters, collapsing whitespace, slashes, and hyphens into single hyphens.
    for c in trimmed.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_whitespace() || c == '/' || c == '\\' || c == '-' {
            if !prev_dash && !result.is_empty() {
                result.push('-');
                prev_dash = true;
            }
        } else {
            result.push(c);
            prev_dash = false;
        }
    }

    // Strip trailing hyphen if present after sanitization.
    if result.ends_with('-') {
        result.pop();
    }

    if result.is_empty() {
        "task".to_string()
    } else {
        result
    }
}

/// Ensure essential Superstate internal paths are added to .git/info/exclude.
pub fn ensure_git_exclude(repo_root: &Path) {
    let exclude_path = repo_root.join(".git").join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let entries = [
        ".agent-state/",
        ".wt/",
        ".wt",
        "plan.md",
        "plan.normalized.md",
        "run-in-container",
    ];
    if let Ok(mut current) = fs::read_to_string(&exclude_path) {
        let mut appended = false;
        for entry in &entries {
            if !current.lines().any(|l| l.trim() == *entry) {
                if !current.ends_with('\n') && !current.is_empty() {
                    current.push('\n');
                }
                current.push_str(entry);
                current.push('\n');
                appended = true;
            }
        }
        if appended {
            let _ = fs::write(&exclude_path, current);
        }
    } else {
        let initial = ".agent-state/\n.wt/\n.wt\nplan.md\nplan.normalized.md\nrun-in-container\n";
        let _ = fs::write(&exclude_path, initial);
    }
}

/// Ensure a baseline git repository exists for worktree and change tracking.
pub fn ensure_git_baseline(repo_root: &Path) {
    let check = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(repo_root)
        .output();

    let inside_git = match check {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(_) => false,
    };

    if !inside_git {
        let init_status = std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(repo_root)
            .status();

        let ok = match init_status {
            Ok(s) if s.success() => true,
            _ => std::process::Command::new("git")
                .args(["init"])
                .current_dir(repo_root)
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
        };

        if ok {
            let _ = std::process::Command::new("git")
                .args(["config", "user.name", "Superstate"])
                .current_dir(repo_root)
                .status();
            let _ = std::process::Command::new("git")
                .args(["config", "user.email", "superstate@local"])
                .current_dir(repo_root)
                .status();
            let _ = std::process::Command::new("git")
                .args([
                    "commit",
                    "--allow-empty",
                    "--no-gpg-sign",
                    "-m",
                    "chore(superstate): initialize tracking baseline",
                ])
                .current_dir(repo_root)
                .status();
        }
    }

    ensure_git_exclude(repo_root);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_slug() {
        assert_eq!(sanitize_slug("task-1"), "task-1");
        assert_eq!(sanitize_slug("Task 1"), "task-1");
        assert_eq!(sanitize_slug("task/1"), "task-1");
        assert_eq!(sanitize_slug("task 1"), "task-1");
        assert_eq!(sanitize_slug("  task / 1  "), "task-1");
        assert_eq!(sanitize_slug(""), "task");
    }
}
