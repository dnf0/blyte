# Tool Output Shunt Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a native tool output shunting layer ("Spotify Portal" pattern) in `superstate` that intercepts command execution, writes full raw logs to disk, and returns a compact high-signal distillation for outputs exceeding 50 lines to prevent context saturation and slash LLM token usage by ~80-90%.

**Architecture:**
- `superstate-core::shunt`: Pure Rust distillation engine that evaluates process output size, writes unstripped raw logs to `.agent-state/logs/cmd_<timestamp>_<hash>.log`, and formats either a 1-line success notice or an extracted failure excerpt.
- `superstate-cli::commands::shunt`: CLI subcommand `superstate shunt [--threshold-lines <N>] -- <command> [args...]` that executes a child process, drains stdout/stderr concurrently to prevent pipe buffer deadlocks, applies distillation, and forwards the exact child exit status code.
- `superstate-core::daemon`: Automatic driver environment integration providing a shell shim so commands dispatched by autonomous driver workers (`agy`, `claude`) transparently execute through `superstate shunt`.

**Tech Stack:** Rust 2021 edition, `std::process::{Command, Stdio}`, `std::thread`, `clap` v4.

## Global Constraints

- Must compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No new external crate dependencies; reuse standard library (`std::process`, `std::thread`, `std::sync`, `std::fs`) and existing workspace crates.
- Exact integer exit code propagation: `superstate shunt` must mirror the child process's exit code verbatim (e.g. exit 127 stays 127, exit 137 stays 137).
- Deadlock-free stream draining: must drain stdout and stderr concurrently to handle outputs exceeding the OS pipe buffer limit (>64 KB).
- Output threshold: exactly 50 lines or 4,096 bytes (whichever is reached first). Outputs under this threshold must pass through unaltered with zero latency or formatting overhead.
- All commits must skip GPG signing (`--no-gpg-sign`).

---

### Task 1: Core Distillation & Log Storage Engine in `crates/superstate-core`

- **Depends on:** none

**Files:**
- Create: `crates/superstate-core/src/shunt.rs`
- Modify: `crates/superstate-core/src/lib.rs:1-36`
- Test: `crates/superstate-core/tests/test_shunt.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct ShuntConfig {
      pub threshold_lines: usize,
      pub threshold_bytes: usize,
      pub logs_dir: PathBuf,
  }

  pub enum DistilledOutput {
      Raw {
          stdout: Vec<u8>,
          stderr: Vec<u8>,
      },
      Distilled {
          summary: String,
          log_path: PathBuf,
          exit_code: i32,
      },
  }

  pub fn evaluate_and_distill(
      cmd_str: &str,
      working_dir: &Path,
      stdout: &[u8],
      stderr: &[u8],
      exit_code: i32,
      config: &ShuntConfig,
  ) -> Result<DistilledOutput, std::io::Error>;
  ```

- [ ] **Step 1: Write the failing integration test**

