# Task 6 Brief: Full Verification, Binary Build & End-to-End Suite

- **Working Directory:** `/Users/danielfisher/repos/superstate/.wt/feat-mcp-cli-complete-parity-task-6`
- **Branch:** `wt/feat-mcp-cli-complete-parity-task-6`
- **Depends on:** Tasks 1-5

## Tasks
1. Address the review observations from Task 5:
   - In `crates/superstate-mcp/src/tools.rs`:
     - Add `json: { "type": "boolean", "description": "Emit machine-readable JSON array" }` to `worktree_list` schema properties.
     - Add `force: { "type": "boolean", "description": "Force removal of dirty or unmerged orphaned branches" }` to `worktree_clean` schema properties.
   - In `crates/superstate-mcp/src/server.rs`:
     - In `clean_orphaned_worktrees(repo_root: &Path, force: bool)`: use `-D` if `force` is true, else use `-d` (with fallback warning or handling) so `force` is honored.
2. Run full workspace test suite:
   - `cargo test --workspace`
   - Ensure all crates pass (`superstate_core`, `superstate_worktree`, `superstate_cli`, `superstate_mcp`).
3. Run workspace clippy and formatting:
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo fmt --all -- --check`
4. Build the release binary:
   - `cargo build --release`
5. Verify git status is clean and commit changes:
   - `git add crates/superstate-mcp`
   - `git commit -m "chore(mcp): complete schema polish and verify workspace suite"`
6. Write final report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-6-report.md`.
