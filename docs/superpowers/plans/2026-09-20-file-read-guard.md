# File Read Guard & AST Shunting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a deterministic `PreToolUse` file read guard (`superstate guard pre-tool-use`) and automated workspace hook provisioning that blocks unbounded large file reads (> 200 lines) into agent context and returns AST symbol outlines without prompt engineering.

**Architecture:** A sub-millisecond Rust guard engine in `superstate-core` evaluates file length and range boundaries against a 200-line budget; `superstate-cli` exposes `guard pre-tool-use` conforming to the Antigravity/Claude Code `PreToolUse` JSON protocol; and `DaemonRunner` automatically provisions `.agents/hooks.json` in the workspace root.

**Tech Stack:** Rust 2021, `superstate-syntax` (tree-sitter AST extraction), `serde_json`, `clap`.

## Global Constraints
- Compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No compiler or clippy warnings (`cargo clippy --all-targets -- -D warnings`).
- Skip GPG verification on all commits (`--no-gpg-sign`).
- Zero prompt engineering: all behavior governed by git diffs, process exit codes (`$? == 0`), and tool hook protocol payloads.
- Parity invariant: `tests/test_arm_input_parity.py` in `superstate-eval` must pass 100%.

---

## Tasks

### Task 1: `superstate-core` Guard Engine & Evaluation Logic

- **Depends on:** none

**Files:**
- Create: `crates/superstate-core/src/guard.rs`
- Modify: `crates/superstate-core/src/lib.rs`
- Test: `crates/superstate-core/tests/test_guard.rs`

**Interfaces:**
- Consumes: `superstate_syntax::inspect::{detect_language, format_outline}`
- Produces:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
  #[serde(tag = "decision", rename_all = "snake_case")]
  pub enum GuardDecision {
      Allow,
      Deny { reason: String },
  }

  #[derive(Debug, Clone)]
  pub struct FileReadGuardConfig {
      pub threshold_lines: usize,
  }

  pub fn evaluate_file_read_guard(
      path: &std::path::Path,
      start_line: Option<usize>,
      end_line: Option<usize>,
      config: &FileReadGuardConfig,
  ) -> GuardDecision;
  ```

- [ ] **Step 1: Write failing unit tests in `test_guard.rs`**
  Cover:
  - Small file ($\le 200$ lines) $\rightarrow$ `Allow`
  - Bounded slice $\le 200$ lines (`end_line - start_line <= 200`) $\rightarrow$ `Allow`
  - Large file unbounded $\rightarrow$ `Deny` with AST symbol outline
  - Large file with slice $> 200$ lines $\rightarrow$ `Deny` with AST symbol outline
  - Non-existent file $\rightarrow$ `Allow`
  - Binary file $\rightarrow$ `Deny`
  - Unsupported language fallback $\rightarrow$ `Deny` with line count summary
- [ ] **Step 2: Run `cargo test -p superstate-core --test test_guard` and confirm failure**
- [ ] **Step 3: Implement `guard.rs` and re-export in `lib.rs`**
- [ ] **Step 4: Run `cargo test -p superstate-core --test test_guard` and confirm it passes**
- [ ] **Step 5: Run `cargo clippy -p superstate-core --all-targets -- -D warnings`**
- [ ] **Step 6: Commit changes with message `feat(core): implement file read guard engine and ast shunting`**

---

### Task 2: `superstate guard pre-tool-use` CLI Subcommand

- **Depends on:** Task 1

**Files:**
- Create: `crates/superstate-cli/src/commands/guard.rs`
- Modify: `crates/superstate-cli/src/commands/mod.rs`
- Modify: `crates/superstate-cli/src/main.rs`
- Test: `crates/superstate-cli/tests/test_cli_guard.rs`

**Interfaces:**
- Consumes: `superstate_core::guard::{evaluate_file_read_guard, FileReadGuardConfig, GuardDecision}`
- Produces:
  - CLI command: `superstate guard pre-tool-use [--threshold-lines <N>]`
  - Reads `stdin` JSON payload conforming to `PreToolUse` schema.
  - Writes `stdout` JSON payload (`{"decision": "allow"}` or `{"decision": "deny", "reason": "..."}`).

- [ ] **Step 1: Write failing integration test in `test_cli_guard.rs`**
  Pipe JSON `PreToolUse` payloads for `view_file` (both allowed and denied) to `superstate guard pre-tool-use` process stdin and assert stdout JSON.
- [ ] **Step 2: Run `cargo test -p superstate-cli --test test_cli_guard` and confirm failure**
- [ ] **Step 3: Implement `commands/guard.rs`, register in `commands/mod.rs` and `main.rs`**
- [ ] **Step 4: Run `cargo test -p superstate-cli --test test_cli_guard` and confirm it passes**
- [ ] **Step 5: Run `cargo clippy -p superstate-cli --all-targets -- -D warnings`**
- [ ] **Step 6: Commit changes with message `feat(cli): add superstate guard pre-tool-use CLI subcommand`**

---

### Task 3: Automated Workspace Hook Provisioning & Parity Verification

- **Depends on:** Task 2

**Files:**
- Modify: `crates/superstate-core/src/daemon.rs`
- Modify: `lab/engine/adapters/superstate_daemon.py`
- Test: `crates/superstate-core/tests/test_daemon.rs`
- Test: `superstate-eval/tests/test_arm_input_parity.py`

**Interfaces:**
- Consumes: `superstate guard pre-tool-use`
- Produces:
  - Workspace `.agents/hooks.json` containing matcher on `view_file|View|read_file` invoking `superstate guard pre-tool-use --threshold-lines 200`.

- [ ] **Step 1: Write integration test verifying `.agents/hooks.json` creation on daemon initialization**
- [ ] **Step 2: Run test and confirm failure**
- [ ] **Step 3: Implement hook file creation in `daemon.rs` and update `superstate_daemon.py`**
- [ ] **Step 4: Run test and confirm it passes**
- [ ] **Step 5: Verify arm input parity in `superstate-eval` (`pytest tests/test_arm_input_parity.py`)**
- [ ] **Step 6: Run full workspace test suite `cargo test --all` and `cargo clippy --all-targets -- -D warnings`**
- [ ] **Step 7: Commit changes with message `feat(daemon): provision automated file read guard hooks in workspace`**