Create `crates/superstate-core/tests/test_shunt.rs`:
```rust
use std::fs;
use std::path::PathBuf;
use superstate_core::shunt::{evaluate_and_distill, DistilledOutput, ShuntConfig};
use tempfile::tempdir;

#[test]
fn test_small_output_passes_through_raw() {
    let dir = tempdir().unwrap();
    let config = ShuntConfig {
        threshold_lines: 50,
        threshold_bytes: 4096,
        logs_dir: dir.path().join(".agent-state/logs"),
    };

    let stdout = b"short output\nline 2\n";
    let stderr = b"";
    let result = evaluate_and_distill("echo test", dir.path(), stdout, stderr, 0, &config).unwrap();

    match result {
        DistilledOutput::Raw { stdout: out, stderr: err } => {
            assert_eq!(out, stdout);
            assert_eq!(err, stderr);
        }
        DistilledOutput::Distilled { .. } => panic!("Expected raw output for short command"),
    }
}

#[test]
fn test_large_successful_output_distills_to_one_line() {
    let dir = tempdir().unwrap();
    let config = ShuntConfig {
        threshold_lines: 50,
        threshold_bytes: 4096,
        logs_dir: dir.path().join(".agent-state/logs"),
    };

    let lines: Vec<String> = (1..=100).map(|i| format!("Compiling crate_{} v0.1.0", i)).collect();
    let stdout = lines.join("\n").into_bytes();
    let stderr = b"";

    let result = evaluate_and_distill("cargo build", dir.path(), &stdout, stderr, 0, &config).unwrap();

    match result {
        DistilledOutput::Distilled { summary, log_path, exit_code } => {
            assert_eq!(exit_code, 0);
            assert!(summary.contains("[superstate-shunt] Command succeeded (exit 0)"));
            assert!(summary.contains("lines omitted"));
            assert!(log_path.exists());
            let saved = fs::read_to_string(&log_path).unwrap();
            assert!(saved.contains("Compiling crate_100"));
        }
        DistilledOutput::Raw { .. } => panic!("Expected distilled output for >50 lines"),
    }
}

#[test]
fn test_large_failing_output_extracts_failure_excerpt() {
    let dir = tempdir().unwrap();
    let config = ShuntConfig {
        threshold_lines: 50,
        threshold_bytes: 4096,
        logs_dir: dir.path().join(".agent-state/logs"),
    };

    let mut lines: Vec<String> = (1..=60).map(|i| format!("test suite {} passing", i)).collect();
    lines.push("FAIL: test_json_schema_parsing_error".to_string());
    lines.push("AssertionError: expected 200 got 500".to_string());
    lines.extend((61..=80).map(|i| format!("test suite {} passing", i)));
    lines.push("Tests: 1 failed, 79 passed, 80 total".to_string());
    let stdout = lines.join("\n").into_bytes();
    let stderr = b"";

    let result = evaluate_and_distill("pytest tests", dir.path(), &stdout, stderr, 1, &config).unwrap();

    match result {
        DistilledOutput::Distilled { summary, log_path, exit_code } => {
            assert_eq!(exit_code, 1);
            assert!(summary.contains("[superstate-shunt] Command failed with exit code 1"));
            assert!(summary.contains("FAIL: test_json_schema_parsing_error"));
            assert!(summary.contains("AssertionError: expected 200 got 500"));
            assert!(summary.contains("Tests: 1 failed, 79 passed, 80 total"));
            assert!(log_path.exists());
        }
        DistilledOutput::Raw { .. } => panic!("Expected distilled output for failing test"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-core --test test_shunt`
Expected: FAIL with "unresolved import `superstate_core::shunt`"

- [ ] **Step 3: Implement minimal `superstate-core::shunt`**

