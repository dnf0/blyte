# Robust Diff Extraction and Blocking Code Review Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure the code review engine captures 100% of committed and uncommitted milestone changes, and empower the daemon with a `--review-blocking` quality gate that fails incomplete implementations and triggers retry turns with exact structured findings.

**Architecture:**
1. In `crates/superstate-review`: Add `get_milestone_diff` which inspects both committed git changes since a starting commit (`start_commit..HEAD`) and uncommitted/untracked files.
2. In `crates/superstate-core`: Track `milestone_start_commits` in `DaemonRunner`, pass the milestone diff to review, and support `--review-blocking` which yields `DaemonStepOutcome::Failed` when Critical/High findings exist.
3. In `crates/superstate-cli`: Wire the `--review-blocking` flag into the daemon CLI and use `get_milestone_diff` in `superstate review`.
4. In `superstate-eval`: Pass `--review-blocking` in `superstate_daemon_review` adapter and update regression tests.

**Tech Stack:** Rust 2021, Cargo, Git CLI (`std::process::Command`), Python 3.12, Pytest.

## Global Constraints
- Must compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No new external crate dependencies; reuse existing workspace crates (`superstate-review`, `superstate-core`, `superstate-cli`).
- Zero network I/O or direct API keys in Rust; local CLI driver execution (`agy` / `claude`) via subprocess.
- Zero prompt contracts or prompt manipulation (`SUPERSTATE_EVAL_PROMPT_CONTRACTS=0` strictly maintained).
- Skip GPG signing on all commits (`--no-gpg-sign`).

---

### Task 1: Milestone Diff Extraction in `crates/superstate-review`

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-review/src/diff.rs`
- Modify: `crates/superstate-review/src/lib.rs`
- Test: `crates/superstate-review/tests/test_milestone_diff.rs`

**Interfaces:**
- Produces: `pub fn get_milestone_diff(repo_root: &Path, start_commit: Option<&str>) -> Result<String, DiffError>`

- [ ] **Step 1: Write failing tests in `crates/superstate-review/tests/test_milestone_diff.rs`**

```rust
use std::fs;
use std::process::Command;
use tempfile::TempDir;
use superstate_review::diff::get_milestone_diff;

fn setup_repo() -> (TempDir, String) {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    Command::new("git").args(["init"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.name", "Tester"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.email", "tester@example.com"]).current_dir(root).output().unwrap();
    
    fs::write(root.join("base.txt"), "base content\n").unwrap();
    Command::new("git").args(["add", "base.txt"]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial commit", "--no-gpg-sign"]).current_dir(root).output().unwrap();
    
    let head = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(root).output().unwrap();
    let initial_hash = String::from_utf8(head.stdout).unwrap().trim().to_string();
    (temp, initial_hash)
}

#[test]
fn test_get_milestone_diff_committed_only() {
    let (temp, initial_hash) = setup_repo();
    let root = temp.path();

    fs::write(root.join("base.txt"), "base content\ncommitted change\n").unwrap();
    Command::new("git").args(["add", "base.txt"]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "update base", "--no-gpg-sign"]).current_dir(root).output().unwrap();

    let diff = get_milestone_diff(root, Some(&initial_hash)).unwrap();
    assert!(diff.contains("+committed change"), "Diff should contain committed changes");
}

#[test]
fn test_get_milestone_diff_uncommitted_only() {
    let (temp, initial_hash) = setup_repo();
    let root = temp.path();

    fs::write(root.join("base.txt"), "base content\nuncommitted change\n").unwrap();

    let diff = get_milestone_diff(root, Some(&initial_hash)).unwrap();
    assert!(diff.contains("+uncommitted change"), "Diff should contain uncommitted changes");
}

