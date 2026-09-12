# Task 3 Brief: Safety, Checkpoint, Rollback & Atomic Exec Handlers (`crates/superstate-mcp/src/server.rs`)

- **Working Directory:** `/Users/danielfisher/repos/superstate/.wt/feat-mcp-cli-complete-parity-task-3`
- **Branch:** `wt/feat-mcp-cli-complete-parity-task-3`
- **Depends on:** Task 2

## Files
- Modify: `crates/superstate-mcp/src/server.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_state_safety.rs`

## Requirements
Implement handlers in `handle_tool_call` for safety and atomic execution tools:

1. `state_checkpoint`:
   - Parameters: `state?: string`
   - Resolve effective state path and repository root.
   - Load state to retrieve `state.step`.
   - Call `superstate_core::checkpoint::create_checkpoint(&repo_root, state.step)`.
   - Return confirmation:
     ```json
     {
       "checkpoint": tag,
       "step": state.step,
       "content": [{ "type": "text", "text": format!("Checkpoint created: {}", tag) }]
     }
     ```

2. `state_rollback`:
   - Parameters: `state?: string`, `reference?: string`
   - Resolve effective state path and repository root.
   - Load state.
   - If `state.active_worktree` exists:
     - Check if worktree directory exists on disk. If so, abort it cleanly via `IsolatedWorktree { branch, path, base_branch, repo_root }.abort()`.
     - Clear `state.active_worktree = None; state.active_branch = None;` and save state atomically.
   - Call `superstate_core::checkpoint::rollback_to(&repo_root, state.step, reference, &[".agent-state"])`.
   - Return confirmation:
     ```json
     {
       "target": target,
       "step": state.step,
       "content": [{ "type": "text", "text": format!("Working tree cleanly rolled back to {}", target) }]
     }
     ```

3. `state_exec`:
   - Parameters:
     - `verify_cmd: string` (required)
     - `state?: string`
     - `verify_git_clean?: boolean` (default false)
     - `checkpoint?: boolean` (default false)
     - `rollback_on_fail?: boolean` (default false)
     - `advance?: boolean` (default true)
     - `json?: boolean` (default false)
   - Follow the exact logic of `cmd_exec` in `crates/superstate-cli/src/commands/state.rs`:
     - If `checkpoint == true`: call `create_checkpoint(&repo_root, state.step)`.
     - Determine target directory: `state.active_worktree` if existing, else `repo_root`.
     - Run verification command: `superstate_core::verifier::run_verification_cmd(&verify_cmd, &target_dir, 600)`.
     - If `verify_git_clean == true` and gate passed, run `verify_git_clean(&target_dir)`.
     - Calculate `passed = gate.passed && git_clean_passed`.
     - If `passed && advance`: advance state machine (using `advance_step` or delegating to the `state_next` flow) and save state.
     - If `!passed`: record failure message into `state.errors` and save state. If `rollback_on_fail == true`: abort active worktree if present, and run `rollback_to(&repo_root, state.step, None, &[".agent-state"])`.
     - Return JSON-RPC response with `passed`, `advanced`, `rolled_back`, `output`, `step`, and text content.

## Steps (TDD)
1. Write failing integration tests in `crates/superstate-mcp/tests/test_mcp_state_safety.rs`:
   - Test `state_checkpoint` creates git tag `agent-checkpoint/step-N`.
   - Test `state_rollback` restores modified file to checkpoint and preserves `.agent-state/state.json`.
   - Test `state_exec` passes with clean tree and advances step when `advance: true`.
   - Test `state_exec` fails when command fails, records error in state, and rolls back modified file when `rollback_on_fail: true`.
2. Run `cargo test -p superstate-mcp --test test_mcp_state_safety` to verify failure.
3. Implement the 3 handlers in `crates/superstate-mcp/src/server.rs`.
4. Run `cargo test -p superstate-mcp --test test_mcp_state_safety` to verify passing.
5. Check `cargo clippy -p superstate-mcp` and `cargo fmt -p superstate-mcp --check`.
6. Commit in the worktree:
   `git add crates/superstate-mcp/src/server.rs crates/superstate-mcp/tests/test_mcp_state_safety.rs`
   `git commit -m "feat(mcp): implement state_checkpoint, state_rollback, and state_exec handlers"`
7. Write report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-3-report.md`.
