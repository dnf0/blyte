# Milestone-Scoped Test Impact Slicing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement milestone-scoped test impact slicing in `superstate state daemon`, enabling fast fail-fast pre-filtering on impacted unit tests during intermediate iterations while preserving full regression verification before DAG milestone advancement.

**Architecture:** Extend `crates/superstate-core/src/impact.rs` with runner command specialization (`build_sliced_test_cmd`). Wire a two-tier verification check into `DaemonRunner::step()` diffing against `milestone_start_commit`. If the sliced tests fail, fail fast and rollback immediately; if they pass, run the full milestone `verify_cmd` before committing.

**Tech Stack:** Rust, Git, Tree-sitter, Pytest, Cargo, Go, Jest/Vitest.

## Global Constraints
- Compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No compiler or clippy warnings (`cargo clippy --all-targets -- -D warnings`).
- Skip GPG verification on all commits (`--no-gpg-sign`).
- Zero prompt engineering: all behavior governed by git diffs, process exit codes (`$? == 0`), and state invariants.

---

### Task 1: Command Specialization in `impact.rs`

- **Depends on:** none

**Files:**
- Create: `crates/superstate-core/tests/test_impact_slicing.rs`
- Modify: `crates/superstate-core/src/impact.rs`

**Interfaces:**
- Produces: `pub fn build_sliced_test_cmd(base_cmd: &str, impacted_files: &[PathBuf], repo_root: &Path) -> Option<String>`

- [ ] **Step 1: Write failing unit tests in `test_impact_slicing.rs`**

```rust
use std::path::{Path, PathBuf};
use superstate_core::impact::build_sliced_test_cmd;

#[test]
fn test_build_sliced_test_cmd_pytest() {
    let repo = Path::new("/workspace");
    let files = vec![
        PathBuf::from("/workspace/tests/test_auth.py"),
        PathBuf::from("/workspace/tests/test_token.py"),
    ];
    let res = build_sliced_test_cmd("pytest tests/", &files, repo);
    assert_eq!(
        res,
        Some("pytest tests/test_auth.py tests/test_token.py".to_string())
    );
}

#[test]
fn test_build_sliced_test_cmd_pytest_container_prefix() {
    let repo = Path::new("/workspace");
    let files = vec![PathBuf::from("/workspace/tests/test_auth.py")];
    let res = build_sliced_test_cmd("./run-in-container pytest tests/", &files, repo);
    assert_eq!(
        res,
        Some("./run-in-container pytest tests/test_auth.py".to_string())
    );
}

#[test]
fn test_build_sliced_test_cmd_cargo() {
    let repo = Path::new("/workspace");
    let files = vec![PathBuf::from("/workspace/tests/test_lexer.rs")];
    let res = build_sliced_test_cmd("cargo test", &files, repo);
    assert_eq!(
        res,
        Some("cargo test --test test_lexer".to_string())
    );
}

#[test]
fn test_build_sliced_test_cmd_unrecognized_returns_none() {
    let repo = Path::new("/workspace");
    let files = vec![PathBuf::from("/workspace/tests/test_auth.py")];
    let res = build_sliced_test_cmd("sh verify.sh", &files, repo);
    assert_eq!(res, None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-core --test test_impact_slicing`  
Expected: FAIL (`build_sliced_test_cmd` does not exist)

- [ ] **Step 3: Implement `build_sliced_test_cmd` in `crates/superstate-core/src/impact.rs`**