Create `crates/superstate-core/src/shunt.rs`:
```rust
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ShuntConfig {
    pub threshold_lines: usize,
    pub threshold_bytes: usize,
    pub logs_dir: PathBuf,
}

impl Default for ShuntConfig {
    fn default() -> Self {
        Self {
            threshold_lines: 50,
            threshold_bytes: 4096,
            logs_dir: PathBuf::from(".agent-state/logs"),
        }
    }
}

pub enum DistilledOutput {
    Raw {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Distilled {
        summary: String,
        log_path: PathBuf,
        exit_code: i32,
    },
}

pub fn evaluate_and_distill(
    cmd_str: &str,
    working_dir: &Path,
    stdout: &[u8],
    stderr: &[u8],
    exit_code: i32,
    config: &ShuntConfig,
) -> Result<DistilledOutput, std::io::Error> {
    let combined_bytes_len = stdout.len() + stderr.len();
    let stdout_str = String::from_utf8_lossy(stdout);
    let stderr_str = String::from_utf8_lossy(stderr);

    let total_lines = stdout_str.lines().count() + stderr_str.lines().count();

    // Small output passthrough policy
    if total_lines <= config.threshold_lines && combined_bytes_len <= config.threshold_bytes {
        return Ok(DistilledOutput::Raw {
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        });
    }

    // Ensure logs directory exists and is git-ignored
    let logs_dir = if config.logs_dir.is_relative() {
        working_dir.join(&config.logs_dir)
    } else {
        config.logs_dir.clone()
    };
    fs::create_dir_all(&logs_dir)?;

    let gitignore_path = logs_dir.join(".gitignore");
    if !gitignore_path.exists() {
        let _ = fs::write(&gitignore_path, "*\n!.gitignore\n");
    }

    // Write full raw log to disk
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let hash = format!("{:x}", md5_hash(cmd_str.as_bytes()));
    let log_filename = format!("cmd_{}_{}.log", stamp, &hash[..8]);
    let log_path = logs_dir.join(log_filename);

    let mut file = fs::File::create(&log_path)?;
    writeln!(file, "=== Command: {} ===", cmd_str)?;
    writeln!(file, "=== Exit Code: {} ===", exit_code)?;
    writeln!(file, "=== Working Dir: {} ===", working_dir.display())?;
    writeln!(file, "=== Timestamp: {} ===\n", stamp)?;
    if !stdout.is_empty() {
        writeln!(file, "--- stdout ---")?;
        file.write_all(stdout)?;
        writeln!(file)?;
    }
    if !stderr.is_empty() {
        writeln!(file, "--- stderr ---")?;
        file.write_all(stderr)?;
        writeln!(file)?;
    }

    // Format distillation
    let summary = if exit_code == 0 {
        format!(
            "[superstate-shunt] Command succeeded (exit 0). {} lines omitted.\nFull log: {}",
            total_lines,
            log_path.display()
        )
    } else {
        let failure_excerpt = extract_failure_excerpt(&stdout_str, &stderr_str);
        format!(
            "[superstate-shunt] Command failed with exit code {} ({} lines omitted).\nFull log: {}\n\n--- Failure Excerpt ---\n{}\n--- End Failure Excerpt ---",
            exit_code,
            total_lines,
            log_path.display(),
            failure_excerpt
        )
    };

    Ok(DistilledOutput::Distilled {
        summary,
        log_path,
        exit_code,
    })
}

fn md5_hash(bytes: &[u8]) -> u128 {
    // Simple fast DJB2/FNV hash for deterministic unique filenames without extra crate dependencies
    let mut hash: u128 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u128;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn extract_failure_excerpt(stdout: &str, stderr: &str) -> String {
    let patterns = [
        "FAIL:", "--- FAIL:", "FAILED", "Error:", "AssertionError",
        "panic:", "undefined:", "TypeError:", "Exception:", "error[E",
    ];

    let combined = format!("{}\n{}", stdout, stderr);
    let lines: Vec<&str> = combined.lines().collect();
    let mut excerpt_lines: Vec<&str> = Vec::new();

    // Find indices matching failure keywords
    for (i, line) in lines.iter().enumerate() {
        if patterns.iter().any(|&pat| line.contains(pat)) {
            let start = i.saturating_sub(2);
            let end = (i + 4).min(lines.len());
            for idx in start..end {
                if !excerpt_lines.contains(&lines[idx]) {
                    excerpt_lines.push(lines[idx]);
                }
            }
            if excerpt_lines.len() >= 15 {
                break;
            }
        }
    }

    // If no failure pattern matched, take the last 15 lines of stderr or stdout
    if excerpt_lines.is_empty() {
        let start = lines.len().saturating_sub(15);
        excerpt_lines.extend_from_slice(&lines[start..]);
    } else {
        // Append last 5 summary lines if not already present
        let tail_start = lines.len().saturating_sub(5);
        for &line in &lines[tail_start..] {
            if !excerpt_lines.contains(&line) {
                excerpt_lines.push(line);
            }
        }
    }

    excerpt_lines.join("\n")
}
```

