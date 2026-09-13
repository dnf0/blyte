# Detached Handoff Protocol & Daemon Runner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the Superstate Detached Handoff Protocol and autonomous Background Daemon Runner to allow LLM chat sessions to detach completely upon handoff, executing multi-milestone tasks in isolated worktrees with deterministic verification gates and live status interrogation.

**Architecture:** 
1. `superstate-core` implements `DaemonRunner` orchestrating milestone iterations, headless driver invocation, test verification, and atomic git checkpoint merges.
2. `superstate-cli` implements the `daemon`, `status` (PID-aware), `logs`, and `cancel` subcommands for process lifecycle and live interrogation.
3. `superstate-mcp` updates `superstate_handoff` to support `detach: true`, launching the background daemon and immediately releasing the calling chat agent.

**Tech Stack:** Rust 2021, Cargo, `clap` (derive), `serde`, `serde_json`, Unix process APIs (`nix` / `std::process`), Git CLI.

## Global Constraints

- Never break existing CLI flags or MCP tool schemas.
- Ensure all subprocess invocations use strict timeouts and detached process groups (`setsid` on Unix).
- All changes must pass `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`.
- Strictly adhere to Conventional Commits format (`feat:`, `fix:`, `chore:`, `test:`).

---

### Task 1: `DaemonRunner` Execution Engine in `superstate-core`

- **Depends on:** none

