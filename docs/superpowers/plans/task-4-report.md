# Task 4: Maintenance, Reconciliation, DAG & Parameter Parity Handlers Report

**Status:** Completed  
**Branch:** `wt/feat-mcp-cli-complete-parity-task-4`  
**Commit:** `95fe1ec4eb0c7e5566ccc018b7437fe4657a388e`  
**Author:** Subagent (Task 4)  
**Date:** 2026-09-12  

---

## 1. Overview & Objectives

Task 4 completes full feature and parameter parity between the Superstate CLI and the Superstate MCP server (`crates/superstate-mcp/src/server.rs`). Specifically:
1. `state_ready`: Inspects multi-task DAG dependencies and returns unblocked tasks.
2. `state_sync`: Reconciles state with a revised plan markdown file, preserving already-completed work and updating `plan_path`.
3. `state_doctor`: Diagnoses execution state drift (missing active tasks, duplicate entries across status sets, plan discrepancies) and optionally applies automated atomic repairs.
4. `state_validate`: Performs schema validation on state documents, reporting violations without mutating files.
5. `state_apply`: Applies JSON merge patches with null-deletion semantics, validates resulting state against schemas prior to persistence, appends findings to the epistemic ledger (`ledger.jsonl`), and supports alternative output paths.
6. `state_list`: Discovers and summarizes execution states across primary checkouts (`.agent-state/state.json`), branch states (`.agent-state/branches/*.json`), and isolated git worktrees (`.wt/*`).
7. **Parameter Parity**:
   - `state_next`: Added support for `ignore_untracked_glob` (passed through to `execute_all_gates_filtered`).
   - `state_prompt`: Added support for `task` (targeting specific pending tasks by ID/title), `with_graph` (injecting knowledge graph reports), and `no_worktree`.

---

## 2. Implementation Summary

### Maintenance & DAG Handlers (`crates/superstate-mcp/src/server.rs`)

1. **`state_ready`**:
   - Parameters: `state?: string`, `json?: boolean`, `repo_root?: string`.
   - Resolves state path via `resolve_mcp_state_path`.
   - Evaluates task dependency DAG with `superstate_core::state_machine::ready_tasks(&state)`.
   - Returns structured JSON containing `"ready": ["Task 1: Title", ...]` alongside human-readable text.

2. **`state_sync`**:
   - Parameters: `plan: string` (required), `state?: string`, `out?: string`, `repo_root?: string`.
   - Reads plan markdown and reconciles state with `superstate_core::maintenance::sync_with_plan(&mut state, &plan_content)`.
   - Preserves completed tasks regardless of plan alterations.
   - Sets `state.plan_path = Some(plan_path)`.
   - Atomically saves reconciled state to `out.unwrap_or(state_path)`.
   - Returns structured `SyncReport` detailing added, removed, preserved completed counts, active task, and pending tasks.

3. **`state_doctor`**:
   - Parameters: `state?: string`, `plan?: string`, `repair?: boolean`, `json?: boolean`, `repo_root?: string`.
   - Runs diagnostic checks via `superstate_core::maintenance::diagnose(&state, plan_content.as_deref())`.
   - When `repair == true`: executes `superstate_core::maintenance::repair(&mut state)` and writes repaired state atomically to disk.
   - Returns diagnosis report including `is_healthy`, `issues`, `warnings`, `repaired_actions`, and formatted diagnostic output.

4. **`state_validate`**:
   - Parameters: `state?: string`, `repo_root?: string`.
   - Loads raw JSON from state file and validates schema invariants using `superstate_core::patching::validate_state(&raw)`.
   - Returns `valid: true` with empty errors list on success, or `valid: false` with detailed error descriptions if invalid.

