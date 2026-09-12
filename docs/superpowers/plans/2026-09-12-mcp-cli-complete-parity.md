# Superstate MCP and CLI Complete Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Achieve 100% complete feature parity between the Superstate CLI (`superstate state ...` and `superstate worktree ...`) and the Model Context Protocol (MCP) server, allowing LLM agents to execute every state machine, safety, maintenance, and worktree capability over standard JSON-RPC.

**Architecture:** Extend `crates/superstate-mcp` with full tool declarations and schemas matching CLI arguments, dispatching directly to core business logic in `superstate_core` (`checkpoint`, `maintenance`, `patching`, `verifier`, `state_machine`) and `superstate_worktree` (`manager`, `types`). All state operations respect branch-scoped state resolution for multi-agent concurrency.

**Tech Stack:** Rust 2021, `serde_json`, `thiserror`, `superstate_core`, `superstate_worktree`.

## Global Constraints

- Never duplicate business logic from `superstate_core` or `superstate_worktree` in MCP handlers.
- Respect branch-scoped state routing (`branch_scoped_state_path` and `find_branch_state_for_worktree`).
- Preserve `.agent-state/` and untracked telemetry files during rollback or clean operations.
- All newly exposed tools must appear in `list_all_mcp_tools()`.
- Strict typing and error handling: return informative JSON-RPC errors or error payloads, never panic or unwrap in server logic.

---

### Task 1: MCP Tool Declarations & Schemas (`crates/superstate-mcp/src/tools.rs`)

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-mcp/src/tools.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_tool_schemas.rs`

**Interfaces:**
- Consumes: `McpTool`, `McpToolInputSchema` from `superstate_mcp::tools`
- Produces: Complete list of MCP tool definitions in `list_all_mcp_tools()`:
  - `state_status`, `state_show`, `state_resume`, `state_metrics`
  - `state_checkpoint`, `state_rollback`, `state_exec`
  - `state_ready`, `state_sync`, `state_doctor`, `state_validate`, `state_apply`, `state_list`
  - `worktree_list`, `worktree_remove`, `worktree_clean`
  - Parameter additions to `state_next` (`ignore_untracked_glob`) and `state_prompt` (`with_graph`, `task`).

- [ ] **Step 1: Write failing test verifying tool schemas and complete catalog**

```rust
// crates/superstate-mcp/tests/test_mcp_tool_schemas.rs
use superstate_mcp::tools::list_all_mcp_tools;