```rust
/// Specialize a base verification command to execute only the impacted test files.
///
/// Inspects the base command to recognize common test runners:
/// - Pytest: `pytest`, `python -m pytest`, or prefixed commands like `./run-in-container pytest`
/// - Cargo: `cargo test`
/// - Go: `go test`
/// - Node/Vitest/Jest: `npm test`, `npx vitest`, `yarn test`
///
/// Returns `Some(specialized_cmd)` if the runner is recognized and files are formatted.
/// Returns `None` if the runner is unrecognized or generic.
pub fn build_sliced_test_cmd(
    base_cmd: &str,
    impacted_files: &[PathBuf],
    repo_root: &Path,
) -> Option<String> {
    if impacted_files.is_empty() {
        return None;
    }

    let trimmed = base_cmd.trim();

    // 1. Pytest detection (supports ./run-in-container pytest, python -m pytest, pytest)
    if let Some(prefix_idx) = trimmed.find("pytest") {
        let prefix = trimmed[..prefix_idx + "pytest".len()].trim();
        let rel_paths: Vec<String> = impacted_files
            .iter()
            .map(|p| {
                p.strip_prefix(repo_root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        return Some(format!("{} {}", prefix, rel_paths.join(" ")));
    }

    // 2. Cargo test detection
    if trimmed.starts_with("cargo test") {
        let mut test_args = Vec::new();
        for file in impacted_files {
            let rel = file.strip_prefix(repo_root).unwrap_or(file);
            if let Some(parent) = rel.parent() {
                if parent == Path::new("tests") {
                    if let Some(stem) = rel.file_stem().and_then(|s| s.to_str()) {
                        test_args.push(format!("--test {}", stem));
                        continue;
                    }
                }
            }
            if let Some(stem) = rel.file_stem().and_then(|s| s.to_str()) {
                test_args.push(stem.to_string());
            }
        }
        test_args.sort();
        test_args.dedup();
        if !test_args.is_empty() {
            return Some(format!("cargo test {}", test_args.join(" ")));
        }
    }

    // 3. Node / Vitest / Jest detection
    if trimmed.contains("vitest") || trimmed.contains("jest") || trimmed.starts_with("npm test") {
        let prefix = if trimmed.starts_with("npm test") {
            "npm test --"
        } else {
            trimmed.split_whitespace().next().unwrap_or("npx vitest")
        };
        let rel_paths: Vec<String> = impacted_files
            .iter()
            .map(|p| {
                p.strip_prefix(repo_root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        return Some(format!("{} {}", prefix, rel_paths.join(" ")));
    }

    None
}
```

- [ ] **Step 4: Run unit tests to verify they pass**

Run: `cargo test -p superstate-core --test test_impact_slicing`  
Expected: PASS

- [ ] **Step 5: Commit Task 1**

```bash
git add crates/superstate-core/src/impact.rs crates/superstate-core/tests/test_impact_slicing.rs
git commit --no-gpg-sign -m "feat(core): implement test runner command specialization for impact slicing"
```

---

### Task 2: Configuration & CLI Flag (`--impact-slicing`)

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-core/src/daemon.rs`
- Modify: `crates/superstate-cli/src/state_cli.rs`
- Modify: `crates/superstate-cli/src/commands/daemon.rs`
- Create: `crates/superstate-cli/tests/test_cli_impact_slicing.rs`

**Interfaces:**
- Consumes: `DaemonConfig`
- Produces: `DaemonConfig.impact_slicing: bool`, CLI `--impact-slicing` flag

- [ ] **Step 1: Write CLI parsing test in `test_cli_impact_slicing.rs`**

```rust
use clap::Parser;
use superstate_cli::state_cli::StateArgs;

#[test]
fn test_cli_impact_slicing_flag_parsing() {
    let args = StateArgs::try_parse_from(["state", "daemon", "--impact-slicing"]);
    assert!(args.is_ok(), "Expected --impact-slicing to parse successfully");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-cli --test test_cli_impact_slicing`  
Expected: FAIL (unrecognized argument `--impact-slicing`)

- [ ] **Step 3: Add `impact_slicing` to `DaemonConfig` and CLI args**

In `crates/superstate-core/src/daemon.rs`:
```rust
pub struct DaemonConfig {
    ...
    /// Enable milestone-scoped test impact slicing for fast fail-fast feedback.
    pub impact_slicing: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            ...
            impact_slicing: false,
        }
    }
}
```

In `crates/superstate-cli/src/state_cli.rs`:
```rust
pub struct DaemonArgs {
    ...
    /// Enable milestone-scoped test impact slicing for fast fail-fast feedback
    #[arg(long, help = "Enable milestone-scoped test impact slicing for fast fail-fast feedback")]
    pub impact_slicing: bool,
}
```

In `crates/superstate-cli/src/commands/daemon.rs`:
Wire `impact_slicing: args.impact_slicing` when constructing `DaemonConfig`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p superstate-cli --test test_cli_impact_slicing`  
Expected: PASS

- [ ] **Step 5: Commit Task 2**

```bash
git add crates/superstate-core/src/daemon.rs crates/superstate-cli/src/state_cli.rs crates/superstate-cli/src/commands/daemon.rs crates/superstate-cli/tests/test_cli_impact_slicing.rs
git commit --no-gpg-sign -m "feat(cli): wire --impact-slicing flag into daemon configuration"
```

---

### Task 3: Two-Tier Gate Execution in `DaemonRunner::step()`

- **Depends on:** Task 1, Task 2

**Files:**
- Modify: `crates/superstate-core/src/daemon.rs:750-800`
- Create: `crates/superstate-core/tests/test_daemon_impact_slicing.rs`

**Interfaces:**
- Consumes: `build_sliced_test_cmd`, `infer_impacted_tests`, `DaemonConfig.impact_slicing`
- Produces: Fast-fail pre-filter before `run_verification_cmd(verify_cmd, ...)`

