# Superstate Interactive & Benchmark Agent Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transform Superstate into an interactive terminal agent harness (like Claude Code) powered by the SKILL.state (arXiv:2608.26263v2) and frontier benchmark execution engine (`dsh`/`codex-exec`).

**Architecture:** Unify the binary surface under `superstate` (retiring legacy secondary binaries). Route `superstate` CLI entrypoint dynamically: subcommands execute utilities (`doctor`, `review`, `mcp`), direct prompts run headless single-shot solver mode, and no-argument invocations launch the interactive terminal REPL with live event streaming and slash commands.

**Tech Stack:** Rust 2021, `clap` (derive), `superstate-agent`, `superstate-provider`, `superstate-core`, `serde`, `serde_json`.

## Global Constraints

- Target crate: `crates/superstate-cli`, utilizing `crates/superstate-agent` and `crates/superstate-provider`
- Rust 2021 edition, zero compiler warnings (`cargo check --workspace --all-targets`)
- Strict conventional commit messages (`feat:`, `fix:`, `docs:`, `test:`, `build:`) with `--no-gpg-sign`
- Roborev automated continuous review after each commit (`roborev status; roborev show HEAD`)
- All tests must pass on macOS and Linux (`cargo test --workspace --all-targets`)

---

### Task 1: Binary Unification & Workspace Dependencies in `crates/superstate-cli/Cargo.toml`

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-cli/Cargo.toml`
- Remove: `crates/superstate-cli/src/state_main.rs` (legacy separate binary entrypoint)

**Interfaces:**
- Consumes: `crates/superstate-agent`, `crates/superstate-provider`
- Produces: `superstate` as the sole compiled binary with agent and provider dependencies enabled

- [ ] **Step 1: Write the failing check test**

Run: `cargo check -p superstate`
Verify that `superstate-agent` is not yet available in `superstate_cli` scope.

- [ ] **Step 2: Update `crates/superstate-cli/Cargo.toml`**

Remove the `[[bin]] name = "state"` block.
Add `superstate-agent` and `superstate-provider` dependencies:

```toml
[package]
name = "superstate"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
repository.workspace = true
readme = "README.md"
description = "Superstate unified interactive agent harness and state engine runner."

[lib]
name = "superstate_cli"
path = "src/lib.rs"

[[bin]]
name = "superstate"
path = "src/main.rs"

[dependencies]
libc = "0.2"
superstate-core = { path = "../superstate-core", version = "0.1.0" }
superstate-syntax = { path = "../superstate-syntax", version = "0.1.0" }
superstate-review = { path = "../superstate-review", version = "0.1.0" }
superstate-rules = { path = "../superstate-rules", version = "0.1.0" }
superstate-mcp = { path = "../superstate-mcp", version = "0.1.0" }
superstate-worktree = { path = "../superstate-worktree", version = "0.1.0" }
superstate-agent = { path = "../superstate-agent", version = "0.1.0" }
superstate-provider = { path = "../superstate-provider", version = "0.1.0" }
clap.workspace = true
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
thiserror.workspace = true
fd-lock.workspace = true
tempfile.workspace = true
```

- [ ] **Step 3: Remove legacy `src/state_main.rs`**

Delete `crates/superstate-cli/src/state_main.rs` so only the unified `superstate` binary exists.

- [ ] **Step 4: Verify check passes**

Run: `cargo check -p superstate`
Expected: PASS with 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-cli/Cargo.toml
git rm crates/superstate-cli/src/state_main.rs
git commit --no-gpg-sign -m "build(cli): unify binary under superstate and add agent dependencies"
```

---

### Task 2: CLI Argument Parser with Dual-Mode Support

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-cli/src/main.rs:1-96`
- Create: `crates/superstate-cli/tests/test_cli_dispatch.rs`

**Interfaces:**
- Consumes: `clap::Parser`, `clap::Subcommand`
- Produces: `Cli` struct supporting optional prompt, `--gate`, `--context`, `--model`, `--container`, `--max-turns`, `--max-gates`, `--rollback`, and optional subcommands

- [ ] **Step 1: Write the failing test in `crates/superstate-cli/tests/test_cli_dispatch.rs`**

```rust
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "superstate")]
struct CliTest {
    #[arg(index = 1)]
    prompt: Option<String>,

    #[arg(short, long)]
    gate: Option<String>,

    #[arg(long, default_value = "conversational")]
    context: String,

    #[arg(short, long)]
    model: Option<String>,

