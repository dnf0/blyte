# DaemonRunner Hardening & Multi-Daemon Fleet Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden the Superstate autonomous daemon runner with worktree lifecycle integration, resilient driver rescue, high-fidelity retry feedback, milestone git checkpointing, and multi-daemon fleet management across concurrent branches.

**Architecture:** 
- In `superstate-core`: decouple worktree operations via a pluggable `StepAdvancer` hook on `DaemonRunner`, implement driver rescue (evaluating verification gates regardless of driver exit code), provide unflattened compiler/test stderr on retries (up to 2,000 chars), auto-checkpoint in-place git commits, and pipe prompts via stdin/prompt files.
- In `superstate-cli`: replace global daemon PID/logs with state-scoped paths (`.agent-state/daemons/<slug>.pid`, `.agent-state/logs/<slug>.log`), attach the full `cmd_next` worktree lifecycle via `StepAdvancer`, implement per-job PID locking, and provide fleet commands (`daemon list`, `logs --branch`, `cancel --all`).
- In `superstate-mcp`: expose multi-daemon visibility in `state_status`, `state_list`, and `superstate_handoff`.

**Tech Stack:** Rust 2021, `libc` (process sessions/signals), `serde`/`serde_json`, `clap` v4, `tempfile`.

## Global Constraints
- Target workspace: `/Users/danielfisher/repos/superstate/.wt/feat-detached-handoff-runner` on branch `feat/detached-handoff-runner`.
- Follow strict TDD: failing test first, minimal implementation, passing test, commit.
- Atomic Conventional Commits (`feat:`, `test:`, `fix:`).
- Zero clippy warnings (`cargo clippy --workspace --all-targets -- -D warnings`).
- Maintain 100% backward compatibility for existing MCP tools and CLI subcommands.

---

### Task 1: Core Daemon Hardening in `superstate-core`

- **Depends on:** none
- **Files:**
  - Modify: `crates/superstate-core/src/daemon.rs`
  - Modify: `crates/superstate-core/src/prompt.rs`
  - Modify: `crates/superstate-core/src/lib.rs`
  - Test: `crates/superstate-core/tests/test_daemon_runner.rs`

**Interfaces:**
- Consumes: `ExecutionState`, `run_verification_cmd`, `render_turnkey_prompt`, `save_state_atomic`.
- Produces:
  ```rust
  pub type StepAdvancer = Box<dyn FnMut(&Path) -> Result<(), String> + Send + 'static>;
  impl DaemonRunner {
      pub fn with_step_advancer(mut self, advancer: StepAdvancer) -> Self;
  }
  ```

- [ ] **Step 1: Write failing tests for StepAdvancer callback, driver rescue, and rich retry prompts**

In `crates/superstate-core/tests/test_daemon_runner.rs`, add:
```rust
#[test]
fn test_daemon_step_advancer_invoked() { ... }

#[test]
fn test_daemon_driver_rescue_on_nonzero_exit() { ... }

#[test]
fn test_daemon_rich_retry_prompt_on_failure() { ... }

#[test]
fn test_daemon_auto_commit_on_pass_in_place() { ... }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p superstate-core --test test_daemon_runner`
Expected: FAIL due to missing `with_step_advancer` and existing driver crash behavior.

- [ ] **Step 3: Implement core hardening in `daemon.rs`**

1. Define `StepAdvancer` type and add `step_advancer: Option<StepAdvancer>` and `last_failure_output: HashMap<String, String>` to `DaemonRunner`.
2. Implement `with_step_advancer`.
3. In `run_step()`:
   - Write prompt to `.agent-state/prompts/<slug>.md` and export `SUPERSTATE_PROMPT_FILE`.
   - Pipe prompt to driver's stdin.
   - If driver exit code != 0, log `[driver-warn]` to `state.notes`, but do not return `Err`. Proceed to evaluate `run_verification_cmd`.
   - If gate passes:
     - If in-place mode and git repo exists, run `git add -A && git commit -m "chore(superstate): checkpoint completed milestone {id}: {title}"`.
     - If `self.step_advancer` is Some: call advancer.
     - Else: call `advance_step(&mut state)` and `save_state_atomic`.
   - If gate fails:
     - Store unflattened stderr/output in `last_failure_output`.
     - On subsequent retries, inject `## 🚨 PREVIOUS ATTEMPT VERIFICATION FAILURE` block into prompt.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p superstate-core --test test_daemon_runner`
Expected: PASS.

- [ ] **Step 5: Commit Task 1**

```bash
git add crates/superstate-core/
git commit -m "feat(core): harden DaemonRunner with StepAdvancer, driver rescue, and rich retries"
```

---

### Task 2: Multi-Daemon Fleet Architecture & PID Scoping in `superstate-cli`

- **Depends on:** Task 1
- **Files:**
  - Modify: `crates/superstate-cli/src/commands/daemon.rs`
  - Modify: `crates/superstate-cli/src/commands/state.rs`
  - Modify: `crates/superstate-cli/src/main.rs`
  - Modify: `crates/superstate-cli/src/state_cli.rs`
  - Test: `crates/superstate-cli/tests/test_cli_daemon_lifecycle.rs`
  - Test: `crates/superstate-cli/tests/test_cli_multi_daemon_fleet.rs`

**Interfaces:**
- Consumes: `DaemonRunner::with_step_advancer`, `cmd_next`.
- Produces:
  ```rust
  pub struct DaemonPaths {
      pub pid_file: PathBuf,
      pub log_file: PathBuf,
      pub prompt_file: PathBuf,
      pub slug: String,
  }
  pub fn daemon_paths_for_state(state_path: &Path) -> DaemonPaths;
  pub fn cmd_daemon_list(repo_root: &Path, as_json: bool) -> Result<(), String>;
  ```

- [ ] **Step 1: Write failing integration test for multi-daemon fleet and per-job PID lock**

Create `crates/superstate-cli/tests/test_cli_multi_daemon_fleet.rs`:
- Test spawning two background daemons on two distinct branches (`feat/alpha` and `feat/beta`).
- Assert both have distinct PID files in `.agent-state/daemons/` and log files in `.agent-state/logs/`.
- Assert attempting to spawn a second daemon on `feat/alpha` fails with a concurrency error.
- Assert `superstate state daemon list` displays both active daemons.
- Assert `superstate state cancel --all` terminates both daemons cleanly.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate --test test_cli_multi_daemon_fleet`
Expected: FAIL.