- [ ] **Step 1: Write integration tests in `test_daemon_impact_slicing.rs`**

```rust
use std::fs;
use tempfile::tempdir;
use superstate_core::daemon::{DaemonConfig, DaemonRunner, DaemonStepOutcome};
use superstate_core::state::{ExecutionState, TaskSpec};

#[test]
fn test_daemon_impact_slicing_fast_fail_skips_full_gate() {
    let dir = tempdir().unwrap();
    let repo = dir.path();
    // Initialize git repo and state with impact_slicing = true
    // Create source file and failing test file
    // Set verify_cmd to "touch full_gate_ran.txt && exit 0"
    // Run daemon step
    // Assert fast slice failed and "full_gate_ran.txt" does NOT exist
}

#[test]
fn test_daemon_impact_slicing_passing_slice_executes_full_gate() {
    let dir = tempdir().unwrap();
    let repo = dir.path();
    // Initialize git repo and state with impact_slicing = true
    // Create source file and passing test file
    // Set verify_cmd to "touch full_gate_ran.txt && exit 0"
    // Run daemon step
    // Assert fast slice passed, full gate executed, milestone completed
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-core --test test_daemon_impact_slicing`  
Expected: FAIL

- [ ] **Step 3: Implement staged verification in `crates/superstate-core/src/daemon.rs`**

Right before `run_verification_cmd(verify_cmd, &working_dir, ...)`:
```rust
        // Stage 1: Milestone-Scoped Test Impact Slicing (Fast-Fail Pre-Filter)
        if self.config.impact_slicing && is_inside_git_repo(&working_dir) {
            let start_commit = self.milestone_start_commits.get(&active_task.id);
            let impacted = infer_impacted_tests(&working_dir, start_commit.map(|s| s.as_str()));
            if !impacted.is_empty() {
                if let Some(sliced_cmd) = build_sliced_test_cmd(verify_cmd, &impacted, &working_dir) {
                    let slice_start = Instant::now();
                    let slice_res = run_verification_cmd(&sliced_cmd, &working_dir, self.config.timeout_secs)?;
                    let slice_elapsed = slice_start.elapsed().as_secs();

                    if !slice_res.passed {
                        let failure_output = if !slice_res.stderr.trim().is_empty() {
                            slice_res.stderr.trim()
                        } else {
                            slice_res.stdout.trim()
                        };
                        let bounded_diag = failure_output.chars().take(1000).collect::<String>();
                        state.notes.push(LedgerEntry::new(
                            "superstate-daemon",
                            format!(
                                "[impact-fail] Task {}: Fast test slice failed in {}s: {}",
                                active_task.id, slice_elapsed, bounded_diag
                            ),
                        ));
                        save_state_atomic(&self.config.state_path, &state)?;

                        if self.config.auto_rollback {
                            if let Err(e) = self.perform_rollback(&working_dir, &active_task.id) {
                                state.notes.push(LedgerEntry::new(
                                    "superstate-daemon",
                                    format!("[rollback-error] Failed to roll back after impact failure: {}", e),
                                ));
                                save_state_atomic(&self.config.state_path, &state)?;
                            }
                        }

                        return Ok(DaemonStepOutcome::Failed {
                            task_id: active_task.id.clone(),
                            error: format!("Fast test slice failed: {}", bounded_diag),
                        });
                    } else {
                        state.notes.push(LedgerEntry::new(
                            "superstate-daemon",
                            format!(
                                "[impact-pass] Task {}: Fast slice passed ({} test files) in {}s, advancing to full gate",
                                active_task.id, impacted.len(), slice_elapsed
                            ),
                        ));
                        save_state_atomic(&self.config.state_path, &state)?;
                    }
                }
            }
        }
```

- [ ] **Step 4: Run integration tests to verify they pass**

Run: `cargo test -p superstate-core --test test_daemon_impact_slicing`  
Expected: PASS

- [ ] **Step 5: Commit Task 3**

```bash
git add crates/superstate-core/src/daemon.rs crates/superstate-core/tests/test_daemon_impact_slicing.rs
git commit --no-gpg-sign -m "feat(daemon): wire milestone test impact slicing into daemon verification loop"
```

---

### Task 4: Full Workspace Verification & Static Analysis

- **Depends on:** Task 1, Task 2, Task 3

**Files:**
- None (verification only)

- [ ] **Step 1: Run full test suite across all workspace crates**

Run: `cargo test --all`  
Expected: PASS (all tests pass across all crates)

- [ ] **Step 2: Run clippy with strict warnings**

Run: `cargo clippy --all-targets -- -D warnings`  
Expected: PASS (0 warnings)

- [ ] **Step 3: Run formatting check**

Run: `cargo fmt --all -- --check`  
Expected: PASS
