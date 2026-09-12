# Task 5 Brief: Worktree Lifecycle Handlers (`crates/superstate-mcp/src/server.rs`)

- **Working Directory:** `/Users/danielfisher/repos/superstate/.wt/feat-mcp-cli-complete-parity-task-5`
- **Branch:** `wt/feat-mcp-cli-complete-parity-task-5`
- **Depends on:** Task 4

## Files
- Modify: `crates/superstate-mcp/src/server.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_worktree_lifecycle.rs`

## Requirements
Implement handlers in `handle_tool_call` for the 3 remaining worktree lifecycle tools:

1. `worktree_list`:
   - Parameters: `repo?: string`, `json?: boolean`
   - Resolve repo root.
   - Initialize `WorktreeManager::new(&repo_root)`.
   - Call `manager.list_worktrees()`.
   - Return list of worktrees (branch, path, HEAD commit, active status).
   - If `json == true`: return structured JSON list.
   - If `json == false`: return formatted text/markdown table matching CLI `superstate worktree list`.

2. `worktree_remove`:
   - Parameters: `branch: string` (required), `force?: boolean` (default false), `repo?: string`
   - Resolve repo root.
   - Initialize `WorktreeManager::new(&repo_root)`.
   - Call `manager.remove_worktree(&branch, force)`.
   - Return confirmation:
     ```json
     {
       "branch": branch,
       "removed": true,
       "content": [{ "type": "text", "text": format!("Removed worktree for branch '{}'", branch) }]
     }
     ```

3. `worktree_clean`:
   - Parameters: `repo?: string`, `force?: boolean` (default false)
   - Resolve repo root.
   - Initialize `WorktreeManager::new(&repo_root)`.
   - Call `manager.clean_orphaned_worktrees(force)`.
   - Return clean report:
     ```json
     {
       "cleaned": cleaned_count,
       "branches": cleaned_branches,
       "content": [{ "type": "text", "text": format!("Cleaned {} orphaned worktree(s)", cleaned_count) }]
     }
     ```

## Steps (TDD)
1. Write failing integration tests in `crates/superstate-mcp/tests/test_mcp_worktree_lifecycle.rs`:
   - Test `worktree_list`: creates 2 worktrees and asserts `worktree_list` returns both with correct branch names and paths in both text and JSON modes.
   - Test `worktree_remove`: removes a worktree and asserts it is no longer listed and its directory is removed.
   - Test `worktree_clean`: creates an orphaned worktree directory or branch and asserts `worktree_clean` cleans it up.
   - Test JSON-RPC dispatch for all 3 tools.
2. Run test to verify failure: `cargo test -p superstate-mcp --test test_mcp_worktree_lifecycle`.
3. Implement the 3 handlers in `crates/superstate-mcp/src/server.rs`.
4. Run test to verify passing: `cargo test -p superstate-mcp --test test_mcp_worktree_lifecycle`.
5. Check `cargo clippy -p superstate-mcp` and `cargo fmt -p superstate-mcp --check`.
6. Commit in the worktree:
   `git add crates/superstate-mcp/src/server.rs crates/superstate-mcp/tests/test_mcp_worktree_lifecycle.rs`
   `git commit -m "feat(mcp): implement worktree_list, worktree_remove, and worktree_clean handlers"`
7. Write report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-5-report.md`.