- [ ] **Step 3: Implement multi-daemon paths, PID locking, and fleet commands**

1. Implement `daemon_paths_for_state(state_path: &Path) -> DaemonPaths`:
   - Scopes to `.agent-state/daemons/<slug>.pid`, `.agent-state/logs/<slug>.log`, `.agent-state/prompts/<slug>.md`.
2. In `cmd_daemon`:
   - Enforce PID lock: if `pid_file` exists and `is_pid_alive`, return `Err`.
   - Wire `runner.with_step_advancer(Box::new(|p| cmd_next(p, None, false, None)))`.
3. Implement `cmd_daemon_list` to scan `.agent-state/daemons/*.pid` and correlate with state files.
4. Update `cmd_cancel`: support `--branch <slug>`, `--state <path>`, and `--all`.
5. Update `cmd_logs`: support `--branch <slug>`.
6. Expose CLI subcommands `superstate state daemon list` and update CLI parsers.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p superstate --test test_cli_multi_daemon_fleet && cargo test -p superstate --test test_cli_daemon_lifecycle`
Expected: PASS.

- [ ] **Step 5: Commit Task 2**

```bash
git add crates/superstate-cli/
git commit -m "feat(cli): support multi-daemon fleet management, state-scoped PIDs, and worktree integration"
```

---

### Task 3: MCP Fleet Visibility & Tool Parity in `superstate-mcp`

- **Depends on:** Task 2
- **Files:**
  - Modify: `crates/superstate-mcp/src/tools.rs`
  - Modify: `crates/superstate-mcp/src/server.rs`
  - Test: `crates/superstate-mcp/tests/test_mcp_detached_handoff.rs`
  - Test: `crates/superstate-mcp/tests/test_mcp_multi_daemon_fleet.rs`

**Interfaces:**
- Consumes: `daemon_paths_for_state`, `is_pid_alive`.
- Produces:
  - `superstate_handoff` returns `slug`, `job_id`, `pid`, `log_path`.
  - `state_status` returns `daemon: { pid, running, slug, log_file }`.
  - `state_list` reports daemon status for all discovered branch states.

- [ ] **Step 1: Write failing test in `superstate-mcp` for multi-daemon tracking**

Create `crates/superstate-mcp/tests/test_mcp_multi_daemon_fleet.rs`:
- Launch two detached handoffs across two different branches.
- Query `state_list` and assert both branches report active daemon PIDs.
- Query `state_status` for branch 1 and branch 2 and verify independent logs.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-mcp --test test_mcp_multi_daemon_fleet`
Expected: FAIL.

- [ ] **Step 3: Implement multi-daemon MCP reporting**

1. Update `superstate_handoff` in `server.rs` to use `daemon_paths_for_state`.
2. Update `state_status` to resolve daemon PID file via `daemon_paths_for_state`.
3. Update `state_list` to inspect `.agent-state/daemons/*.pid` and include daemon status in JSON output.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p superstate-mcp --test test_mcp_multi_daemon_fleet && cargo test -p superstate-mcp --test test_mcp_detached_handoff`
Expected: PASS.

- [ ] **Step 5: Commit Task 3**

```bash
git add crates/superstate-mcp/
git commit -m "feat(mcp): add multi-daemon fleet visibility to state_list, state_status, and handoff"
```

---

### Task 4: End-to-End Multi-Daemon & Worktree Integration Verification

- **Depends on:** Tasks 1, 2, 3
- **Files:**
  - Modify: `tests/e2e_detached_handoff.sh`
  - Create: `tests/e2e_multi_daemon_fleet.sh`

- [ ] **Step 1: Update `tests/e2e_detached_handoff.sh`**

Add assertions for:
- Worktree lifecycle merge and cleanup under `DaemonRunner`.
- Auto-checkpoint verification.
- Stdin / prompt file delivery.

- [ ] **Step 2: Create `tests/e2e_multi_daemon_fleet.sh`**

Author shell script testing:
1. Spawning two detached daemons on `feat/job-a` and `feat/job-b`.
2. Running `superstate state daemon list` and verifying both PIDs.
3. Polling both to completion.
4. Verifying both branch commits and `completion.json` files.

- [ ] **Step 3: Run full workspace test and lint suite**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
bash tests/e2e_detached_handoff.sh
bash tests/e2e_multi_daemon_fleet.sh
```

- [ ] **Step 4: Commit Task 4**

```bash
git add tests/
git commit -m "test(e2e): verify multi-daemon fleet execution and worktree lifecycle"
```
