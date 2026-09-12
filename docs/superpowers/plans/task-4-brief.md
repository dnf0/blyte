# Task 4 Brief: Maintenance, Reconciliation, DAG & Parameter Parity (`crates/superstate-mcp/src/server.rs`)

- **Working Directory:** `/Users/danielfisher/repos/superstate/.wt/feat-mcp-cli-complete-parity-task-4`
- **Branch:** `wt/feat-mcp-cli-complete-parity-task-4`
- **Depends on:** Task 3

## Files
- Modify: `crates/superstate-mcp/src/server.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_maintenance_and_dag.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_tool_parameter_parity.rs`

## Requirements
Implement handlers in `handle_tool_call` for the maintenance and DAG tools, plus parameter parity for `state_next` and `state_prompt`:

1. `state_ready`:
   - Parameters: `state?: string`, `json?: boolean`
   - Resolve effective state path and load state.
   - Call `superstate_core::state_machine::ready_tasks(&state)`.
   - If `json == true`: return JSON array of task titles (`"Task 1: Title"`).
   - If `json == false`: format human-readable text listing ready tasks.
   - Return `{ "ready": [...], "content": [{ "type": "text", "text": ... }] }`.

2. `state_sync`:
   - Parameters: `plan: string` (required path to plan markdown), `state?: string`, `out?: string`
   - Resolve state path, load state. Read plan file contents.
   - Call `superstate_core::maintenance::sync_with_plan(&mut state, &plan_content)`.
   - Update `state.plan_path = Some(PathBuf::from(plan_path))`.
   - Save state atomically to `out.unwrap_or(state_path)`.
   - Return sync report (added tasks, removed tasks, preserved completed count, etc.) in structured JSON and text.

3. `state_doctor`:
   - Parameters: `state?: string`, `plan?: string`, `repair?: boolean` (default false), `json?: boolean` (default false)
   - Resolve state path, load state.
   - If `plan` is provided, read its content as `Option<String>`.
   - Call `superstate_core::maintenance::diagnose(&state, plan.as_deref())`.
   - If `repair == true`:
     - Call `superstate_core::maintenance::repair(&mut state)`.
     - Save repaired state atomically.
   - Return diagnosis report (`is_healthy`, `issues`, `warnings`, `repaired_actions`).

4. `state_validate`:
   - Parameters: `state?: string`
   - Read raw JSON of state file into `serde_json::Value`.
   - Call `superstate_core::patching::validate_state(&raw)`.
   - If errors is empty: return `{ "valid": true, "content": [{ "type": "text", "text": "State is valid." }] }`.
   - If errors non-empty: return `{ "valid": false, "errors": errors, "content": [{ "type": "text", "text": format!("State is invalid ({} problems).", errors.len()) }] }`.

5. `state_apply`:
   - Parameters:
     - `state?: string`
     - `patch?: string | object` (allow either path string or inline JSON object)
     - `note?: string`
     - `out?: string`
   - If both `patch` and `note` are None: return error "Nothing to apply: pass patch and/or note."
   - Read state JSON into `serde_json::Value`.
   - If `patch` is provided:
     - If string: check if file exists, read JSON. If not file, parse as JSON string.
     - If object: use directly.
     - Call `superstate_core::patching::apply_patch(&mut state, &patch_value)`.
   - If `note` is provided:
     - Call `superstate_core::ledger::append_ledger_note(&state_path, LedgerEntry::new("agent", note))`.
   - Validate patched state with `superstate_core::patching::validate_state(&state)`. If invalid, refuse to save and return error.
   - Save patched state atomically to `out.unwrap_or(state_path)`.
   - Return confirmation in JSON and text.

6. `state_list`:
   - Parameters: `state?: string` (optional state path or directory to scan, defaults to repo root `.agent-state`), `json?: boolean`
   - Discover `.agent-state/state.json` and any branch states in `.agent-state/branches/*.json`.
   - Also scan `.wt/*` worktree directories if repo exists.
   - Return list of state files, their branch names, active tasks, and status.

7. Parameter parity updates on existing tools:
   - In `state_next`:
     - Extract `ignore_untracked_glob: Option<String>` from `args`.
     - Forward `ignore_untracked_glob.as_deref()` to `superstate_core::verifier::execute_all_gates_filtered`.
   - In `state_prompt`:
     - Extract `with_graph: Option<bool>` and `task: Option<String>` from `args`.
     - Support selecting a specific task in the pending set via `superstate_core::prompt::select_task(&state, task.as_deref())`.
     - Pass `with_graph.unwrap_or(false)` to `superstate_core::prompt::render_turnkey_prompt`.

## Steps (TDD)
1. Write failing integration tests:
   - `crates/superstate-mcp/tests/test_mcp_maintenance_and_dag.rs`:
     - Test `state_ready`: returns ready tasks in multi-task state.
     - Test `state_sync`: reconciles revised plan and preserves completed tasks.
     - Test `state_doctor`: diagnoses drift and repairs missing fields.
     - Test `state_validate`: confirms valid state or reports schema errors.
     - Test `state_apply`: applies patch and records note.
     - Test `state_list`: discovers primary and branch state files.
   - `crates/superstate-mcp/tests/test_mcp_tool_parameter_parity.rs`:
     - Test `state_next` ignores untracked files matching `ignore_untracked_glob`.
     - Test `state_prompt` targeting specific task and with `with_graph: true`.
2. Run tests to verify failure.
3. Implement handlers and parameter forwarding in `crates/superstate-mcp/src/server.rs`.
4. Run tests to verify they pass.
5. Check `cargo clippy -p superstate-mcp` and `cargo fmt -p superstate-mcp --check`.
6. Commit in the worktree:
   `git add crates/superstate-mcp/src/server.rs crates/superstate-mcp/tests/`
   `git commit -m "feat(mcp): implement maintenance, reconciliation, DAG tools, and parameter parity"`
7. Write report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-4-report.md`.