#[test]
fn test_get_milestone_diff_committed_and_uncommitted() {
    let (temp, initial_hash) = setup_repo();
    let root = temp.path();

    fs::write(root.join("base.txt"), "base content\ncommitted change\n").unwrap();
    Command::new("git").args(["add", "base.txt"]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "update base", "--no-gpg-sign"]).current_dir(root).output().unwrap();

    fs::write(root.join("uncommitted.txt"), "uncommitted file\n").unwrap();

    let diff = get_milestone_diff(root, Some(&initial_hash)).unwrap();
    assert!(diff.contains("+committed change"), "Diff should contain committed changes");
    assert!(diff.contains("+uncommitted file"), "Diff should contain untracked uncommitted file");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-review --test test_milestone_diff`
Expected: FAIL (`get_milestone_diff` does not exist)

- [ ] **Step 3: Implement `get_milestone_diff` in `crates/superstate-review/src/diff.rs`**

```rust
pub fn get_milestone_diff(repo_root: &Path, start_commit: Option<&str>) -> Result<String, DiffError> {
    let head_hash = match Command::new("git").args(["rev-parse", "HEAD"]).current_dir(repo_root).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => return get_git_diff(repo_root, None),
    };

    let mut combined_diff = String::new();

    if let Some(start) = start_commit {
        let start = start.trim();
        if !start.is_empty() && start != head_hash {
            let range = format!("{}..HEAD", start);
            if let Ok(committed_diff) = get_git_diff(repo_root, Some(&range)) {
                if !committed_diff.trim().is_empty() {
                    combined_diff.push_str(&committed_diff);
                }
            }
        }
    }

    if let Ok(working_diff) = get_git_diff(repo_root, Some("HEAD")) {
        if !working_diff.trim().is_empty() {
            if !combined_diff.is_empty() && !combined_diff.ends_with('\n') {
                combined_diff.push('\n');
            }
            combined_diff.push_str(&working_diff);
        }
    }

    if combined_diff.trim().is_empty() && start_commit.is_none() {
        if let Ok(out) = Command::new("git").args(["rev-parse", "--verify", "HEAD~1"]).current_dir(repo_root).output() {
            if out.status.success() {
                if let Ok(last_commit_diff) = get_git_diff(repo_root, Some("HEAD~1..HEAD")) {
                    return Ok(last_commit_diff);
                }
            }
        }
    }

    Ok(combined_diff)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-review --test test_milestone_diff`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-review/src/diff.rs crates/superstate-review/src/lib.rs crates/superstate-review/tests/test_milestone_diff.rs
git commit --no-gpg-sign -m "feat(review): implement get_milestone_diff with committed and uncommitted change tracking"
```

---

### Task 2: Milestone Base Commit Tracking & `--review-blocking` in `crates/superstate-core`

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-core/src/daemon.rs`
- Test: `crates/superstate-core/tests/test_daemon_blocking_review.rs`

**Interfaces:**
- `DaemonConfig::review_blocking: bool`
- Tracks `milestone_start_commits: HashMap<String, String>` in `DaemonRunner`

- [ ] **Step 1: Write failing tests in `crates/superstate-core/tests/test_daemon_blocking_review.rs`**

```rust
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;
use superstate_core::daemon::{DaemonConfig, DaemonRunner, DaemonStepOutcome};
use superstate_core::models::{ExecutionState, ExecutionStatus, TaskSpec};
use superstate_core::state_machine::save_state_atomic;

