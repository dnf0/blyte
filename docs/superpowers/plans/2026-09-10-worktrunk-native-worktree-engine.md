# Native Worktrunk Git Worktree Engine Implementation Plan

> **Goal**: Embed `worktrunk` (`0.77.0`, pure library mode without CLI/TUI features) into Superstate to provide native, isolated Git worktree management, APFS/btrfs reflink COW cache sharing, atomic milestone squashes, and zero-blast-radius rollbacks.

- **Proposed Architecture**: Dedicated `crates/superstate-worktree` crate in the `superstate` Cargo workspace, consumed by `superstate-cli` via `superstate worktree <create|merge|remove|list>`.

---

## Tasks

### Task 1: Scaffold `crates/superstate-worktree` Crate

- **Files to Modify/Create**:
  - `Cargo.toml`: Add `"crates/superstate-worktree"` to `[workspace.members]`.
  - `crates/superstate-worktree/Cargo.toml`: Create package definition with `worktrunk = { version = "0.77", default-features = false }`, `tempfile`, `thiserror`, `serde`, `tracing`.
  - `crates/superstate-worktree/src/lib.rs`: Library entrypoint and module exports (`error`, `types`, `cache`, `manager`).
  - `crates/superstate-worktree/src/error.rs`: Domain error types (`WorktreeError`).
- **Verification**: `cargo check -p superstate-worktree`.

---

### Task 2: Implement Reflink Cache Sharing (`crates/superstate-worktree/src/cache.rs`)

- **Files to Modify/Create**:
  - `crates/superstate-worktree/src/cache.rs`:
    - Implement `reflink_cache_dirs(source_root: &Path, target_root: &Path, cache_names: &[&str]) -> Result<u64, WorktreeError>`.
    - Recursively walks and reflink-copies directories (e.g. `.venv`, `node_modules`, `target`) into the worktree using `worktrunk::copy::reflink_or_copy`.
  - `crates/superstate-worktree/tests/test_cache_reflink.rs`:
    - Unit test creating a source directory tree, executing reflink copy to a target directory, and asserting file presence and contents.
- **Verification**: `cargo test -p superstate-worktree --test test_cache_reflink`.

---

### Task 3: Implement Worktree Lifecycle Engine (`crates/superstate-worktree/src/manager.rs`)

- **Files to Modify/Create**:
  - `crates/superstate-worktree/src/types.rs`: `WorktreeConfig`, `WorktreeSummary`, `WorktreeState`.
  - `crates/superstate-worktree/src/manager.rs`:
    - `WorktreeManager::new(repo_root: &Path) -> Result<Self, WorktreeError>`.
    - `WorktreeManager::create(&self, config: &WorktreeConfig) -> Result<IsolatedWorktree, WorktreeError>`: Runs `git worktree add`, checks out branch, and triggers reflink cache copying.
    - `WorktreeManager::list(&self) -> Result<Vec<WorktreeSummary>, WorktreeError>`: Inspects active worktrees and integration status.
    - `IsolatedWorktree::merge_into_base(&self, commit_msg: &str) -> Result<(), WorktreeError>`: Merges branch into base branch cleanly.
    - `IsolatedWorktree::destroy(self, force: bool) -> Result<(), WorktreeError>`: Removes worktree with `worktrunk::git::remove::remove_worktree_with_cleanup`.
  - `crates/superstate-worktree/tests/test_worktree_lifecycle.rs`:
    - Integration test creating a git repository with baseline commit, spawning an isolated worktree, writing a test commit in the worktree, merging into main, and verifying main contains the commit.
- **Verification**: `cargo test -p superstate-worktree --test test_worktree_lifecycle`.

---

### Task 4: CLI Integration in `crates/superstate-cli` (`superstate worktree`)

- **Files to Modify/Create**:
  - `crates/superstate-cli/Cargo.toml`: Add `superstate-worktree = { path = "../superstate-worktree", version = "0.1.0" }`.
  - `crates/superstate-cli/src/main.rs`: Register `Worktree` subcommand in Clap CLI enum.
  - `crates/superstate-cli/src/commands/worktree.rs`: Implement CLI handlers:
    - `create <branch> [--base <base>] [--path <path>] [--copy-caches]`
    - `list`
    - `merge <branch> [--target <target>] [--message <msg>]`
    - `remove <branch> [--force]`
  - `crates/superstate-cli/tests/test_worktree_cli.rs`:
    - End-to-end test invoking `superstate worktree create`, `list`, `merge`, and `remove`.
- **Verification**: `cargo test -p superstate --test test_worktree_cli`.

---

### Task 5: End-to-End Workspace & Regression Testing

- **Actions**:
  - Run `cargo test` across all crates in `superstate`.
  - Run `cargo clippy --workspace` to ensure clean code style and zero warnings.
  - Commit all changes with Conventional Commits.
- **Verification**: `cargo test && cargo clippy --all-targets -- -D warnings`.