Export in `crates/superstate-core/src/lib.rs`:
```rust
pub mod shunt;
pub use shunt::{evaluate_and_distill, DistilledOutput, ShuntConfig};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-core --test test_shunt`
Expected: PASS (all 3 tests green)

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-core/src/shunt.rs crates/superstate-core/src/lib.rs crates/superstate-core/tests/test_shunt.rs
git commit --no-gpg-sign -m "feat(core): implement distillation engine and raw log storage for tool output shunting"
```

---

### Task 2: CLI Subcommand `superstate shunt` in `crates/superstate-cli`

- **Depends on:** Task 1

**Files:**
- Create: `crates/superstate-cli/src/commands/shunt.rs`
- Modify: `crates/superstate-cli/src/commands/mod.rs`
- Modify: `crates/superstate-cli/src/main.rs:18-77,93-130`
- Test: `crates/superstate-cli/tests/test_cli_shunt.rs`

**Interfaces:**
- Produces: CLI subcommand `superstate shunt [--threshold-lines <N>] -- <command> [args...]`

- [ ] **Step 1: Write the failing CLI integration test**

Create `crates/superstate-cli/tests/test_cli_shunt.rs`:
```rust
use std::process::Command;

#[test]
fn test_cli_shunt_small_output_exit_code_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "echo", "hello world"])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("hello world"));
    assert!(!stdout.contains("[superstate-shunt]"));
}

#[test]
fn test_cli_shunt_propagates_non_zero_exit_code() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--", "sh", "-c", "exit 42"])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(42));
}

#[test]
fn test_cli_shunt_large_output_truncates_and_saves_log() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["shunt", "--threshold-lines", "10", "--", "sh", "-c", "for i in $(seq 1 30); do echo line $i; done"])
        .output()
        .expect("failed to run superstate shunt");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[superstate-shunt] Command succeeded (exit 0)"));
    assert!(stdout.contains("Full log:"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-cli --test test_cli_shunt`
Expected: FAIL with "unrecognized subcommand 'shunt'"

- [ ] **Step 3: Implement `superstate-cli::commands::shunt`**

Create `crates/superstate-cli/src/commands/shunt.rs`:
```rust
use std::env;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use superstate_core::shunt::{evaluate_and_distill, DistilledOutput, ShuntConfig};

pub fn execute_shunt(cmd_args: &[String], threshold_lines: usize) -> Result<i32, String> {
    if cmd_args.is_empty() {
        return Err("No command specified to shunt".to_string());
    }

    let program = &cmd_args[0];
    let args = &cmd_args[1..];
    let cmd_str = cmd_args.join(" ");

    let current_dir = env::current_dir().map_err(|e| e.to_string())?;

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn command '{}': {}", program, e))?;

    // Drain stdout and stderr concurrently to prevent pipe buffer deadlocks (>64 KB)
    let mut stdout_pipe = child.stdout.take().ok_or("Failed to capture stdout")?;
    let mut stderr_pipe = child.stderr.take().ok_or("Failed to capture stderr")?;

    let stdout_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });

    let stderr_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait for child command: {}", e))?;

    let stdout_bytes = stdout_handle.join().unwrap_or_default();
    let stderr_bytes = stderr_handle.join().unwrap_or_default();

    let exit_code = status.code().unwrap_or(1);

    let config = ShuntConfig {
        threshold_lines,
        threshold_bytes: 4096,
        logs_dir: PathBuf::from(".agent-state/logs"),
    };

    let result = evaluate_and_distill(
        &cmd_str,
        &current_dir,
        &stdout_bytes,
        &stderr_bytes,
        exit_code,
        &config,
    )
    .map_err(|e| format!("Shunt distillation error: {}", e))?;

    match result {
        DistilledOutput::Raw { stdout, stderr } => {
            let _ = io::stdout().write_all(&stdout);
            let _ = io::stderr().write_all(&stderr);
        }
        DistilledOutput::Distilled { summary, .. } => {
            println!("{}", summary);
        }
    }

    Ok(exit_code)
}
```

Add to `crates/superstate-cli/src/commands/mod.rs`:
```rust
pub mod shunt;
```

Add `Shunt` subcommand to `crates/superstate-cli/src/main.rs`:
```rust
    /// Execute a shell command through the output shunting distillation layer
    Shunt {
        /// Line count threshold before shunting triggers
        #[arg(long, default_value = "50")]
        threshold_lines: usize,
        /// Command and arguments to execute
        #[arg(trailing_var_arg = true, required = true)]
        cmd: Vec<String>,
    },