    #[arg(long)]
    container: Option<String>,

    #[arg(long, default_value = "25")]
    max_turns: usize,

    #[arg(long, default_value = "5")]
    max_gates: usize,

    #[arg(long, default_value_t = true)]
    rollback: bool,

    #[command(subcommand)]
    command: Option<SubTest>,
}

#[derive(clap::Subcommand, Debug, PartialEq)]
enum SubTest {
    Doctor,
}

#[test]
fn test_parse_no_args_launches_interactive() {
    let cli = CliTest::try_parse_from(["superstate"]).unwrap();
    assert!(cli.prompt.is_none());
    assert!(cli.command.is_none());
}

#[test]
fn test_parse_single_shot_prompt() {
    let cli = CliTest::try_parse_from(["superstate", "fix pytest failure in stats.py", "--gate", "pytest"]).unwrap();
    assert_eq!(cli.prompt.as_deref(), Some("fix pytest failure in stats.py"));
    assert_eq!(cli.gate.as_deref(), Some("pytest"));
    assert_eq!(cli.context, "conversational");
    assert!(cli.command.is_none());
}

#[test]
fn test_parse_subcommand_routes_cleanly() {
    let cli = CliTest::try_parse_from(["superstate", "doctor"]).unwrap();
    assert_eq!(cli.command, Some(SubTest::Doctor));
    assert!(cli.prompt.is_none());
}
```

- [ ] **Step 2: Run test to verify it compiles and runs**

Run: `cargo test -p superstate --test test_cli_dispatch`
Expected: PASS

- [ ] **Step 3: Update `crates/superstate-cli/src/main.rs` `Cli` struct**

Modify `Cli` to support optional `prompt`, execution flags (`gate`, `context`, `model`, `container`, `max_turns`, `max_gates`, `rollback`), and `Option<Commands>` for subcommands.

- [ ] **Step 4: Verify check passes**

Run: `cargo check -p superstate`
Expected: PASS with 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-cli/src/main.rs crates/superstate-cli/tests/test_cli_dispatch.rs
git commit --no-gpg-sign -m "feat(cli): add dual-mode prompt and execution flags to CLI parser"
```

---

### Task 3: Interactive REPL Module with Live Event Streaming and Slash Commands

- **Depends on:** Task 2

**Files:**
- Create: `crates/superstate-cli/src/repl.rs`
- Modify: `crates/superstate-cli/src/lib.rs` (add `pub mod repl;`)
- Create: `crates/superstate-cli/tests/test_repl_commands.rs`

**Interfaces:**
- Consumes: `superstate_agent::run::{Runner, Budget, Event}`, `superstate_agent::state::State`, `superstate_agent::shell::LocalShell`, `superstate_agent::fs::LocalFs`, `superstate_provider::gemini::Gemini`
- Produces: `pub fn run_repl(config: ReplConfig) -> Result<(), String>` and interactive slash command handler (`/help`, `/state`, `/diff`, `/review`, `/doctor`, `/gate`, `/clear`, `/exit`)

- [ ] **Step 1: Write the failing test in `crates/superstate-cli/tests/test_repl_commands.rs`**

```rust
use std::io::Cursor;
use superstate_cli::repl::{handle_repl_command, ReplSession};

#[test]
fn test_slash_help_command() {
    let mut session = ReplSession::new("test-repo".into(), None);
    let mut out = Vec::new();
    let handled = handle_repl_command("/help", &mut session, &mut out).unwrap();
    assert!(handled);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("/help"));
    assert!(text.contains("/state"));
    assert!(text.contains("/diff"));
}

#[test]
fn test_slash_gate_command() {
    let mut session = ReplSession::new("test-repo".into(), None);
    let mut out = Vec::new();
    handle_repl_command("/gate cargo test", &mut session, &mut out).unwrap();
    assert_eq!(session.gate.as_deref(), Some("cargo test"));
}
```

- [ ] **Step 2: Run test to observe failure**

Run: `cargo test -p superstate --test test_repl_commands`
Expected: FAIL (missing `repl` module).

- [ ] **Step 3: Implement `crates/superstate-cli/src/repl.rs`**

Implement `ReplSession`, `handle_repl_command`, `render_banner`, formatters for `Event::Turn`, `Event::Tool`, `Event::Gate`, and `Event::RolledBack`, and `run_repl_loop` reading from any `std::io::BufRead` source.

- [ ] **Step 4: Register `pub mod repl;` in `crates/superstate-cli/src/lib.rs`**

