//! CLI command definitions and handlers for Git worktree operations.

use clap::Subcommand;
use std::path::{Path, PathBuf};
use superstate_worktree::manager::{IsolatedWorktree, WorktreeManager};
use superstate_worktree::types::WorktreeConfig;

#[derive(Subcommand, Debug, Clone)]
pub enum WorktreeCommands {
    /// Create a new isolated worktree
    Create {
        /// Branch name to create or checkout
        branch: String,
        /// Base branch to branch off from (e.g. main)
        #[arg(short, long)]
        base: Option<String>,
        /// Custom destination path for worktree
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Cache directory names to reflink-copy (e.g. .venv target node_modules)
        #[arg(short, long, value_delimiter = ',', num_args = 1..)]
        caches: Vec<String>,
        /// Disable auto-detection and copying of cache directories
        #[arg(long, conflicts_with = "caches")]
        no_cache: bool,
        /// Base repository root (defaults to current dir)
        #[arg(short, long, default_value = ".")]
        repo: PathBuf,
    },
    /// List active worktrees in the repository
    List {
        /// Base repository root (defaults to current dir)
        #[arg(short, long, default_value = ".")]
        repo: PathBuf,
    },
    /// Merge a worktree branch into the base branch
    Merge {
        /// Branch name to merge
        branch: String,
        /// Base branch to merge into (defaults to the default/primary branch)
        #[arg(short, long)]
        base: Option<String>,
        /// Commit message for the merge
        #[arg(short, long, default_value = "chore: merge milestone worktree")]
        message: String,
        /// Base repository root (defaults to current dir)
        #[arg(short, long, default_value = ".")]
        repo: PathBuf,
    },
    /// Remove a worktree and clean up its tracking
    Remove {
        /// Branch name of the worktree to remove
        branch: String,
        /// Force removal even if worktree is dirty
        #[arg(short, long)]
        force: bool,
        /// Base repository root (defaults to current dir)
        #[arg(short, long, default_value = ".")]
        repo: PathBuf,
    },
}

fn resolve_worktree_for_action(
    repo: &Path,
    branch: &str,
    action: &str,
    expected_base: Option<&str>,
) -> Result<IsolatedWorktree, String> {
    let manager = WorktreeManager::new(repo).map_err(|e| e.to_string())?;
    let list = manager.list().map_err(|e| e.to_string())?;

    let primary_wt = list
        .iter()
        .find(|w| w.is_main)
        .cloned()
        .ok_or_else(|| "Could not identify primary repository worktree".to_string())?;

    let wt = list
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch))
        .ok_or_else(|| format!("Worktree branch '{}' not found", branch))?;

    if wt.is_main {
        return Err(format!("Cannot {} primary worktree branch '{}'", action, branch));
    }

    let base_branch = expected_base
        .map(|s| s.to_string())
        .or_else(|| primary_wt.branch.clone())
        .unwrap_or_else(|| "main".to_string());

    if let Some(target) = expected_base {
        if primary_wt.branch.as_deref() != Some(target) {
            return Err(format!(
                "Primary worktree is checked out to {:?}, expected target base branch '{}'",
                primary_wt.branch, target
            ));
        }
    }

    Ok(IsolatedWorktree {
        branch: branch.to_string(),
        path: wt.path,
        base_branch,
        repo_root: primary_wt.path,
    })
}

pub fn dispatch_worktree(cmd: WorktreeCommands) -> Result<(), String> {
    match cmd {
        WorktreeCommands::Create {
            branch,
            base,
            path,
            caches,
            no_cache,
            repo,
        } => {
            let manager = WorktreeManager::new(&repo).map_err(|e| e.to_string())?;
            let mut cache_dirs = caches;
            if !no_cache && cache_dirs.is_empty() {
                // Auto-detect common cache dirs
                for candidate in &[".venv", "node_modules", "target"] {
                    if repo.join(candidate).exists() {
                        cache_dirs.push(candidate.to_string());
                    }
                }
            }

            let config = WorktreeConfig {
                base_path: repo.clone(),
                branch: branch.clone(),
                base_branch: base,
                worktree_path: path,
                cache_dirs,
            };

            let isolated = manager.create(&config).map_err(|e| e.to_string())?;
            println!(
                "Created isolated worktree for '{}' at {}",
                branch,
                isolated.path.display()
            );
            Ok(())
        }
        WorktreeCommands::List { repo } => {
            let manager = WorktreeManager::new(&repo).map_err(|e| e.to_string())?;
            let list = manager.list().map_err(|e| e.to_string())?;

            if list.is_empty() {
                println!("No active worktrees.");
                return Ok(());
            }

            println!("{:<25} {:<10} {:<40}", "BRANCH", "HEAD", "PATH");
            println!("{:-<25} {:-<10} {:-<40}", "", "", "");
            for wt in list {
                let branch_display = wt
                    .branch
                    .unwrap_or_else(|| "(detached HEAD)".to_string())
                    + if wt.is_main { " [main]" } else { "" };
                let short_sha = if wt.commit_sha.len() >= 7 {
                    &wt.commit_sha[..7]
                } else {
                    &wt.commit_sha
                };
                println!(
                    "{:<25} {:<10} {:<40}",
                    branch_display,
                    short_sha,
                    wt.path.display()
                );
            }
            Ok(())
        }
        WorktreeCommands::Merge {
            branch,
            base,
            message,
            repo,
        } => {
            let isolated = resolve_worktree_for_action(&repo, &branch, "merge", base.as_deref())?;
            isolated.merge_into_base(&message).map_err(|e| e.to_string())?;
            println!("Merged worktree branch '{}' into base", branch);
            Ok(())
        }
        WorktreeCommands::Remove {
            branch,
            force,
            repo,
        } => {
            let isolated = resolve_worktree_for_action(&repo, &branch, "remove", None)?;
            isolated.destroy(force).map_err(|e| e.to_string())?;
            println!("Removed worktree branch '{}'", branch);
            Ok(())
        }
    }
}
