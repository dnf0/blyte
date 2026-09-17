# Advisory Code Review Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement offline adversarial code review in `superstate`, providing both a standalone CLI (`superstate review`) and daemon-integrated advisory feedback (`superstate daemon --advisory-review`) that logs findings into the state ledger and prevents false passes on vacuous gates.

**Architecture:** A core runner in `crates/superstate-review` invokes local CLI drivers (`agy` or `claude`), extracts structured `ReviewReport` findings, and maps critical/high severities. `crates/superstate-cli` provides formatted text/JSON output and ledger recording. `crates/superstate-core::daemon` wires advisory review into milestone gate evaluations.

**Tech Stack:** Rust (2021 edition), `serde`, `serde_json`, `thiserror`, `clap`.

## Global Constraints
- Must compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No new external crate dependencies; reuse existing workspace crates (`superstate-review`, `superstate-core`, `superstate-cli`).
- Zero network I/O or direct API keys in the Rust binary; use local CLI driver execution (`agy` / `claude`).
- Handle malformed or markdown-fenced LLM responses without panics.

---

### Task 1: Core Review Engine & Runner in `crates/superstate-review`

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-review/src/schema.rs`
- Modify: `crates/superstate-review/src/lib.rs`
- Create: `crates/superstate-review/src/runner.rs`
- Create: `crates/superstate-review/tests/test_runner.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct ReviewConfig {
      pub driver: Option<String>,
      pub timeout_secs: u64,
  }

  #[derive(Error, Debug)]
  pub enum ReviewError {
      #[error("I/O error: {0}")]
      Io(#[from] std::io::Error),
      #[error("Diff error: {0}")]
      Diff(#[from] crate::diff::DiffError),
      #[error("Driver execution error: {0}")]
      Driver(String),
      #[error("Driver not found in PATH or environment")]
      DriverNotFound,
      #[error("JSON parse error: {0}")]
      Json(String),
  }

  pub fn resolve_review_driver(configured_driver: Option<&str>) -> Result<String, ReviewError>;
  pub fn clean_json_response(raw: &str) -> String;
  pub fn parse_review_report(raw: &str) -> Result<ReviewReport, ReviewError>;
  pub fn execute_review(
      repo_root: &Path,
      diff: &str,
      task_instruction: Option<&str>,
      config: &ReviewConfig,
  ) -> Result<ReviewReport, ReviewError>;
  ```

- [ ] **Step 1: Write the failing tests for review runner, driver resolution, and report parsing**

```rust
// crates/superstate-review/tests/test_runner.rs
use std::path::PathBuf;
use superstate_review::runner::{clean_json_response, parse_review_report, resolve_review_driver};
use superstate_review::schema::Severity;

#[test]
fn test_clean_json_response() {
    assert_eq!(clean_json_response("{\"summary\": \"ok\"}"), "{\"summary\": \"ok\"}");
    assert_eq!(clean_json_response("```json\n{\"summary\": \"ok\"}\n```"), "{\"summary\": \"ok\"}");
    assert_eq!(clean_json_response("```\n{\"summary\": \"ok\"}\n```"), "{\"summary\": \"ok\"}");
}

#[test]
fn test_parse_review_report_dict() {
    let raw = r#"{
        "summary": "Potential memory leak",
        "findings": [
            {
                "severity": "High",
                "file": "src/main.rs",
                "line_range": [10, 20],
                "problem": "Unbounded buffer",
                "suggested_fix": "Add capacity limit"
            }
        ],
        "has_blocking_issues": false
    }"#;
    let report = parse_review_report(raw).expect("Failed to parse report");
    assert_eq!(report.summary, "Potential memory leak");
    assert_eq!(report.findings.len(), 1);
    assert!(report.has_blocking_issues, "High severity should promote has_blocking_issues");
}

#[test]
fn test_parse_review_report_list() {
    let raw = r#"[
        {
            "severity": "Critical",
            "file": "src/lib.rs",
            "line_range": [5, 6],
            "problem": "Buffer overflow",
            "suggested_fix": "Bounds check"
        }
    ]"#;
    let report = parse_review_report(raw).expect("Failed to parse report list");
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].severity, Severity::Critical);
    assert!(report.has_blocking_issues);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-review --test test_runner`
Expected: FAIL (modules or functions do not exist yet)

- [ ] **Step 3: Implement `schema.rs` display and `runner.rs`**

Add `Display` for `ReviewReport` in `crates/superstate-review/src/schema.rs`.
Create `crates/superstate-review/src/runner.rs` with `ReviewConfig`, `ReviewError`, `clean_json_response`, `parse_review_report`, `resolve_review_driver`, and `execute_review`.
Export them in `crates/superstate-review/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-review`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-review
git commit -m "feat(review): implement review runner and report parser"
```

---

