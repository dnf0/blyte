//! Native Worktree management engine for Superstate powered by Worktrunk.

pub mod cache;
pub mod error;

pub use cache::reflink_cache_dirs;
pub use error::WorktreeError;
