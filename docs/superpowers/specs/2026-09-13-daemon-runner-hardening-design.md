# Design Specification: DaemonRunner Hardening & Production Lifecycle

**Date:** 2026-09-13  
**Status:** Approved  
**Scope:** `crates/superstate-core`, `crates/superstate-cli`, `crates/superstate-mcp`

---

## 1. Context & Motivation

In benchmark evaluations (`superstate-eval`), the Category B Deterministic State arm achieved superior performance because an external Python supervisor (`lab/engine`) strictly governed the milestone lifecycle:
1. Turnkey prompt rendering with strict negative constraints.
2. Headless driver execution in an isolated workspace.
3. Verification gate check via OS exit codes (evaluating gate regardless of driver exit status).
4. Atomic state advancement, worktree merging, and next-task worktree provisioning.
5. High-fidelity feedback on retries if verification fails.

The initial implementation of `DaemonRunner` in `superstate-core` proved the concept of a detached daemon, but had 6 production gaps:
1. Worktree lifecycle missing (did not merge completed worktree or provision next worktree).
2. Fragile driver exit (non-zero driver exit aborted the entire daemon instead of checking gate/retrying).
3. Low-fidelity failure feedback (retries only saw a 250-char flattened note).
4. No milestone git checkpoints (uncommitted changes left dirty in working tree).
5. No PID concurrency guard (allowed spawning competing daemons against the same state).
6. Single-channel prompt delivery (only via env var `SUPERSTATE_PROMPT`).

---

## 2. Architecture & Components

### 2.1 Pluggable Step Advancement (`StepAdvancer`)
- **Crate:** `superstate-core` (`daemon.rs`)
- **Type Definition:**
  ```rust
  pub type StepAdvancer = Box<dyn FnMut(&Path) -> Result<(), String> + Send + 'static>;
  ```
- **Lifecycle Integration:**
  - `DaemonRunner::with_step_advancer(mut self, advancer: StepAdvancer) -> Self`
  - In `superstate-cli` (`cmd_daemon`), configure:
    ```rust
    let runner = DaemonRunner::new(config)?
        .with_step_advancer(Box::new(|state_path| {
            superstate_cli::commands::state::cmd_next(state_path, None, false, None)
        }));
    ```
  - In `run_step()`: when gate passes, invoke the configured advancer if present. If None, fall back to in-process `advance_step(&mut state)` and `save_state_atomic(...)`.

### 2.2 Resilient Driver Execution & Rescue Pattern
- In `DaemonRunner::run_step()`:
  - Spawn driver with stdout/stderr piped or captured.
  - If driver exit code != 0, do not abort immediately. Record `[driver-warn]` in ledger.
  - Always evaluate `run_verification_cmd`.
  - If gate passes, the milestone succeeds (rescues agent from minor tooling exit quirks).
  - If gate fails, record error output and increment retry count.

### 2.3 High-Fidelity Failure Feedback on Retries
- `DaemonRunner` stores `last_failure_output: HashMap<String, String>`.
- When rendering prompt for retry attempt > 0:
  - Append formatted Markdown section:
    ```markdown
    ## 🚨 PREVIOUS ATTEMPT VERIFICATION FAILURE (Attempt {attempt} of {max_retries})
    The verification command `{verify_cmd}` failed with exit code {exit_code}:
    ```
    {full_stderr_or_output_up_to_2000_chars}
    ```
    Please diagnose and correct this failure before concluding your turn.
    ```

### 2.4 Milestone Git Checkpoint in In-Place Mode
- When `state.no_worktree` is true and `.git` exists:
  - Upon gate pass, run `git status --porcelain`.
  - If changes detected: `git add -A && git commit -m "chore(superstate): checkpoint completed milestone {id}: {title}"`.

### 2.5 PID Concurrency Guard
- In `superstate-cli` (`cmd_daemon`):
  - Check if `daemon.pid` exists.
  - If exists: read PID, call `is_pid_alive(pid)`.
  - If alive: return `Err("A superstate daemon is already running (PID: ...)")`.
  - If stale: remove file and continue.

### 2.6 Multi-Channel Prompt Delivery
- Write prompt to `.agent-state/current_prompt.md` (or branch-scoped directory).
- Export `SUPERSTATE_PROMPT_FILE` containing path to file.
- Pipe prompt bytes to driver's `stdin`.

---

## 3. Verification & Testing Strategy
1. **Core Unit & Integration Tests:**
   - Test `StepAdvancer` callback invocation upon gate pass.
   - Test resilient driver rescue (driver fails with code 1, but creates target file -> gate passes -> step advances).
   - Test rich retry prompt generation on verification failure.
2. **CLI Integration Tests:**
   - Test PID lock guard blocking concurrent spawns.
   - Test worktree merge & next-worktree provisioning under `DaemonRunner`.
3. **E2E Shell Script:**
   - Update `tests/e2e_detached_handoff.sh` to exercise worktree mode, stdin piping, and PID lock verification.