#[test]
fn test_all_cli_parity_tools_declared() {
    let tools = list_all_mcp_tools();
    let names: Vec<String> = tools.into_iter().map(|t| t.name).collect();

    let expected = [
        "state_init",
        "state_show",
        "state_status",
        "state_next",
        "state_ready",
        "state_checkpoint",
        "state_rollback",
        "state_prompt",
        "state_sync",
        "state_exec",
        "state_doctor",
        "state_validate",
        "state_apply",
        "state_resume",
        "state_metrics",
        "state_list",
        "state_note",
        "worktree_create",
        "worktree_merge",
        "worktree_list",
        "worktree_remove",
        "worktree_clean",
        "superstate_review",
        "superstate_preflight",
        "superstate_handoff",
    ];

    for name in expected {
        assert!(
            names.contains(&name.to_string()),
            "Tool '{}' missing from list_all_mcp_tools()",
            name
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-mcp --test test_mcp_tool_schemas`
Expected: FAIL due to missing tool definitions.

- [ ] **Step 3: Implement tool definitions in `crates/superstate-mcp/src/tools.rs`**

Add all tool definitions to `list_all_mcp_tools()`:
- `state_show`: properties `state`, `json`
- `state_status`: properties `state`, `json`
- `state_resume`: properties `state`
- `state_metrics`: properties `state`, `json`
- `state_checkpoint`: properties `state`
- `state_rollback`: properties `state`, `reference`
- `state_exec`: properties `verify_cmd` (required), `state`, `verify_git_clean`, `checkpoint`, `rollback_on_fail`, `advance`
- `state_ready`: properties `state`, `json`
- `state_sync`: properties `plan` (required), `state`, `out`
- `state_doctor`: properties `state`, `plan`, `repair`, `json`
- `state_validate`: properties `state`
- `state_apply`: properties `state`, `patch`, `note`, `out`
- `state_list`: properties `state`, `json`
- `worktree_list`: properties `repo`
- `worktree_remove`: properties `branch` (required), `force`, `repo`
- `worktree_clean`: properties `repo`
- Update `state_next`: add `ignore_untracked_glob` property
- Update `state_prompt`: add `with_graph`, `task` properties

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-mcp --test test_mcp_tool_schemas`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-mcp/src/tools.rs crates/superstate-mcp/tests/test_mcp_tool_schemas.rs
git commit -m "feat(mcp): declare schemas for all CLI parity tools and parameters"
```

---

### Task 2: State Inspection, Status & Briefing Handlers (`crates/superstate-mcp/src/server.rs`)

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-mcp/src/server.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_state_inspection.rs`

**Interfaces:**
- Consumes:
  - `load_state`, `resolve_project_root` from `superstate_core`
  - `superstate_core::maintenance::{resume_briefing, metrics}`
  - `superstate_core::ledger::read_ledger`
- Produces: Handlers in `handle_tool_call` for `state_status`, `state_show`, `state_resume`, `state_metrics`.

- [ ] **Step 1: Write integration tests in `crates/superstate-mcp/tests/test_mcp_state_inspection.rs`**

```rust
// Test state_status, state_show, state_resume, state_metrics over handle_tool_call
// Verifies structured JSON and formatted markdown returns
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-mcp --test test_mcp_state_inspection`
Expected: FAIL ("Unknown tool: state_status")

- [ ] **Step 3: Implement handlers in `crates/superstate-mcp/src/server.rs`**

- In `handle_tool_call`:
  - Route `"state_status"`: load state, read ledger, return status JSON or text.
  - Route `"state_show"`: load state, render markdown status or JSON representation.
  - Route `"state_resume"`: load state, fetch recent 5 git commits via `Command::new("git").args(["log", "--oneline", "-5"])`, read ledger, return `resume_briefing(&state, &notes, &commits)`.
  - Route `"state_metrics"`: load state, read ledger, call `metrics(&state, notes.len())`, return JSON or formatted metrics summary.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-mcp --test test_mcp_state_inspection`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-mcp/src/server.rs crates/superstate-mcp/tests/test_mcp_state_inspection.rs
git commit -m "feat(mcp): implement state_status, state_show, state_resume, and state_metrics handlers"
```

---

### Task 3: Safety, Checkpoint, Rollback & Atomic Exec Handlers (`crates/superstate-mcp/src/server.rs`)

- **Depends on:** Task 2

**Files:**
- Modify: `crates/superstate-mcp/src/server.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_state_safety.rs`

**Interfaces:**
- Consumes:
  - `create_checkpoint`, `rollback_to` from `superstate_core::checkpoint`
  - `execute_all_gates_filtered`, `run_verification_cmd`, `verify_git_clean` from `superstate_core::verifier`
  - `advance_step`, `load_state`, `save_state_atomic` from `superstate_core::state_machine`
  - `IsolatedWorktree` from `superstate_worktree::manager`
- Produces: Handlers in `handle_tool_call` for `state_checkpoint`, `state_rollback`, `state_exec`.

- [ ] **Step 1: Write integration tests in `crates/superstate-mcp/tests/test_mcp_state_safety.rs`**

- Test checkpoint creation generates `agent-checkpoint/step-N` tag.
- Test rollback restores modified file to checkpoint and preserves `.agent-state/state.json`.
- Test `state_exec` passes with clean tree and advances step when `advance: true`.
- Test `state_exec` fails with dirty tree or bad command, rolls back when `rollback_on_fail: true`, and records error in state.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-mcp --test test_mcp_state_safety`
Expected: FAIL ("Unknown tool: state_checkpoint")

- [ ] **Step 3: Implement handlers in `crates/superstate-mcp/src/server.rs`**

- In `handle_tool_call`:
  - Route `"state_checkpoint"`: call `create_checkpoint(&repo_root, state.step)`.
  - Route `"state_rollback"`: if `state.active_worktree` exists, abort worktree; call `rollback_to(&repo_root, state.step, reference, &[".agent-state"])`.
  - Route `"state_exec"`:
    - If `checkpoint: true`, create checkpoint before running.
    - Run verification command and optional `verify_git_clean`.
    - If passed and `advance: true`: advance step, save state.
    - If failed: record error into `state.errors`. If `rollback_on_fail: true`, abort active worktree and rollback to checkpoint.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-mcp --test test_mcp_state_safety`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-mcp/src/server.rs crates/superstate-mcp/tests/test_mcp_state_safety.rs
git commit -m "feat(mcp): implement state_checkpoint, state_rollback, and state_exec handlers"
```

---

### Task 4: Maintenance, Reconciliation, DAG & Parameter Parity (`crates/superstate-mcp/src/server.rs`)

- **Depends on:** Task 3

**Files:**
- Modify: `crates/superstate-mcp/src/server.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_maintenance_and_dag.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_tool_parameter_parity.rs`

**Interfaces:**
- Consumes:
  - `ready_tasks` from `superstate_core::state_machine`
  - `sync_with_plan`, `diagnose`, `repair` from `superstate_core::maintenance`
  - `validate_state`, `apply_patch`, `state_to_value` from `superstate_core::patching`
  - `execute_all_gates_filtered` with `ignore_untracked_glob`
  - `render_turnkey_prompt` with `with_graph` and `task`
- Produces:
  - Handlers for `state_ready`, `state_sync`, `state_doctor`, `state_validate`, `state_apply`, `state_list`.
  - Parameter handling in `state_next` (`ignore_untracked_glob`) and `state_prompt` (`with_graph`, `task`).

- [ ] **Step 1: Write integration tests in `crates/superstate-mcp/tests/`**

- `test_mcp_maintenance_and_dag.rs`: test ready tasks in multi-task state, plan sync, state doctor diagnosis, schema validation, patch application, and branch state listing.
- `test_mcp_tool_parameter_parity.rs`: test `state_next` with `ignore_untracked_glob`, and `state_prompt` with `task` selection and `with_graph`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p superstate-mcp --test test_mcp_maintenance_and_dag`
Expected: FAIL

- [ ] **Step 3: Implement handlers and parameter forwarding in `crates/superstate-mcp/src/server.rs`**

- Route `"state_ready"`: call `ready_tasks(&state)`, return list of titles or JSON array.
- Route `"state_sync"`: read plan file, call `sync_with_plan(&mut state, &plan)`, save updated state.
- Route `"state_doctor"`: call `diagnose(&state, plan.as_deref())`. If `repair: true`, call `repair(&mut state)`.
- Route `"state_validate"`: read state JSON, call `validate_state(&raw)`.
- Route `"state_apply"`: if `patch` provided, call `apply_patch(&mut state, &patch)`. If `note` provided, append to ledger. Validate resulting state and write to disk.
- Route `"state_list"`: scan `.agent-state/` for state files and worktrees, return list.
- In `"state_next"`: extract `ignore_untracked_glob: Option<String>` and pass to `execute_all_gates_filtered`.
- In `"state_prompt"`: extract `task: Option<String>` and `with_graph: Option<bool>` and pass to `select_task` and `render_turnkey_prompt`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p superstate-mcp --test test_mcp_maintenance_and_dag && cargo test -p superstate-mcp --test test_mcp_tool_parameter_parity`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-mcp/src/server.rs crates/superstate-mcp/tests/test_mcp_maintenance_and_dag.rs crates/superstate-mcp/tests/test_mcp_tool_parameter_parity.rs
git commit -m "feat(mcp): implement maintenance, reconciliation, DAG tools, and parameter parity"
```

---

### Task 5: Worktree Lifecycle Handlers (`crates/superstate-mcp/src/server.rs`)

- **Depends on:** Task 4

**Files:**
- Modify: `crates/superstate-mcp/src/server.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_worktree_lifecycle.rs`

**Interfaces:**
- Consumes:
  - `WorktreeManager`, `IsolatedWorktree` from `superstate_worktree::manager`
- Produces: Handlers in `handle_tool_call` for `worktree_list`, `worktree_remove`, `worktree_clean`.

- [ ] **Step 1: Write integration tests in `crates/superstate-mcp/tests/test_mcp_worktree_lifecycle.rs`**

- Test `worktree_list` returns active worktrees created with `worktree_create`.
- Test `worktree_remove` removes worktree path and branch.
- Test `worktree_clean` cleans orphaned `.wt/*` directories.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-mcp --test test_mcp_worktree_lifecycle`
Expected: FAIL ("Unknown tool: worktree_list")

- [ ] **Step 3: Implement worktree handlers in `crates/superstate-mcp/src/server.rs`**

- Route `"worktree_list"`: call `WorktreeManager::new(&repo_root)?.list_worktrees()`, return JSON list of worktree paths, branches, and commit SHAs.
- Route `"worktree_remove"`: call `WorktreeManager::new(&repo_root)?.remove_worktree(&branch, force)`.
- Route `"worktree_clean"`: call `WorktreeManager::new(&repo_root)?.clean_orphaned_worktrees()`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-mcp --test test_mcp_worktree_lifecycle`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-mcp/src/server.rs crates/superstate-mcp/tests/test_mcp_worktree_lifecycle.rs
git commit -m "feat(mcp): implement worktree_list, worktree_remove, and worktree_clean handlers"
```

---

### Task 6: Full Verification, Binary Build & End-to-End Suite

- **Depends on:** Tasks 1-5

**Files:**
- Test: Full cargo workspace test suite

- [ ] **Step 1: Run full cargo test suite**

Run: `cargo test --all`
Expected: All tests pass cleanly across all crates.

- [ ] **Step 2: Run linter and formatter**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: 0 warnings, 0 errors, clean formatting.

- [ ] **Step 3: Build and reinstall updated CLI binary**

Run: `cargo install --path crates/superstate-cli --force`
Expected: Successful installation to `~/.cargo/bin/superstate`.

- [ ] **Step 4: Commit and finalize**

```bash
git commit --allow-empty -m "chore(mcp): complete verification and binary build for MCP CLI parity"
```
