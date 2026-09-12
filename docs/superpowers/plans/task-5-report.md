# Task 5: Worktree Lifecycle Handlers Report

**Status:** Completed  
**Branch:** `wt/feat-mcp-cli-complete-parity-task-5`  
**Commit:** `3f92e351d401817419293acc21f6e6a51368c7cf`  
**Author:** Subagent (Task 5)  
**Date:** 2026-09-12  

---

## 1. Overview & Objectives

Task 5 implements full CLI parity for git worktree lifecycle operations in the Superstate MCP server (`crates/superstate-mcp/src/server.rs`). Specifically:
1. `worktree_list`: Lists active worktrees in the repository with branch, path, HEAD commit SHA, and primary worktree status in both JSON and human-readable tabular text formats.
2. `worktree_remove`: Safely removes an isolated worktree and its branch, guarding against deletion of the primary worktree or deleting a worktree while the current working directory is inside it.
3. `worktree_clean`: Cleans up orphaned worktree directories in `.wt/`, deletes dangling `wt/*` branches, and prunes stale git worktree metadata via `git worktree prune`.

---

## 2. Implementation Summary

### Worktree Handlers (`crates/superstate-mcp/src/server.rs`)

1. **`worktree_list`**:
   - **Parameters:** `repo?: string` (with `repo_root` / `cwd` fallbacks), `json?: boolean`.
   - **Behavior:**
     - Resolves the repository root path.
     - Initializes `WorktreeManager::new(&repo_root)` and calls `manager.list()`.
     - Maps summaries to a list of worktrees including `branch`, `path`, `commit_sha`, `head`, `is_main`, and `active`.
     - When `json == true`: returns structured JSON with formatted JSON string in `content`.
     - When `json == false` (or omitted): returns formatted CLI-identical table with `BRANCH`, `HEAD`, `PATH` headers, short SHAs, and `[main]` indicator for primary worktree.

2. **`worktree_remove`**:
   - **Parameters:** `branch: string` (required), `force?: boolean` (default `false`), `repo?: string`.
   - **Behavior:**
     - Resolves the worktree via `resolve_worktree_for_remove`.
     - Validates that the branch exists in the worktree list (supporting both `wt/<branch>` and exact branch names).
     - Validates that the primary worktree cannot be removed (`Cannot remove primary worktree branch '{branch}'`).
     - Validates that the current working directory is not inside the target worktree path (`Cannot remove worktree '{branch}' while current working directory is inside it`).
     - Calls `isolated.destroy(force)`, which force-deletes the git branch via `worktrunk::git::remove_worktree_with_cleanup` and purges the directory from disk.
     - Returns confirmation response matching the MCP contract:
       ```json
       {
         "status": "success",
         "branch": branch,
         "removed": true,
         "content": [{ "type": "text", "text": "Removed worktree for branch '...'" }]
       }
       ```

3. **`worktree_clean`**:
   - **Parameters:** `repo?: string`, `force?: boolean` (default `false`).
   - **Behavior:**
     - Implements `clean_orphaned_worktrees(repo_root, force)`.
     - Prunes stale git worktree tracking via `git worktree prune`.
     - Scans `.wt/` for directories not matching any active worktree returned by `WorktreeManager::list()`. Removes orphaned directories and associated branches.
     - Scans `refs/heads/wt/*` for branches that have no active worktrees checked out, and deletes dangling branches via `git branch -D`.
     - Runs a second `git worktree prune` pass.
     - Returns clean report:
       ```json
       {
         "status": "success",
         "cleaned": cleaned_count,
         "branches": cleaned_branches,
         "content": [{ "type": "text", "text": "Cleaned N orphaned worktree(s)" }]
       }
       ```

4. **Repository Root Argument Parity**:
   - Updated `worktree_create` and `worktree_merge` to accept `"repo"` in addition to `"repo_root"` and `"cwd"`, ensuring consistent parameter names across all worktree tools.

---

## 3. Test-Driven Development (TDD) Lifecycle

1. **RED Phase**:
   - Authored integration test suite in `crates/superstate-mcp/tests/test_mcp_worktree_lifecycle.rs`:
     - `test_worktree_list_text_and_json_modes`: Creates two isolated worktrees, verifies text table formatting and structured JSON output.
     - `test_worktree_remove_success_and_cleanup`: Creates a worktree, removes it via `worktree_remove`, verifies disk directory removal and absence from `worktree_list`.
     - `test_worktree_remove_errors_and_guards`: Asserts errors on removing primary worktree and removing non-existent branches.
     - `test_worktree_clean_orphaned_directories_and_branches`: Creates an orphaned directory in `.wt/` and an orphaned branch `wt/orphaned-branch`, asserts both are cleaned and reported, and tests clean report idempotency (reporting 0 on subsequent calls).
     - `test_json_rpc_dispatch_all_three_tools`: Tests JSON-RPC `tools/call` standard request dispatch for `worktree_list`, `worktree_remove`, and `worktree_clean`.
   - Executed `cargo test -p superstate-mcp --test test_mcp_worktree_lifecycle`.
   - Verified that all 5 tests failed with expected errors: `Unknown tool: worktree_list`, `Unknown tool: worktree_remove`, and `Unknown tool: worktree_clean`.

2. **GREEN Phase**:
   - Implemented tool dispatch arms in `crates/superstate-mcp/src/server.rs`.
   - Implemented helper functions `resolve_worktree_for_remove` and `clean_orphaned_worktrees`.
   - Executed `cargo test -p superstate-mcp --test test_mcp_worktree_lifecycle`: all 5 tests passed in 0.53s.

3. **Refactor & Code Quality**:
   - Formatted all code cleanly using `cargo fmt -p superstate-mcp`.
   - Verified static analysis with `cargo clippy -p superstate-mcp --all-targets -- -D warnings`: 0 warnings, clean compilation.
   - Executed complete crate test suite `cargo test -p superstate-mcp`: all 46 tests passed cleanly across all 10 test binaries.

---

## 4. Git Commit Details

- **Commit:** `3f92e351d401817419293acc21f6e6a51368c7cf`
- **Message:** `feat(mcp): implement worktree_list, worktree_remove, and worktree_clean handlers`
- **Files:**
  - `crates/superstate-mcp/src/server.rs` (+327, -2)
  - `crates/superstate-mcp/tests/test_mcp_worktree_lifecycle.rs` (+413)
- **Git Status:** Working tree clean (`git status --porcelain -uall` empty).
