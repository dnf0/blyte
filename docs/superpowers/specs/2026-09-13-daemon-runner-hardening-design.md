# Design Specification: DaemonRunner Hardening & Multi-Daemon Fleet Lifecycle

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

To support real developer workflows and multi-agent systems, Superstate requires:
1. Hardened autonomous execution (resilient driver execution, rich retry diagnostics, worktree merge/provisioning, git checkpoints).
2. **Multi-Daemon Concurrent Execution:** Running multiple autonomous background daemons simultaneously across different feature branches without PID collisions, log overwrites, or git index lockups.

---

## 2. Architecture & Components

### 2.1 Multi-Daemon Fleet Scoping & File Layout
Every daemon is keyed to its state file or branch slug. Rather than global singleton PID and log files, all runtime artifacts are scoped per job:

| Resource | Path Template | Description |
| :--- | :--- | :--- |
| **State File** | `.agent-state/branches/<slug>.json` | State machine DAG and task history |
| **Daemon PID** | `.agent-state/daemons/<slug>.pid` | Process ID for liveness and cancellation |
| **Daemon Log** | `.agent-state/logs/<slug>.log` | Stored stdout/stderr of daemon and driver |
| **Prompt File**| `.agent-state/prompts/<slug>.md` | Current active milestone prompt |

*(For trunk/default `.agent-state/state.json`, slug defaults to `default`).*

### 2.2 Pluggable Step Advancement (`StepAdvancer`)
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
  - In `run_step()`: when verification passes, invoke the configured advancer if present. It merges the task worktree, advances the task, and provisions the next task worktree. If None, falls back to in-process `advance_step(&mut state)` and `save_state_atomic(...)`.

### 2.3 Resilient Driver Execution & Rescue Pattern
- In `DaemonRunner::run_step()`:
  - Spawn driver with stdout/stderr piped and prompt delivered via `SUPERSTATE_PROMPT`, `SUPERSTATE_PROMPT_FILE`, and child `stdin`.
  - If driver exit code != 0, do not abort the daemon immediately. Record `[driver-warn]` in the epistemic ledger.
  - Always evaluate `run_verification_cmd`.
  - If gate passes, the milestone succeeds (rescues agent from minor tooling exit quirks).
  - If gate fails, record error output and increment retry count.

### 2.4 High-Fidelity Failure Feedback on Retries
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

### 2.5 Milestone Git Checkpoint in In-Place Mode
- When `state.no_worktree` is true and `.git` exists:
  - Upon gate pass, run `git status --porcelain`.
  - If changes detected: `git add -A && git commit -m "chore(superstate): checkpoint completed milestone {id}: {title}"`.

### 2.6 Multi-Daemon Management CLI & MCP
- **Per-Job PID Lock Guard:** Check `.agent-state/daemons/<slug>.pid`. Only error if a daemon is already active for *the same slug*.
- **Daemon Fleet Listing (`superstate state daemon list`):**
  Inspects `.agent-state/daemons/*.pid` and matching `.agent-state/branches/*.json` to report all running, completed, or failed daemons with PID, status, active task, and log path.
- **Targeted Logs (`superstate state logs [--state <path> | --branch <slug>] [--tail <n>]`):**
  Tails the log file for that specific job.
- **Targeted & Mass Cancellation (`superstate state cancel [--state <path> | --branch <slug> | --all]`):**
  Terminates target PID(s) via SIGTERM followed by SIGKILL and cleans up PID file(s).

---

## 3. Verification & Testing Strategy
1. **Core Unit & Integration Tests:**
   - Test `StepAdvancer` callback invocation upon gate pass.
   - Test resilient driver rescue (driver fails with code 1, but creates target file -> gate passes -> step advances).
   - Test rich retry prompt generation on verification failure.
2. **CLI Integration Tests:**
   - Test multi-daemon concurrent execution: spawn two daemons on distinct branch states simultaneously, verify independent PIDs, distinct logs, and `daemon list`.
   - Test per-job PID lock guard (prevents duplicate daemon on same branch while allowing different branches).
   - Test `superstate cancel --all`.
3. **E2E Shell Script:**
   - Update `tests/e2e_detached_handoff.sh` to test worktree mode, stdin piping, and multi-daemon fleet listing.
