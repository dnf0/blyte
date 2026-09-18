# Default Deterministic Git Rollback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Promote deterministic git rollback on verification failure to the default execution policy in `DaemonRunner` and provide a `--no-auto-rollback` CLI opt-out flag.

**Architecture:** Invert `DaemonConfig.auto_rollback` default to `true`, add serde `default_true` deserialization helper, add `--no-auto-rollback` and `use_auto_rollback()` to `DaemonArgs` in CLI, and align integration tests across `superstate-core` and `superstate-cli`.

**Tech Stack:** Rust (2021 edition), `clap` (derive API), `serde`, `superstate-core`, `superstate-cli`.

## Global Constraints

- Must compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No new external crate dependencies; use existing workspace crates.
- Commits must use Conventional Commits and skip GPG verification (`--no-gpg-sign`).
- Retain backwards compatibility for `--auto-rollback` while providing `--no-auto-rollback`.

---

### Task 1: Core Daemon Defaults in `crates/superstate-core`

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-core/src/daemon.rs:180-200`
- Test: `crates/superstate-core/tests/test_daemon_rollback.rs`

**Interfaces:**
- Produces: `DaemonConfig::default()` with `auto_rollback: true` and `#[serde(default = "default_true")]`.

**Steps:**
- [ ] Write failing test in `test_daemon_rollback.rs` asserting `DaemonConfig::default().auto_rollback == true` and that `DaemonRunner` rolls back by default without explicit flag.
- [ ] Run `cargo test -p superstate-core --test test_daemon_rollback` and verify failure.
- [ ] Update `DaemonConfig` in `crates/superstate-core/src/daemon.rs` to set `auto_rollback: true` in `Default::default()` and use `#[serde(default = "default_true")]`.
- [ ] Run `cargo test -p superstate-core --test test_daemon_rollback` and confirm green pass.
- [ ] Commit with message `feat(core): enable deterministic rollback by default in DaemonConfig`.

---

### Task 2: CLI Opt-Out & Environmental Parsing in `crates/superstate-cli`

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-cli/src/state_cli.rs:316-324`
- Modify: `crates/superstate-cli/src/commands/daemon.rs:115-130`
- Test: `crates/superstate-cli/tests/test_cli_rollback.rs`

**Interfaces:**
- Produces: `DaemonArgs.no_auto_rollback: bool`, `DaemonArgs::use_auto_rollback(&self) -> bool`.

**Steps:**
- [ ] Write failing CLI tests in `test_cli_rollback.rs` checking:
  1. Default CLI parse has `use_auto_rollback() == true`.
  2. `--no-auto-rollback` flag sets `use_auto_rollback() == false`.
  3. `SUPERSTATE_DAEMON_AUTO_ROLLBACK=0` sets `use_auto_rollback() == false`.
  4. Execution test: verification failure rolls back by default; passes when `--no-auto-rollback` keeps dirty tree.
- [ ] Run `cargo test -p superstate-cli --test test_cli_rollback` and verify failure.
- [ ] Add `no_auto_rollback` and `use_auto_rollback()` to `DaemonArgs` in `crates/superstate-cli/src/state_cli.rs`.
- [ ] Wire `args.use_auto_rollback()` in `crates/superstate-cli/src/commands/daemon.rs`.
- [ ] Run `cargo test -p superstate-cli --test test_cli_rollback` and confirm green pass.
- [ ] Commit with message `feat(cli): add --no-auto-rollback and default to automatic rollback`.

---

### Task 3: Full Workspace Verification & Binary Build

- **Depends on:** Task 1, Task 2

**Files:**
- Modify: Any existing integration tests expecting default `auto_rollback: false`.
- Build: `target/release/superstate`

**Steps:**
- [ ] Run `cargo test --all` across the entire workspace to identify any tests broken by the new default.
- [ ] Update any legacy tests expecting `auto_rollback: false` to explicitly pass `auto_rollback: false`.
- [ ] Run `cargo clippy --all -- -D warnings`.
- [ ] Build release binary: `cargo build --release -p superstate-cli`.
- [ ] Verify `./target/release/superstate daemon --help` shows `--no-auto-rollback`.
- [ ] Commit any test adjustments with message `test(all): align integration tests with default auto-rollback`.
