//! Worktree domain error types.

use std::path::PathBuf;
use thiserror::Error;

/// Errors arising during worktree lifecycle, reflink copying, or git integration.
#[derive(Debug, Error)]
pub enum WorktreeError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Git error: {0}")]
    Git(String),

    #[error("Repository error: {0}")]
    Repository(String),

    #[error("Worktree '{branch}' already exists at {path}")]
    AlreadyExists {
        branch: String,
        path: PathBuf,
    },

    #[error("Worktree '{branch}' not found")]
    NotFound {
        branch: String,
    },

    #[error("Failed to copy cache directory from {src_path} to {dst_path}: {details}")]
    CacheCopy {
        src_path: PathBuf,
        dst_path: PathBuf,
        details: String,
    },

    #[error("Worktree merge failed: {0}")]
    MergeConflict(String),
}
