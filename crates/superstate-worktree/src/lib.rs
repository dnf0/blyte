//! Native Worktree management engine for Superstate powered by Worktrunk.

pub mod cache;
pub mod error;
pub mod manager;
pub mod types;

pub use cache::reflink_cache_dirs;
pub use error::WorktreeError;
pub use manager::{IsolatedWorktree, WorktreeManager};
pub use types::{WorktreeConfig, WorktreeSummary};
