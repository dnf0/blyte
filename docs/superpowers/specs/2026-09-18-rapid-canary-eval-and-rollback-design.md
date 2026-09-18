# Rapid Canary Evaluation and Deterministic Daemon Rollback Design

## 1. Problem Statement & Motivation
Long evaluation cycles (6–12 hours for a 14-task core benchmark) stifle rapid iteration and make architectural discovery slow and risky. Furthermore, in existing evaluation runs, when an agent fails verification on an attempt, it continues editing on top of its corrupted, partially-broken working tree, compounding errors across retry turns.

This design introduces two coupled components:
1. **The Canary 3 Evaluation Slice**: A fast, representative, 12–15 minute evaluation suite (`scripts/run_canary.sh` and `--canary` flag) targeting 3 high-discriminating tasks across different build and language stacks.
2. **Deterministic Daemon Rollback**: An automated rollback mechanism in `superstate` daemon (`--auto-rollback`) that restores the git working tree to the milestone start commit upon verification failure, clearing contaminated state and giving the worker a clean slate with explicit epistemic failure notes.

---

## 2. Architecture & Components

### Component A: The "Canary 3" Slice (`superstate-eval`)
- **Task Selection**:
  1. `terminal-bench/build-cython-ext`: C/Python extension compilation, build scripts, repository structure (~4 min).
  2. `terminal-bench/chess-best-move`: Algorithmic logic, Python script generation, precise constraint following (~3 min).
  3. `datacurve/httpx-multipart-response-parsing`: Python repository maintenance, RFC-compliant multipart MIME parsing, test suite pass (~5 min).
- **Harness Integration**:
  - Add `CANARY_TASKS = ("build-cython-ext", "chess-best-move", "httpx-multipart-response-parsing")` to `lab/benchmarks/task_sets.py`.
  - Add `--canary` CLI flag to `scripts/paired_run.py` and `scripts/runner.py`.
  - Create turnkey execution script `scripts/run_canary.sh` running 3 tasks paired (6 total runs) at concurrency 2 with output `results/canary_<timestamp>.json`.
  - Total execution duration: ~12–15 minutes.

### Component B: Deterministic Git Rollback in `superstate`
- **Configuration**:
  - Add `pub auto_rollback: bool` to `DaemonConfig` in `crates/superstate-core/src/daemon.rs`.
  - Add `--auto-rollback` flag to `DaemonArgs` in `crates/superstate-cli/src/state_cli.rs`, exposed on both `superstate daemon` and `superstate state daemon`.
  - Pass `--auto-rollback` through detachment in `cmd_daemon`.
  - Environment variable fallback: `SUPERSTATE_AUTO_ROLLBACK=1`.
- **State Machine Integration in `DaemonRunner::step()`**:
  - When milestone execution begins, record `milestone_start_commits[active_task.id] = git rev-parse HEAD`.
  - When verification fails (`!gate_res.passed` or review blocks) AND `attempts_so_far < attempt_ceiling`:
    - If `self.config.auto_rollback && is_inside_git_repo(&working_dir)`:
      - If `milestone_start_commits` has recorded the start commit for this task:
        - Run `git reset --hard <start_commit>` in `working_dir`.
        - Run `git clean -fd` in `working_dir` to remove any untracked garbage files created by the failed attempt.
        - Add an epistemic note to `state.notes`:
          `[rollback] Task {id}: attempt {attempt} failed verification ({reason}). Working tree rolled back to clean baseline commit {start_commit}. Avoid repeating the previous failed approach.`
  - On the subsequent retry attempt, the driver worker is invoked in a pristine workspace matching the milestone start commit.

---

## 3. Data Flow & Execution Lifecycle

```
                     [ Milestone Start ]
                              │
               Record `milestone_start_commit`
                              │
                              ▼
                     [ Driver Worker Turn ]
                              │
                              ▼
                 [ Verification Gate Check ]
                              │
           ┌──────────────────┴──────────────────┐
           ▼                                     ▼
        PASSES                                 FAILS
           │                                     │
    [ Code Review ]                     [ Rollback Check ]
    (if active)                                  │
    ├── Clean ──► Checkpoint & Advance           ├── auto_rollback == true:
    └── Blocks                                   │   1. `git reset --hard <start_commit>`
            │                                    │   2. `git clean -fd`
            │                                    │   3. Record `[rollback]` note
            ▼                                    │   4. Decrement retry budget
      Trigger Rollback ◄─────────────────────────┘
```

---

## 4. Verification & Testing Strategy

### Tier 1: Deterministic Offline Tests (< 1 second)
- `crates/superstate-core/tests/test_daemon_rollback.rs`:
  - Initialize a test git repository.
  - Create milestone with failing verification gate.
  - Driver script creates a dirty file `dirty.txt` and fails.
  - Assert that after daemon step:
    - Step outcome is `DaemonStepOutcome::Failed`.
    - `dirty.txt` is eliminated by `git reset --hard` and `git clean -fd`.
    - `state.notes` contains the `[rollback]` entry.
- `crates/superstate-cli/tests/test_cli_rollback.rs`:
  - Test CLI argument parsing and propagation of `--auto-rollback`.

### Tier 2: Canary Benchmark (~12–15 minutes)
- Wire `SuperstateDaemonAdapter` in `superstate-eval` to accept `auto_rollback: bool` (and `SUPERSTATE_EVAL_AUTO_ROLLBACK=1`).
- Execute `scripts/run_canary.sh` comparing `superstate_daemon` with `--auto-rollback` vs `superstate_mcp`.
- Verify telemetry: ensure tasks that fail on attempt 1 trigger clean rollbacks and record `[rollback]` notes in `extra` telemetry.
