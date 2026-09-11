# Direct State Handoff & Plan-Free Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate `plan.md` from the execution control plane by introducing direct CLI state initialization, markdown-on-demand views, an MCP pre-flight and handoff server for Claude Code and `agy`, and zero-plan evaluation adapter integration.

**Architecture:**
1. `superstate-core` adds `init_state_direct` constructing typed `ExecutionState` directly from goal and verify command without reading or creating markdown files.
2. `superstate-cli` updates `InitArgs` to support `--goal` and `--verify-cmd` as first-class flags alongside `--plan`, and implements `superstate state show --markdown`.
3. `superstate-mcp` implements `superstate_preflight`, `superstate_handoff`, and complete JSON-RPC `tools/call` dispatch.
4. `superstate-eval` updates `SuperstateAdapter` to initialize state directly via CLI flags, eliminating synthetic `plan.md` files from disk.

**Tech Stack:** Rust (2021 edition), `clap` (derive), `serde`, `serde_json`, `superstate-core`, `superstate-worktree`, `superstate-mcp`, Python 3.12 (`pytest`, `pyright`, `ruff`).

## Global Constraints
- Strictly follow TDD: write failing unit/integration tests before implementing code.
- Zero markdown files (`plan.md`) created on disk during direct initialization.
- Backward compatibility: `--plan <path>` must continue to work for multi-milestone plans.
- All rust crates must pass `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`.
- Python eval harness must pass `ruff check`, `pyright`, and `pytest`.

---

### Task 1: Direct State Initialization & Show Command in `superstate-core` and `superstate-cli`

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-core/src/state_machine.rs`
- Modify: `crates/superstate-cli/src/state_cli.rs`
- Modify: `crates/superstate-cli/src/commands/state.rs`
- Create: `crates/superstate-cli/tests/test_state_init_direct.rs`

**Interfaces:**
- Produces:
  - `superstate_core::state_machine::init_state_direct(goal: &str, verify_cmd: &str, files_modify: Vec<PathBuf>, no_worktree: bool) -> ExecutionState`
  - CLI: `superstate state init [--plan <path>] [--goal <str>] [--verify-cmd <str>] [--files <csv>] [--no-worktree]`
  - CLI: `superstate state show [--markdown] [--json]`

**Steps:**
- [ ] Write failing integration test in `crates/superstate-cli/tests/test_state_init_direct.rs`:
  - Test initializing with `--goal "Fix bug"` and `--verify-cmd "cargo test"` writes `.agent-state/state.json` without any `plan.md` on disk.
  - Test missing `--verify-cmd` when `--goal` is provided fails closed with code 1.
  - Test running `superstate state show` prints markdown formatted view.
- [ ] Run `cargo test --test test_state_init_direct` to confirm test failure.
- [ ] In `crates/superstate-core/src/state_machine.rs`, implement `init_state_direct`:
  - Constructs `TaskSpec` with id `"task-1"`, title `goal`, `files_modify`, and `verification_cmd`.
  - Constructs `ExecutionState` with `active_task: Some(task)`, `status: ExecutionStatus::InProgress`, `step: 1`, `no_worktree`.
- [ ] In `crates/superstate-cli/src/state_cli.rs`:
  - In `InitArgs`: make `plan: Option<PathBuf>`, add `goal: Option<String>`, `verify_cmd: Option<String>`, `files: Option<String>`.
  - In `ShowArgs`: add `markdown: bool`.
- [ ] In `crates/superstate-cli/src/commands/state.rs`:
  - Update `cmd_init` to validate mutual exclusivity / exhaustiveness.
  - If `--plan` is present, use `init_state_from_plan`.
  - If `--goal` is present, ensure `--verify-cmd` is present and call `init_state_direct`.
  - Update `cmd_show` to format markdown when requested or default.
- [ ] Run `cargo test -p superstate-core -p superstate-cli --test test_state_init_direct` and ensure all tests pass.
- [ ] Run `cargo clippy --all-targets -- -D warnings` and `cargo fmt`.
- [ ] Commit: `feat(cli): support direct state initialization from goal and verify-cmd`

---

### Task 2: MCP Pre-Flight and Handoff Tools with `tools/call` Dispatch

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-mcp/src/tools.rs`
- Modify: `crates/superstate-mcp/src/server.rs`
- Create: `crates/superstate-mcp/tests/test_mcp_tools_call.rs`

**Interfaces:**
- Produces:
  - MCP Tools: `superstate_preflight`, `superstate_handoff`
  - JSON-RPC method: `tools/call`

**Steps:**
- [ ] Write failing test in `crates/superstate-mcp/tests/test_mcp_tools_call.rs`:
  - Test `tools/list` returns `superstate_preflight` and `superstate_handoff`.
  - Test `tools/call` with `superstate_preflight` executing `echo 'passed'` returns success and exit code 0.
  - Test `tools/call` with `superstate_handoff` initializes state and returns turnkey prompt.
- [ ] Run `cargo test -p superstate-mcp --test test_mcp_tools_call` to confirm failure.
- [ ] In `crates/superstate-mcp/src/tools.rs`:
  - Register `superstate_preflight` schema (`verify_cmd`).
  - Register `superstate_handoff` schema (`goal`, `verify_cmd`, `files`, `no_worktree`).
- [ ] In `crates/superstate-mcp/src/server.rs`:
  - Implement `tools/call` method handling in `handle_json_rpc`.
  - Implement execution of `superstate_preflight` using `std::process::Command`.
  - Implement execution of `superstate_handoff` invoking `init_state_direct` and `cmd_prompt`.
- [ ] Run `cargo test -p superstate-mcp` and verify all tests pass.
- [ ] Run `cargo clippy --all-targets -- -D warnings` and `cargo fmt`.
- [ ] Commit: `feat(mcp): implement superstate_preflight and superstate_handoff with tools/call dispatch`

---

### Task 3: Evaluation Adapter Direct State Integration in `superstate-eval`

- **Depends on:** Task 1

**Files:**
- Modify: `lab/engine/adapters/superstate.py`
- Modify: `tests/test_adapters.py`

**Interfaces:**
- Consumes: `superstate state init --goal "..." --verify-cmd "..." [--no-worktree]`
- Produces: `SuperstateAdapter` execution with zero disk markdown files.

**Steps:**
- [ ] Write failing test in `tests/test_adapters.py`:
  - Test `test_zero_turn_direct_state_init_produces_no_markdown_file`: verifies that when `zero_turn_planning=True`, `plan.md` does not exist on disk, and `superstate state init` is invoked with `--goal` and `--verify-cmd`.
- [ ] Run `uv run pytest tests/test_adapters.py -k test_zero_turn_direct_state_init` to confirm failure.
- [ ] In `lab/engine/adapters/superstate.py`:
  - When `zero_turn_planning=True`: call `superstate state init --goal <instruction> --verify-cmd <cmd> [--no-worktree]`.
  - Remove code writing synthetic `plan.md` to disk.
  - Set `plan_source = "direct_state_handoff"`.
- [ ] Run `uv run pytest tests/test_adapters.py` and `uv run ruff check lab/ tests/` and `uv run pyright lab/ tests/`.
- [ ] Commit: `feat(eval): wire SuperstateAdapter to direct CLI state initialization without plan.md`