### Task 2: CLI Command `superstate review` in `crates/superstate-cli`

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-cli/src/main.rs`
- Modify: `crates/superstate-cli/src/commands/review.rs`
- Create: `crates/superstate-cli/tests/test_cli_review.rs`

**Interfaces:**
- Consumes: `superstate_review::{execute_review, get_git_diff, ReviewConfig, ReviewReport}`
- Consumes: `superstate_core::ledger::append_ledger_note`
- Produces: CLI command `superstate review [commit_range] [--repo] [--driver] [--state-path] [--record] [--json] [--fail-on-blocking]`

- [ ] **Step 1: Write the failing tests for CLI review**

```rust
// crates/superstate-cli/tests/test_cli_review.rs
use std::process::Command;
use tempfile::TempDir;

#[test]
fn test_cli_review_clean_diff() {
    let temp_dir = TempDir::new().unwrap();
    // Initialize dummy git repo
    Command::new("git").args(["init"]).current_dir(temp_dir.path()).output().unwrap();

    let binary = env!("CARGO_BIN_EXE_superstate");
    let output = Command::new(binary)
        .args(["review", "--repo", temp_dir.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No changes detected"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-cli --test test_cli_review`
Expected: FAIL or passes existing stub behavior

- [ ] **Step 3: Implement CLI flags and execution in `review.rs`**

Update `Commands::Review` in `crates/superstate-cli/src/main.rs` to accept:
- `commit_range: Option<String>`
- `repo: PathBuf`
- `driver: Option<String>`
- `state_path: Option<PathBuf>`
- `record: bool`
- `json: bool`
- `fail_on_blocking: bool`

In `crates/superstate-cli/src/commands/review.rs`:
- Compute git diff (if range given, diff range; if omitted, `git diff HEAD` or `git diff`).
- If diff is empty, emit clean report.
- Otherwise call `superstate_review::runner::execute_review`.
- If `--json`, serialize report to JSON.
- If `--record`, append `[review-advisory]` entries to `.agent-state/state.json`.
- If `--fail-on-blocking` and `report.has_blocking_issues`, exit with code 1.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p superstate-cli --test test_cli_review`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-cli
git commit -m "feat(cli): wire superstate review with driver execution, --json and --record"
```

---

### Task 3: Daemon Advisory Review Loop in `crates/superstate-core`

- **Depends on:** Task 1, Task 2

**Files:**
- Modify: `crates/superstate-core/src/daemon.rs`
- Modify: `crates/superstate-cli/src/state_cli.rs`
- Create: `crates/superstate-core/tests/test_daemon_advisory_review.rs`

**Interfaces:**
- Consumes: `superstate_review::runner::{execute_review, ReviewConfig}`
- Produces: `DaemonConfig.advisory_review: bool`, `--advisory-review` flag in `superstate daemon`.

- [ ] **Step 1: Write integration tests for daemon advisory review**

```rust
// crates/superstate-core/tests/test_daemon_advisory_review.rs
use std::fs;
use tempfile::TempDir;
use superstate_core::daemon::{DaemonConfig, DaemonRunner};
use superstate_core::models::{ExecutionState, ExecutionStatus, MilestoneSpec, TaskSpec};
use superstate_core::state_machine::save_state_atomic;

#[test]
fn test_daemon_advisory_review_records_note() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let state = ExecutionState {
        status: ExecutionStatus::InProgress,
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Test Task".to_string(),
            verification_cmd: Some("true".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    save_state_atomic(&state_file, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_file.clone(),
        max_retries: 3,
        timeout_secs: 5,
        driver_cmd: None,
        advisory_review: true,
        ..Default::default()
    };

    let mut runner = DaemonRunner::new(config);
    let outcome = runner.step().unwrap();
    // Verifies step completes or records notes without crashing
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-core --test test_daemon_advisory_review`
Expected: FAIL (advisory_review field not on DaemonConfig)

- [ ] **Step 3: Wire advisory review in `daemon.rs` and CLI args**

In `crates/superstate-core/src/daemon.rs`:
- Add `advisory_review: bool` to `DaemonConfig`.
- In `DaemonRunner::step`:
  After `gate_res = run_verification_cmd(...)`:
  If `gate_res.passed` is true and `self.config.advisory_review`:
  - Extract git diff for active milestone.
  - Call `execute_review` using `self.config.driver_cmd` or default.
  - If `report.has_blocking_issues`:
    - Append `[review-advisory]` note to `state.notes`.
    - If `is_vacuous_gate(verify_cmd)` and `self.config.driver_cmd.is_some()`:
      - Hold milestone open (return `DaemonStepOutcome::Failed` for retry).
In `crates/superstate-cli/src/state_cli.rs`:
- Add `--advisory-review` flag to `DaemonArgs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --all`
Expected: All tests pass across workspace.

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-core crates/superstate-cli
git commit -m "feat(daemon): wire advisory review into milestone gate evaluation"
```
