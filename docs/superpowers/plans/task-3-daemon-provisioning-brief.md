# Task 3 Brief: Daemon Automated Provisioning & Untracked Hygiene in `superstate`

## Workspace & Context
- **Working Directory:** `/Users/danielfisher/repos/superstate/.worktrees/feat-pre-tool-use-command-guard`
- **Feature Branch:** `feat/pre-tool-use-command-guard`
- **Spec:** `docs/superpowers/specs/2026-09-20-pre-tool-use-command-polling-guard-design.md`
- **Plan:** `docs/superpowers/plans/2026-09-20-pre-tool-use-command-polling-guard.md`

## Objective
Wire automated workspace provisioning of `superstate-command-guard` into `DaemonRunner::step()`, add CLI flags `--no-command-guard` (with `--no-guard` disabling both), ensure `.agent-state/command_guard_history.json` and `.agents/` are cleanly untracked and excluded, and update the installed release binary.

## Files to Create / Modify
- **Modify:** `crates/superstate-core/src/daemon.rs` (add `guard_commands: bool` to `DaemonConfig`, provision hook in `step()`)
- **Modify:** `crates/superstate-cli/src/state_cli.rs` (add `--no-command-guard`, wire `use_command_guard()`)
- **Modify:** `crates/superstate-cli/src/commands/daemon.rs` (forward flag into `DaemonConfig` and detached subprocess)
- **Modify / Extend Tests:**
  - `crates/superstate-core/tests/test_daemon_guard.rs`
  - `crates/superstate-cli/tests/test_cli_daemon_guard.rs`
- **Report:** `docs/superpowers/plans/task-3-daemon-provisioning-report.md`

## Interface Specification

### `crates/superstate-core/src/daemon.rs`
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonConfig {
    // Existing fields...
    pub guard_file_reads: bool,
    pub guard_commands: bool, // NEW: default true
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            // Existing defaults...
            guard_file_reads: true,
            guard_commands: true,
        }
    }
}
```

### Provisioning in `step()`
When provisioning `.agents/hooks.json`:
- If `hooks.json` does not exist, create it with active guards:
  - If `guard_file_reads`: add `superstate-file-guard` (`view_file|View|read_file`).
  - If `guard_commands`: add `superstate-command-guard` (`run_command|manage_task`).
- If `hooks.json` already exists (e.g. from previous step or existing project), load existing JSON object and merge/insert the missing guard keys without overwriting existing unrelated user hooks.
- Append `.agents/\n` to `.git/info/exclude` if not already present.
- Verify `.agent-state` is ignored by git checkpoints and untracked tree fingerprinting.

### CLI Flags in `state_cli.rs` & `commands/daemon.rs`
```rust
#[derive(Args, Debug, Clone)]
pub struct DaemonArgs {
    // Existing args...
    /// Disable automated PreToolUse command & polling guard
    #[arg(long, default_value_t = false)]
    pub no_command_guard: bool,
}
```
- In `DaemonArgs::use_command_guard(&self) -> bool`:
  `!self.no_guard && !self.no_command_guard`
- In `DaemonArgs::use_guard(&self) -> bool`:
  `!self.no_guard && !self.no_file_guard` (or alias)
- Forward `--no-command-guard` to detached daemon subprocess in `cmd_daemon` if detached.

## Required TDD Steps
1. Write failing tests in `crates/superstate-core/tests/test_daemon_guard.rs` and `crates/superstate-cli/tests/test_cli_daemon_guard.rs`:
   - `test_daemon_provisions_command_guard_in_hooks_json`: verify `superstate-command-guard` exists with matcher `run_command|manage_task` and command `superstate guard pre-command --debounce-sec 30`.
   - `test_daemon_no_command_guard_disables_provisioning`: verify `--no-command-guard` prevents `superstate-command-guard` from being added.
   - `test_daemon_no_guard_disables_both`: verify `--no-guard` disables both file and command guards.
   - `test_daemon_merges_hooks_when_file_exists`: verify existing user hooks in `hooks.json` are preserved when daemon injects guards.
2. Run tests to verify RED failure: `cargo test -p superstate-core --test test_daemon_guard`.
3. Implement provisioning logic and CLI flags.
4. Run tests to verify GREEN pass: `cargo test -p superstate-core --test test_daemon_guard` and `cargo test -p superstate --test test_cli_daemon_guard`.
5. Run full workspace verification:
   - `cargo test --all`
   - `cargo clippy --all-targets -- -D warnings`
6. Install updated release binary: `cargo install --path crates/superstate-cli --locked --force`.
7. Commit changes: `git commit -m "feat(daemon): provision automated command polling guard in workspace hooks" --no-gpg-sign`.
8. Write report to `docs/superpowers/plans/task-3-daemon-provisioning-report.md`.
