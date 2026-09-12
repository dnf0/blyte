# Task 2 Brief: State Inspection, Status & Briefing Handlers (`crates/superstate-mcp/src/server.rs`)

- **Working Directory:** `/Users/danielfisher/repos/superstate/.wt/feat-mcp-cli-complete-parity-task-2`
- **Branch:** `wt/feat-mcp-cli-complete-parity-task-2`
- **Depends on:** Task 1

## Files
- Modify: `crates/superstate-mcp/src/server.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_state_inspection.rs`

## Requirements
Implement handlers in `handle_tool_call` for the 4 inspection tools:
1. `state_status`:
   - Parameters: `state?: string`, `json?: boolean`
   - Resolve effective state path via branch-scoped routing (`find_branch_state_for_worktree` or `branch_scoped_state_path` or resolve path).
   - Load state via `load_state`.
   - Read ledger entries via `superstate_core::ledger::read_ledger`.
   - If `json == true`: return JSON-serialized `state` (matching `superstate state status --json`).
   - If `json == false` (or not specified): format human-readable text output:
     ```text
     State Status: {:?}
     Step: {}
     Active Task: {}
     Completed Tasks: {}
     Ledger Entries: {}
       ✓ task...
     Recent Notes (Epistemic Ledger):
       • [author] note
     ```
2. `state_show`:
   - Parameters: `state?: string`, `json?: boolean`
   - If `json == true`: return JSON-serialized state.
   - If `json == false`: return Markdown formatted summary matching `superstate state show`:
     ```markdown
     # Superstate Execution Status
     - **Status:** {:?} (Step {})
     - **Base Branch:** `{}` (if present)
     - **Active Task:** {}: {} (or None)
     - **Verification Command:** `{}` (if present)
     - **Active Worktree:** `{}` (branch `{}`) (if present)
     ```
3. `state_resume`:
   - Parameters: `state?: string`
   - Resolve effective state path and repo root.
   - Load state and ledger notes.
   - Query last 5 git commits:
     ```rust
     let commits = std::process::Command::new("git")
         .args(["log", "--oneline", "-5"])
         .current_dir(&repo_root)
         .output()
         ...
     ```
   - Return `superstate_core::maintenance::resume_briefing(&state, &notes, &commits)`.
4. `state_metrics`:
   - Parameters: `state?: string`, `json?: boolean`
   - Load state, read ledger.
   - Compute `let m = superstate_core::maintenance::metrics(&state, notes.len());`
   - If `json == true`: serialize `m` to JSON.
   - If `json == false`: format telemetry text matching `superstate state metrics`.

## Steps (TDD)
1. Write failing integration tests in `crates/superstate-mcp/tests/test_mcp_state_inspection.rs`:
   - Initialize a temporary repo with `.agent-state/state.json` containing active task, 1 completed task, and a ledger note.
   - Test JSON-RPC calls to `state_status`, `state_show`, `state_resume`, and `state_metrics`.
   - Verify each returns successful response with expected markdown/text and JSON formats.
2. Run `cargo test -p superstate-mcp --test test_mcp_state_inspection` to verify failure.
3. Implement the 4 handlers in `crates/superstate-mcp/src/server.rs`.
4. Run `cargo test -p superstate-mcp --test test_mcp_state_inspection` to verify passing.
5. Check `cargo clippy -p superstate-mcp` and `cargo fmt -p superstate-mcp --check`.
6. Commit in the worktree directory:
   `git add crates/superstate-mcp/src/server.rs crates/superstate-mcp/tests/test_mcp_state_inspection.rs`
   `git commit -m "feat(mcp): implement state_status, state_show, state_resume, and state_metrics handlers"`
7. Write report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-2-report.md`.
