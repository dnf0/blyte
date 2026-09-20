# Task 1 Brief: Core Polling Guard & Cooldown State Engine in `superstate-core`

## Workspace & Context
- **Working Directory:** `/Users/danielfisher/repos/superstate/.worktrees/feat-pre-tool-use-command-guard`
- **Feature Branch:** `feat/pre-tool-use-command-guard`
- **Spec:** `docs/superpowers/specs/2026-09-20-pre-tool-use-command-polling-guard-design.md`
- **Plan:** `docs/superpowers/plans/2026-09-20-pre-tool-use-command-polling-guard.md`

## Objective
Implement a deterministic PreToolUse command and polling guard engine in `crates/superstate-core` that enforces a cooldown debounce on process inspection, task status checks, and repeated commands.

## Files to Create / Modify
- **Create:** `crates/superstate-core/src/command_guard.rs`
- **Modify:** `crates/superstate-core/src/lib.rs` (export `command_guard` module)
- **Test:** `crates/superstate-core/tests/test_command_guard.rs`
- **Report:** `docs/superpowers/plans/task-1-command-guard-report.md`

## Interface Specification
```rust
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandGuardConfig {
    pub debounce_sec: u64,
    pub state_path: Option<PathBuf>,
}

impl Default for CommandGuardConfig {
    fn default() -> Self {
        Self {
            debounce_sec: 30,
            state_path: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum CommandGuardDecision {
    Allow,
    Deny { reason: String },
}

pub fn evaluate_command_guard(
    tool_name: &str,
    args: &serde_json::Map<String, serde_json::Value>,
    config: &CommandGuardConfig,
) -> CommandGuardDecision;
```

## Detection & Enforcement Rules
1. **Tool Classification:**
   - If `tool_name.eq_ignore_ascii_case("manage_task")`:
     - Inspect `Action` or `action` argument. If `"status"` or `"list"`, classification is `manage_task_status`.
   - If `tool_name.eq_ignore_ascii_case("run_command")` (or shell execution tool):
     - Inspect `CommandLine` / `command` / `cmd`.
     - Check procfs patterns: regex `r"/proc/(\d+|\*)/(cmdline|stat|status)"`.
     - Check process inspection: regex `r"\b(ps\s+aux|ps\s+-ef|pgrep|top|htop|jobs)\b"`.
     - Exact duplicate detection: SHA-256 hash or normalized representation of `CommandLine`.
   - If none match, return `CommandGuardDecision::Allow` immediately.
2. **State & Cooldown:**
   - State path defaults to `.agent-state/command_guard_history.json`.
   - Track timestamp in seconds since UNIX epoch.
   - If entry exists and $t_{\text{now}} - t_{\text{last}} < \text{debounce\_sec}$:
     - Return `CommandGuardDecision::Deny { reason: format!("[superstate-guard] Busy-wait polling blocked: command was executed {}s ago. Cooldown active ({}s remaining). Do not loop on status; yield turn to await background task completion notification.", elapsed, remaining) }`.
   - Otherwise, update timestamp to $t_{\text{now}}$, prune entries older than $2 \times \text{debounce\_sec}$, write state file, and return `CommandGuardDecision::Allow`.
3. **Fail-Open Policy:**
   - If state file cannot be opened/written (I/O error, permissions, concurrent write), return `CommandGuardDecision::Allow`.
   - If state file is corrupted JSON, overwrite or return `CommandGuardDecision::Allow`.
   - Never panic!

## Required TDD Steps
1. Write failing tests in `crates/superstate-core/tests/test_command_guard.rs`.
2. Run `cargo test -p superstate-core --test test_command_guard` (verify RED failure).
3. Implement `crates/superstate-core/src/command_guard.rs` and export in `crates/superstate-core/src/lib.rs`.
4. Run `cargo test -p superstate-core --test test_command_guard` (verify GREEN pass).
5. Run `cargo clippy -p superstate-core --all-targets -- -D warnings`.
6. Commit changes: `git commit -m "feat(core): implement PreToolUse command and polling guard engine" --no-gpg-sign`.
7. Write report to `docs/superpowers/plans/task-1-command-guard-report.md`.