#[test]
fn test_daemon_review_blocking_fails_on_blocking_issues() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path();

    Command::new("git").args(["init"]).current_dir(repo_root).output().unwrap();
    Command::new("git").args(["config", "user.name", "Tester"]).current_dir(repo_root).output().unwrap();
    Command::new("git").args(["config", "user.email", "tester@example.com"]).current_dir(repo_root).output().unwrap();
    fs::write(repo_root.join("code.txt"), "original\n").unwrap();
    Command::new("git").args(["add", "code.txt"]).current_dir(repo_root).output().unwrap();
    Command::new("git").args(["commit", "-m", "init", "--no-gpg-sign"]).current_dir(repo_root).output().unwrap();

    let script = temp.path().join("mock_review_driver.sh");
    fs::write(&script, "#!/bin/sh\ncat <<'EOF'\n{\n  \"has_blocking_issues\": true,\n  \"summary\": \"Critical bugs found\",\n  \"findings\": [\n    {\"file\": \"code.txt\", \"line\": 1, \"severity\": \"High\", \"problem\": \"Broken logic\", \"suggested_fix\": \"Fix it\"}\n  ]\n}\nEOF\n").unwrap();
    Command::new("chmod").args(["+x", script.to_str().unwrap()]).output().unwrap();

    let state_file = repo_root.join(".agent-state").join("state.json");
    fs::create_dir_all(state_file.parent().unwrap()).unwrap();

    let mut state = ExecutionState::default();
    state.status = ExecutionStatus::InProgress;
    state.active_task = Some(TaskSpec {
        id: "task-1".to_string(),
        title: "Implement feature".to_string(),
        verification_cmd: Some("exit 0".to_string()),
        ..Default::default()
    });
    save_state_atomic(&state_file, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_file,
        interval_secs: 1,
        timeout_secs: 30,
        max_retries: 2,
        driver_cmd: Some(script.to_str().unwrap().to_string()),
        auto_commit: false,
        advisory_review: true,
        review_blocking: true,
    };

    let mut runner = DaemonRunner::new(config);
    let outcome = runner.step().unwrap();

    match outcome {
        DaemonStepOutcome::Failed { task_id, reason, .. } => {
            assert_eq!(task_id, "task-1");
            assert!(reason.contains("code-review"), "Reason should cite code-review: {}", reason);
        }
        _ => panic!("Expected DaemonStepOutcome::Failed with review_blocking=true, got {:?}", outcome),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-core --test test_daemon_blocking_review`
Expected: FAIL (`review_blocking` does not exist on `DaemonConfig`)

- [ ] **Step 3: Update `crates/superstate-core/src/daemon.rs`**
- Add `pub review_blocking: bool` to `DaemonConfig`.
- Add `milestone_start_commits: HashMap<String, String>` to `DaemonRunner`.
- In `DaemonRunner::step`:
  - When milestone begins, record `start_commit`.
  - When `gate_res.passed` is true and `(self.config.advisory_review || self.config.review_blocking)` is true:
    - Call `superstate_review::diff::get_milestone_diff(&working_dir, start_commit)`.
    - If `report.has_blocking_issues && (self.config.review_blocking || is_vacuous_gate(verify_cmd))`:
      - Return `DaemonStepOutcome::Failed`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-core --test test_daemon_blocking_review`
Expected: PASS

- [ ] **Step 5: Run all core tests**

Run: `cargo test -p superstate-core`
Expected: PASS (all tests green)

- [ ] **Step 6: Commit**

```bash
git add crates/superstate-core/src/daemon.rs crates/superstate-core/tests/test_daemon_blocking_review.rs crates/superstate-core/tests/test_daemon_runner.rs
git commit --no-gpg-sign -m "feat(daemon): track milestone start commit and support review_blocking"
```

---

### Task 3: CLI Flag `--review-blocking` in `crates/superstate-cli`

- **Depends on:** Task 2

**Files:**
- Modify: `crates/superstate-cli/src/main.rs`
- Modify: `crates/superstate-cli/src/commands/daemon.rs`
- Modify: `crates/superstate-cli/src/commands/review.rs`
- Test: `crates/superstate-cli/tests/test_cli_blocking_review.rs`

- [ ] **Step 1: Write failing test in `crates/superstate-cli/tests/test_cli_blocking_review.rs`**

Verify CLI parser parses `--review-blocking` on `superstate state daemon`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-cli --test test_cli_blocking_review`
Expected: FAIL

- [ ] **Step 3: Wire `--review-blocking` flag in `main.rs`, `daemon.rs`, and update `review.rs`**
- Wire CLI argument `--review-blocking`.
- Update `cmd_review` in `commands/review.rs` to call `get_milestone_diff`.

- [ ] **Step 4: Run CLI tests**

Run: `cargo test -p superstate-cli`
Expected: PASS

- [ ] **Step 5: Commit and Install**

```bash
git add crates/superstate-cli/src/main.rs crates/superstate-cli/src/commands/daemon.rs crates/superstate-cli/src/commands/review.rs crates/superstate-cli/tests/test_cli_blocking_review.rs
git commit --no-gpg-sign -m "feat(cli): wire --review-blocking CLI flag and milestone diff in review"
cargo install --path crates/superstate-cli --locked --force
```

---

### Task 4: Eval Adapter Wiring & Regression Verification in `superstate-eval`

- **Depends on:** Task 3

**Files:**
- Modify: `lab/engine/adapters/superstate_daemon.py`
- Modify: `tests/test_arm_input_parity.py`

- [ ] **Step 1: Wire `--review-blocking` into `superstate_daemon.py`**
- In `SuperstateDaemonAdapter._run_daemon`:
  - When `self.advisory_review` is enabled, also pass `--review-blocking`.

- [ ] **Step 2: Run regression parity test**

Run: `.venv/bin/python -m pytest tests/test_arm_input_parity.py -q`
Expected: PASS

- [ ] **Step 3: Run full pytest suite in `superstate-eval`**

Run: `.venv/bin/python -m pytest tests/ -q`
Expected: PASS (all tests pass)

- [ ] **Step 4: Commit**

```bash
git add lab/engine/adapters/superstate_daemon.py tests/test_arm_input_parity.py
git commit --no-gpg-sign -m "feat(eval): wire --review-blocking into superstate_daemon_review adapter"
```