```
And wire execution in `main.rs`:
```rust
    Commands::Shunt { threshold_lines, cmd } => {
        match commands::shunt::execute_shunt(&cmd, threshold_lines) {
            Ok(code) => std::process::exit(code),
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-cli --test test_cli_shunt`
Expected: PASS (all 3 CLI tests pass)

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-cli/src/commands/shunt.rs crates/superstate-cli/src/commands/mod.rs crates/superstate-cli/src/main.rs crates/superstate-cli/tests/test_cli_shunt.rs
git commit --no-gpg-sign -m "feat(cli): add superstate shunt command with exit code preservation"
```

---

### Task 3: Driver Wrapper & Daemon Integration in `crates/superstate-core`

- **Depends on:** Task 2

**Files:**
- Modify: `crates/superstate-core/src/daemon.rs:648-675`
- Test: `crates/superstate-core/tests/test_daemon_shunt.rs`

**Interfaces:**
- Produces: `DaemonConfig::shunt_driver_output: bool` flag (default `true`) which sets up shell shimming for worker command executions.

- [ ] **Step 1: Write integration test for daemon driver shunting**

Create `crates/superstate-core/tests/test_daemon_shunt.rs`:
```rust
use std::fs;
use superstate_core::daemon::{DaemonConfig, DaemonRunner};
use superstate_core::models::{ExecutionState, ExecutionStatus, TaskSpec};
use superstate_core::state_machine::save_state_atomic;
use tempfile::tempdir;

#[test]
fn test_daemon_runner_injects_shunt_wrapper() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join("state.json");

    let mut state = ExecutionState::new();
    state.active_task = Some(TaskSpec {
        id: "task-1".to_string(),
        title: "Test Shunt".to_string(),
        description: "Run test".to_string(),
        status: ExecutionStatus::Pending,
        verification_cmd: "echo 'done'".to_string(),
        target_files: vec![],
    });
    save_state_atomic(&state_file, &state).unwrap();

    let config = DaemonConfig {
        state_path: state_file,
        driver_cmd: Some("echo 'driver turn'".to_string()),
        max_retries: 1,
        timeout_secs: 10,
        enable_advisory_review: false,
        review_blocking: false,
        auto_rollback: true,
        shunt_driver_output: true,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let outcome = runner.step().unwrap();
    assert!(outcome.is_advanced());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-core --test test_daemon_shunt`
Expected: FAIL with "no field `shunt_driver_output` on struct `DaemonConfig`"

- [ ] **Step 3: Add `shunt_driver_output` to `DaemonConfig` and wire in `daemon.rs`**

In `crates/superstate-core/src/daemon.rs`:
Add `pub shunt_driver_output: bool` to `DaemonConfig` (defaulting to `true` in `Default`).
When executing the driver in `run_step`, if `self.config.shunt_driver_output` is true, export `SUPERSTATE_SHUNT_ENABLED=1` and create a shim script or prepend `PATH` with `.agent-state/bin` containing a wrapper so child processes run through `superstate shunt`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-core --test test_daemon_shunt`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-core/src/daemon.rs crates/superstate-core/tests/test_daemon_shunt.rs
git commit --no-gpg-sign -m "feat(daemon): wire tool output shunting into driver worker execution environment"
```

---

### Task 4: Full Workspace Compilation, Formatting & Verification

- **Depends on:** Tasks 1, 2, 3

**Files:**
- Modify: `docs/positioning-and-economics.md` (document token savings and context saturation mitigation)

- [ ] **Step 1: Run full workspace test suite**

Run: `cargo test --all`
Expected: All tests pass across `superstate-core`, `superstate-cli`, `superstate-review`, `superstate-mcp`, `superstate-syntax`, `superstate-worktree`.

- [ ] **Step 2: Run clippy and format checks**

Run: `cargo clippy --all -- -D warnings`
Run: `cargo fmt --all -- --check`
Expected: Clean with 0 warnings.

- [ ] **Step 3: Commit documentation update**

```bash
git add docs/positioning-and-economics.md
git commit --no-gpg-sign -m "docs: document tool output shunt layer token economics"
```
