# Adversarial Code Review Control Plane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire full task goal instructions into the daemon's adversarial review execution, verify blocking and non-destructive retry mechanics, and register `superstate_daemon_review` in `superstate-eval` with complete input parity.

**Architecture:** The daemon control plane evaluates the git diff of completed milestones using `superstate-review` against the full task instruction (`state.goal`). On blocking issues (`Critical`/`High`), completion is blocked, findings are recorded into the Epistemic Ledger (`state.notes`), and the working tree is preserved for iterative refinement.

**Tech Stack:** Rust (superstate-core, superstate-cli, superstate-review), Python 3.12 (superstate-eval, pytest, ruff).

## Global Constraints

- All Rust changes must compile with `cargo check --all` and pass `cargo test --all` with zero warnings.
- Clean conventional commits with `--no-gpg-sign`.
- All Python changes must run through `.venv/bin/python` and pass `ruff check` and `pytest`.
- Maintain strict arm input parity across all evaluation arms.

---

### Task 1: Forward Full Goal Instructions to Review in `superstate-core`

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-core/src/daemon.rs:780-795`
- Test: `crates/superstate-core/tests/test_daemon_blocking_review.rs`

**Interfaces:**
- Consumes: `state.goal: Option<String>`, `active_task.title: String`
- Produces: `review_instruction: &str` passed to `superstate_review::execute_review`

- [ ] **Step 1: Write failing test in `crates/superstate-core/tests/test_daemon_blocking_review.rs`**

```rust
#[test]
fn test_daemon_review_uses_full_state_goal_when_available() {
    let dir = setup_test_git_repo();
    let repo = dir.path();
    let state_path = repo.join(".agent-state/state.json");

    let long_instruction = "Comprehensive Task Objective: Fix cython extension compatibility with Numpy 2.3";
    let milestones = vec![MilestoneSpec {
        title: "Short Title".to_string(),
        verify_cmd: "test -f README.md".to_string(),
        files: vec![],
        ..Default::default()
    }];
    let state = init_state_milestones(Some(long_instruction), milestones, None, true).unwrap();
    save_state_atomic(&state_path, &state).unwrap();

    fs::write(repo.join("README.md"), "# Broken code\n").unwrap();
    Command::new("git").args(["add", "README.md"]).current_dir(repo).output().unwrap();
    Command::new("git").args(["commit", "-m", "feat: broken", "--no-gpg-sign"]).current_dir(repo).output().unwrap();

    // Mock driver that fails if the prompt does not contain the full goal
    let mock_driver = format!(
        "python3 -c 'import sys, json; prompt = sys.stdin.read(); assert \"Comprehensive Task Objective\" in prompt; print(json.dumps({{\"summary\": \"Missing Cython compilation\", \"findings\": [{{\"severity\": \"High\", \"file\": \"README.md\", \"line_range\": [1,1], \"problem\": \"Uncompiled\", \"suggested_fix\": \"build\"}}], \"has_blocking_issues\": True}}))'"
    );

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(mock_driver),
        max_retries: 2,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: true,
        review_blocking: true,
        auto_rollback: false,
        shunt_driver_output: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(matches!(outcome, DaemonStepOutcome::Failed { .. }));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-core --test test_daemon_blocking_review test_daemon_review_uses_full_state_goal_when_available`
Expected: FAIL (assertion fails because mock driver did not see "Comprehensive Task Objective")

- [ ] **Step 3: Update `crates/superstate-core/src/daemon.rs` to pass `state.goal`**

In `crates/superstate-core/src/daemon.rs`:
```rust
            let review_instruction = state.goal.as_deref().unwrap_or(&active_task.title);
            if let Ok(report) = superstate_review::execute_review(
                &working_dir,
                &diff,
                Some(review_instruction),
                &review_cfg,
            ) {
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-core --test test_daemon_blocking_review`
Expected: PASS (all 6 tests pass)

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-core/src/daemon.rs crates/superstate-core/tests/test_daemon_blocking_review.rs
git commit --no-gpg-sign -m "feat(daemon): forward full state goal to adversarial code review"
```

---

### Task 2: Reinstall CLI & Verify Cli Tests in `superstate`

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-cli/src/commands/daemon.rs` (if needed)
- Test: `crates/superstate-cli/tests/test_cli_blocking_review.rs`

- [ ] **Step 1: Run CLI blocking review tests**

Run: `cargo test -p superstate --test test_cli_blocking_review`
Expected: PASS

- [ ] **Step 2: Run full workspace clippy and test suite**

Run: `cargo clippy --all-targets -- -D warnings && cargo test --all`
Expected: PASS with 0 warnings and 0 failures

- [ ] **Step 3: Reinstall release binary to system**

Run: `cargo install --path crates/superstate-cli --locked --force`
Expected: Installed `superstate` binary in `~/.cargo/bin/superstate`

- [ ] **Step 4: Verify installed binary version and help**

Run: `~/.cargo/bin/superstate state daemon --help`
Expected: Shows `--advisory-review`, `--review-blocking`, `--auto-rollback` flags

---

### Task 3: Adapter Wiring & Parity in `superstate-eval`

- **Depends on:** Task 2

**Files:**
- Modify: `lab/engine/adapters/superstate_daemon.py:190-200`
- Modify: `lab/engine/runner.py:150-160`
- Test: `tests/test_superstate_daemon_adapter.py`
- Test: `tests/test_arm_input_parity.py`

**Interfaces:**
- Consumes: `SuperstateDaemonAdapter(advisory_review=True, review_blocking=True, auto_rollback=False, name="superstate_daemon_review")`
- Produces: CLI spawn arguments: `["superstate", "state", "daemon", ..., "--advisory-review", "--review-blocking", "--no-auto-rollback"]`

- [ ] **Step 1: Write failing test in `tests/test_superstate_daemon_adapter.py`**

```python
def test_daemon_spawn_cmd_review_blocking_flags():
    adapter = SuperstateDaemonAdapter(
        advisory_review=True,
        review_blocking=True,
        auto_rollback=False,
        name="superstate_daemon_review",
    )
    cmd = adapter._build_daemon_spawn_cmd(Path("/fake/workspace"))
    assert "--advisory-review" in cmd
    assert "--review-blocking" in cmd
    assert "--no-auto-rollback" in cmd
```

- [ ] **Step 2: Run test to verify RED**

Run: `.venv/bin/python -m pytest tests/test_superstate_daemon_adapter.py -k test_daemon_spawn_cmd_review_blocking_flags`
Expected: Verify behavior or assertions.

- [ ] **Step 3: Verify and update `lab/engine/runner.py` for `superstate_daemon_review`**

Ensure `superstate_daemon_review` resolves to `SuperstateDaemonAdapter(advisory_review=True, review_blocking=True, auto_rollback=False, name="superstate_daemon_review")`.

- [ ] **Step 4: Run full test suite and ruff checks in `superstate-eval`**

Run: `.venv/bin/python -m pytest tests/test_superstate_daemon_adapter.py tests/test_arm_input_parity.py -q`
Run: `.venv/bin/ruff check lab tests`
Expected: 100% PASS, 0 ruff errors

- [ ] **Step 5: Commit**

```bash
git add lab/engine/adapters/superstate_daemon.py lab/engine/runner.py tests/test_superstate_daemon_adapter.py tests/test_arm_input_parity.py
git commit --no-gpg-sign -m "feat(eval): wire superstate_daemon_review with full review blocking and edit preservation"
```
