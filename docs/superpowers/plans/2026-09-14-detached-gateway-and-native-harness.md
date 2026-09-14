# Detached Gateway & Native Rust Harness Implementation Plan

**Goal:** Establish the Detached Handoff Protocol and Native Rust Daemon Harness as the singular execution engine of Superstate, permanently removing in-loop MCP mutation tools per ADR 0002.

**Architecture:**
- **Interactive Layer (`superstate-mcp`):** Exposes strictly 4 Gateway tools (`superstate_handoff`, `superstate_status`, `superstate_cancel`, `superstate_doctor`). Enforces `detach: true` by default. Completely removes internal in-loop tools (`state_init`, `state_next`, `state_prompt`, etc.) from the MCP catalog.
- **Execution Layer (`superstate-core`):** The native Rust `DaemonRunner` in `crates/superstate-core/src/daemon.rs` owns 100% of the execution lifecycle (worktree isolation, zero-context prompt injection, headless driver invocation, deterministic verification gates, error-feedback retries, atomic checkpoint commits, worktree auto-merge, and completion artifact emission).

---

## Proposed Changes

### 1. `crates/superstate-mcp/src/server.rs`
- In `handle_list_tools`, prune the returned tools list down to strictly:
  1. `superstate_handoff` (with `detach` defaulting to `true`)
  2. `superstate_status`
  3. `superstate_cancel`
  4. `superstate_doctor`
- In `handle_call_tool`:
  - Retain handlers for the 4 gateway tools.
  - Return a descriptive error if any deprecated in-loop tool is called:
    `"The tool '{name}' has been deprecated. In-loop MCP execution is an anti-pattern. Use 'superstate_handoff' to initiate autonomous detached execution in an isolated worktree."`
  - Ensure `superstate_handoff` defaults to `detach: true` unless explicitly overridden.

### 2. `crates/superstate-mcp/tests/`
- Update existing MCP integration tests (`test_mcp_server.rs`, etc.) to assert that `tools/list` returns exactly the 4 Gateway tools.
- Add test verifying that `superstate_handoff` defaults to detached execution and successfully spawns the background daemon.

### 3. `crates/superstate-core/src/daemon.rs` & `crates/superstate-cli/src/commands/daemon.rs`
- Add native worktree auto-merge on final milestone completion:
  When the final milestone passes verification and the state machine transitions to `Completed`:
  - Merge the worktree feature branch back into the base branch cleanly.
  - Remove the temporary `.wt/<slug>` worktree.
  - Write `.agent-state/completion.json` containing status, milestones completed, base branch, commit SHA, and timing metadata.
- Emit completion signal (terminal bell / OS notification hook).

---

## Detailed Task Breakdown

### Task 1: Prune MCP Server Tool Catalog to Strict Gateway (TDD)
1. **Write failing test:** In `crates/superstate-mcp/tests/test_gateway_tools.rs`:
   - Test `tools/list` returns exactly `["superstate_handoff", "superstate_status", "superstate_cancel", "superstate_doctor"]`.
   - Test that calling legacy `state_next` or `state_init` returns the descriptive deprecation error directing to `superstate_handoff`.
2. **Implement changes:**
   - In `crates/superstate-mcp/src/server.rs`, update `handle_list_tools` to register only the 4 Gateway tools.
   - Update `handle_call_tool` to reject legacy in-loop tools with clear deprecation guidance.
   - Enforce `detach: true` default in `superstate_handoff`.
3. **Verify:**
   `cargo test -p superstate-mcp --test test_gateway_tools`
4. **Commit:**
   `git commit -m "feat(mcp): streamline tool catalog to strict detached handoff gateway"`

### Task 2: Native Worktree Auto-Merge & Completion Artifact in Daemon (TDD)
1. **Write failing test:** In `crates/superstate-core/tests/test_daemon_automerge.rs`:
   - Setup a temporary repository with a base branch and a multi-milestone task.
   - Run `DaemonRunner` through completion.
   - Assert that the worktree branch is merged cleanly into the base branch.
   - Assert that `.agent-state/completion.json` exists and contains valid JSON metadata.
2. **Implement changes:**
   - In `crates/superstate-core/src/daemon.rs`, enhance `DaemonRunner::run_step` and completion handling to execute worktree merge, worktree removal, and completion artifact emission upon task graph completion.
3. **Verify:**
   `cargo test -p superstate-core --test test_daemon_automerge`
4. **Commit:**
   `git commit -m "feat(core): implement automatic worktree merge and completion artifact on daemon success"`

### Task 3: Full End-to-End Gateway Handoff Integration Test
1. **Write integration test:** In `crates/superstate-mcp/tests/test_e2e_detached_handoff.rs`:
   - Call `superstate_handoff` via JSON-RPC stdio.
   - Verify that preflight runs, worktree is created, daemon is detached with PID file, and prompt is generated.
2. **Verify:**
   `cargo test --workspace`
   `cargo clippy --all-targets -- -D warnings`
   `cargo fmt --all -- --check`
3. **Commit:**
   `git commit -m "test(mcp): add end-to-end integration test for detached handoff gateway"`