Add `pub mod repl;` to `crates/superstate-cli/src/lib.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p superstate --test test_repl_commands`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/superstate-cli/src/repl.rs crates/superstate-cli/src/lib.rs crates/superstate-cli/tests/test_repl_commands.rs
git commit --no-gpg-sign -m "feat(cli): add interactive REPL module with live event streaming and slash commands"
```

---

### Task 4: Headless Single-Shot Solver Integration in `crates/superstate-cli`

- **Depends on:** Task 3

**Files:**
- Create: `crates/superstate-cli/src/task_runner.rs`
- Modify: `crates/superstate-cli/src/lib.rs` (add `pub mod task_runner;`)
- Modify: `crates/superstate-cli/src/main.rs` (dispatch to `task_runner::run_task` or `repl::run_repl`)

**Interfaces:**
- Consumes: `superstate_agent::run::Runner`, `superstate_agent::context::{Conversational, StateOnly}`, `superstate_agent::shell::{LocalShell, ContainerShell}`
- Produces: `pub fn run_task(args: TaskArgs) -> Result<(), String>`

- [ ] **Step 1: Write the failing unit test for `TaskArgs` in `crates/superstate-cli/tests/test_task_runner.rs`**

```rust
use superstate_cli::task_runner::TaskArgs;

#[test]
fn test_task_args_defaults() {
    let args = TaskArgs {
        prompt: "fix issue".into(),
        gate: Some("pytest".into()),
        context_strategy: "state-only".into(),
        model: None,
        container: None,
        max_turns: 20,
        max_gates: 4,
        rollback: true,
    };
    assert_eq!(args.context_strategy, "state-only");
    assert_eq!(args.max_turns, 20);
}
```

- [ ] **Step 2: Implement `crates/superstate-cli/src/task_runner.rs`**

Implement `TaskArgs`, `run_task`, wire `Runner` with event logger printing turns, tool outcomes, and gate verdicts, and handle exit codes based on `Outcome`.

- [ ] **Step 3: Wire `main.rs` dispatch**

In `crates/superstate-cli/src/main.rs`:
- If `cli.command` is `Some(cmd)`: execute subcommand.
- If `cli.command` is `None` and `cli.prompt` is `Some(prompt)`: execute `task_runner::run_task(...)`.
- If `cli.command` is `None` and `cli.prompt` is `None`: execute `repl::run_repl(...)`.

- [ ] **Step 4: Verify workspace check and test**

Run: `cargo test -p superstate`
Expected: PASS with 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-cli/src/task_runner.rs crates/superstate-cli/src/lib.rs crates/superstate-cli/src/main.rs crates/superstate-cli/tests/test_task_runner.rs
git commit --no-gpg-sign -m "feat(cli): wire headless single-shot solver and interactive REPL into superstate entrypoint"
```

---

### Task 5: End-to-End Integration Tests & CLI Verification

- **Depends on:** Task 4

**Files:**
- Create: `crates/superstate-cli/tests/test_e2e_cli.rs`
- Modify: `README.md` (document `superstate` interactive and headless usage)

**Interfaces:**
- Consumes: `superstate` compiled binary
- Produces: Verified end-to-end execution of `superstate --help`, `superstate doctor`, and piped REPL interactions

- [ ] **Step 1: Write E2E CLI test `crates/superstate-cli/tests/test_e2e_cli.rs`**

```rust
use std::process::Command;

#[test]
fn test_superstate_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .arg("--help")
        .output()
        .expect("failed to run superstate --help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Superstate"));
    assert!(stdout.contains("--gate"));
    assert!(stdout.contains("--context"));
}

#[test]
fn test_superstate_doctor_subcommand() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .arg("doctor")
        .output()
        .expect("failed to run superstate doctor");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Verification Handshake Passed"));
}
```

- [ ] **Step 2: Run E2E tests**

Run: `cargo test -p superstate --test test_e2e_cli`
Expected: PASS.

- [ ] **Step 3: Update `README.md` with unified `superstate` CLI instructions**

Document:
- `superstate` (launches interactive REPL)
- `superstate "fix bug in tests" --gate "pytest"` (headless single-shot solver)
- `superstate doctor` / `superstate review` (subcommands)

- [ ] **Step 4: Run full verification checklist**

Run:
```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```
Expected: PASS with 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-cli/tests/test_e2e_cli.rs README.md
git commit --no-gpg-sign -m "test(cli): add end-to-end CLI integration tests and update documentation"
```