5. **`state_apply`**:
   - Parameters: `patch?: string | object`, `note?: string`, `out?: string`, `state?: string`, `repo_root?: string`.
   - Rejects invocation if both `patch` and `note` are omitted.
   - Applies patch using `superstate_core::patching::apply_patch(&mut state_val, &patch)`.
   - Appends epistemic notes to `ledger.jsonl` using `superstate_core::ledger::append_ledger_note`.
   - Pre-validates patched document against schema using `validate_state(&state_val)`; aborts without saving if invalid.
   - Persists atomically via `save_json_atomic(&save_path, &state_val)`.

6. **`state_list`**:
   - Parameters: `state?: string`, `json?: boolean`, `repo_root?: string`.
   - Discovers state across:
     - `.agent-state/branches/*.json`
     - `.agent-state/state.json`
     - `.wt/*/.agent-state/state.json`
   - Maps summaries into `Vec<BranchStateSummary>` with branch name, status, active task, step progress, and base branch.
   - Returns both machine-readable JSON array and tabular formatted ASCII representation.

### Parameter Parity Updates (`crates/superstate-mcp/src/server.rs`)

1. **`state_next`**:
   - Extracts `ignore_untracked_glob: Option<String>` from `args`.
   - Forwards `ignore_untracked_glob.as_deref()` to `superstate_core::verifier::execute_all_gates_filtered`.
   - Correctly bypasses verification failure for whitelisted untracked files while maintaining strict cleanliness checks for other files.

2. **`state_prompt`**:
   - Extracts `task: Option<String>`, `with_graph: Option<bool>`, and `no_worktree: Option<bool>` from `args`.
   - Forwards parameters to `superstate_core::prompt::render_turnkey_prompt`, enabling targeted task prompt generation and knowledge graph injection.

---

## 3. Test-Driven Development (TDD) Lifecycle

1. **RED Phase**:
   - Created integration tests in `crates/superstate-mcp/tests/test_mcp_maintenance_and_dag.rs`:
     - `test_state_ready_returns_unblocked_tasks`
     - `test_state_sync_reconciles_plan_and_preserves_completed`
     - `test_state_doctor_diagnoses_and_repairs`
     - `test_state_validate_valid_and_invalid`
     - `test_state_apply_patch_and_note`
     - `test_state_list_discovers_states`
   - Created integration tests in `crates/superstate-mcp/tests/test_mcp_tool_parameter_parity.rs`:
     - `test_state_next_ignore_untracked_glob`
     - `test_state_prompt_targeting_specific_task`
     - `test_state_prompt_with_graph_parameter`
   - Ran `cargo test -p superstate-mcp --test test_mcp_maintenance_and_dag && cargo test -p superstate-mcp --test test_mcp_tool_parameter_parity`.
   - Verified that all new tests failed with expected errors (`Unknown tool` or missing parameter behavior).

2. **GREEN Phase**:
   - Implemented parameter extraction and forwarding in `state_next` and `state_prompt`.
   - Implemented tool dispatch arms for `state_ready`, `state_sync`, `state_doctor`, `state_validate`, `state_apply`, and `state_list`.
   - Re-ran tests: all 9 new integration tests passed.

3. **Refactor & Code Quality**:
   - Cleaned up unused imports in `server.rs`.
   - Ran `cargo fmt -p superstate-mcp --check` and formatted code cleanly.
   - Ran `cargo clippy -p superstate-mcp -- -D warnings` — zero warnings.
   - Ran complete crate test suite `cargo test -p superstate-mcp`: all 44 tests passed across 9 integration test suites.

---

## 4. Git Commit Details

- **Commit**: `95fe1ec4eb0c7e5566ccc018b7437fe4657a388e`
- **Message**: `feat(mcp): implement maintenance, reconciliation, DAG tools, and parameter parity`
- **Files**:
  - `crates/superstate-mcp/src/server.rs` (+656, -10)
  - `crates/superstate-mcp/tests/test_mcp_maintenance_and_dag.rs` (+452)
  - `crates/superstate-mcp/tests/test_mcp_tool_parameter_parity.rs` (+221)