**Files:**
- Create: `/Users/danielfisher/repos/superstate/crates/superstate-core/src/daemon.rs`
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-core/src/lib.rs`
- Test: `/Users/danielfisher/repos/superstate/crates/superstate-core/tests/test_daemon_runner.rs`

**Interfaces:**
- Consumes: `ExecutionState`, `WorktreeManager`, `cmd_prompt`, `run_verification`
- Produces: 
  ```rust
  pub struct DaemonConfig {
      pub state_path: PathBuf,
      pub driver: String,
      pub max_retries: usize,
      pub lease_timeout_sec: u64,
  }
  pub struct DaemonRunner;
  impl DaemonRunner {
      pub fn new(config: DaemonConfig) -> Self;
      pub fn run_step(&mut self) -> Result<DaemonStepOutcome, StateError>;
      pub fn run_to_completion(&mut self) -> Result<CompletionReport, StateError>;
  }
  ```

- [ ] **Step 1: Write integration tests in `crates/superstate-core/tests/test_daemon_runner.rs`**
  - Test `run_step` with passing synthetic driver (`echo 'diff'`) and passing `verify_cmd` (`true`): verifies checkpoint is committed, step advances.
  - Test `run_step` with failing `verify_cmd` (`false`): verifies failure is recorded in notes and task retry counter increments.
  - Test `run_to_completion`: verifies all milestones execute sequentially and writes `.agent-state/completion.json`.

- [ ] **Step 2: Run test to confirm failure**
  ```bash
  cargo test -p superstate-core --test test_daemon_runner
  ```

- [ ] **Step 3: Implement `DaemonRunner` in `crates/superstate-core/src/daemon.rs`**
  - Implement loop over `ExecutionState.active_task`.
  - Format turnkey prompt using existing prompt generator.
  - Execute driver command (supporting `claude`, `agy`, or shell command) in worktree directory.
  - Execute `verify_cmd` using `std::process::Command`.
  - Advance task and merge worktree using `superstate_worktree` on success.
  - Record error diff and manage retry backoff on failure.

- [ ] **Step 4: Export module in `crates/superstate-core/src/lib.rs`**
  - Add `pub mod daemon;` and re-export `DaemonRunner`, `DaemonConfig`.

- [ ] **Step 5: Verify unit tests pass**
  ```bash
  cargo test -p superstate-core --test test_daemon_runner
  cargo clippy -p superstate-core --all-targets -- -D warnings
  ```

- [ ] **Step 6: Commit Task 1**
  ```bash
  git add crates/superstate-core/
  git commit -m "feat(core): implement DaemonRunner for autonomous milestone execution"
  ```

---

### Task 2: Detached Process Lifecycle & CLI Interrogation in `superstate-cli`

- **Depends on:** Task 1

**Files:**
- Create: `/Users/danielfisher/repos/superstate/crates/superstate-cli/src/commands/daemon.rs`
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-cli/src/state_cli.rs`
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-cli/src/commands/state.rs`
- Test: `/Users/danielfisher/repos/superstate/crates/superstate-cli/tests/test_cli_daemon_lifecycle.rs`

**Interfaces:**
- Consumes: `DaemonRunner` from `superstate-core`
- Produces:
  - CLI: `superstate daemon [--state <path>] [--driver <name>] [--detach]`
  - CLI: `superstate status [--json]` (with daemon PID check & live progress)
  - CLI: `superstate logs [--tail <N>] [--follow]`
  - CLI: `superstate cancel [--job <id>]`

- [ ] **Step 1: Write integration tests in `crates/superstate-cli/tests/test_cli_daemon_lifecycle.rs`**
  - Test `superstate daemon --detach` spawns background process, writes `.agent-state/daemon.pid`.
  - Test `superstate status` shows PID and running milestone status.
  - Test `superstate cancel` sends `SIGTERM` and kills the process cleanly.

- [ ] **Step 2: Run test to confirm failure**
  ```bash
  cargo test -p superstate-cli --test test_cli_daemon_lifecycle
  ```

- [ ] **Step 3: Update `StateCommands` enum in `crates/superstate-cli/src/state_cli.rs`**
  - Add `Daemon`, `Logs`, `Cancel` variants.
  - Update `StatusArgs` to support `--json` and PID inspection.

- [ ] **Step 4: Implement `commands/daemon.rs` and update `commands/state.rs`**
  - Implement detached process fork (using `std::process::Command` detached with redirection to `.agent-state/logs/daemon.log`).
  - Write PID to `.agent-state/daemon.pid`.
  - Implement `cmd_status` PID liveness checking (`kill -0 <pid>`).
  - Implement `cmd_cancel` process termination.

- [ ] **Step 5: Verify tests pass**
  ```bash
  cargo test -p superstate-cli --test test_cli_daemon_lifecycle
  cargo clippy -p superstate-cli --all-targets -- -D warnings
  ```

- [ ] **Step 6: Commit Task 2**
  ```bash
  git add crates/superstate-cli/
  git commit -m "feat(cli): add daemon execution, live status interrogation, and cancel commands"
  ```

---

### Task 3: MCP `superstate_handoff` Detached Execution Support

- **Depends on:** Task 2

**Files:**
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-mcp/src/tools.rs`
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-mcp/src/server.rs`
- Test: `/Users/danielfisher/repos/superstate/crates/superstate-mcp/tests/test_mcp_detached_handoff.rs`

**Interfaces:**
- Consumes: `superstate daemon --detach` CLI command
- Produces: MCP `superstate_handoff` tool response with `status: "detached"` and job ID.

- [ ] **Step 1: Write integration test in `crates/superstate-mcp/tests/test_mcp_detached_handoff.rs`**
  - Test calling `superstate_handoff` with `detach: true` returns `status: "detached"`, provisions worktree, and spawns daemon.

- [ ] **Step 2: Run test to confirm failure**
  ```bash
  cargo test -p superstate-mcp --test test_mcp_detached_handoff
  ```

- [ ] **Step 3: Update `superstate_handoff` schema in `crates/superstate-mcp/src/tools.rs`**
  - Add `detach` (boolean, default: true) and `driver` (string, optional) parameters.

- [ ] **Step 4: Implement detached dispatch in `crates/superstate-mcp/src/server.rs`**
  - Run preflight check on `verify_cmd`.
  - Initialize state via `init_state_milestones`.
  - If `detach` is true, invoke `superstate daemon --detach` via background command.
  - Return immediate JSON response: `{ "status": "detached", "job_id": "...", "worktree_path": "..." }`.

- [ ] **Step 5: Verify all MCP tests pass**
  ```bash
  cargo test -p superstate-mcp
  cargo clippy -p superstate-mcp --all-targets -- -D warnings
  ```

- [ ] **Step 6: Commit Task 3**
  ```bash
  git add crates/superstate-mcp/
  git commit -m "feat(mcp): support detached autonomous execution in superstate_handoff"
  ```

---

### Task 4: End-to-End System Verification & Dogfooding

- **Depends on:** Task 1, Task 2, Task 3

**Files:**
- Create: `/Users/danielfisher/repos/superstate/tests/e2e_detached_handoff.sh`
- Test: `/Users/danielfisher/repos/superstate/tests/e2e_detached_handoff.sh`

**Interfaces:**
- Consumes: Full binary suite (`superstate state init`, `superstate daemon`, `superstate status`)
- Produces: Verified end-to-end multi-milestone detached run with zero terminal blocking.

- [ ] **Step 1: Write end-to-end verification script `tests/e2e_detached_handoff.sh`**
  - Creates temporary test git repository.
  - Runs `superstate state init --milestones '[{"title":"m1","verify_cmd":"test -f a.txt"},{"title":"m2","verify_cmd":"test -f b.txt"}]'`.
  - Spawns daemon detached with a synthetic driver script that creates `a.txt` then `b.txt`.
  - Polls `superstate state status` until completion.
  - Asserts commits exist on the base branch and `completion.json` is generated.

- [ ] **Step 2: Run e2e test script and verify clean exit 0**
  ```bash
  bash tests/e2e_detached_handoff.sh
  ```

- [ ] **Step 3: Run full repository test and lint suite**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --check
  ```

- [ ] **Step 4: Commit Task 4**
  ```bash
  git add tests/e2e_detached_handoff.sh
  git commit -m "test(e2e): add end-to-end verification script for detached handoff execution"
  ```
