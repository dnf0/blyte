# Task 3 Brief: Automated Workspace Hook Provisioning & Parity Verification

## Context & Base Commit
- **Working Directory:** `/Users/danielfisher/repos/superstate/.worktrees/feat-pre-tool-use-file-guard`
- **Base Commit:** `62e7cfb`
- **Spec:** `docs/superpowers/specs/2026-09-20-file-read-guard-design.md`
- **Plan:** `docs/superpowers/plans/2026-09-20-file-read-guard.md`

## What Needs to be Built
Wire automated file read guard hook provisioning into `superstate-core::daemon` and `superstate-cli`.

### 1. `crates/superstate-core/src/daemon.rs`
1. Update `DaemonConfig`:
   ```rust
   /// Automatically guard file reads and shunt large files to symbol outlines
   pub guard_file_reads: bool,
   ```
   In `impl Default for DaemonConfig`:
   ```rust
   guard_file_reads: true,
   ```
2. In `DaemonRunner::step()`:
   When setting up driver execution (around where `shunt_driver_output` is handled):
   If `self.config.guard_file_reads` is true:
   - Ensure `working_dir.join(".agents")` exists (`fs::create_dir_all`).
   - Check if `working_dir.join(".agents/hooks.json")` exists. If not, write formatted JSON:
     ```json
     {
       "superstate-file-guard": {
         "PreToolUse": [
           {
             "matcher": "view_file|View|read_file",
             "hooks": [
               {
                 "command": "superstate guard pre-tool-use --threshold-lines 200"
               }
             ]
           }
         ]
       }
     }
     ```

### 2. `crates/superstate-cli/src/state_cli.rs` & `commands/daemon.rs`
1. In `DaemonArgs` (`crates/superstate-cli/src/state_cli.rs`):
   ```rust
   /// Disable automatic file read guard during driver execution
   #[arg(
       long,
       help = "Disable automatic file read guard during driver execution"
   )]
   pub no_guard: bool,
   ```
   Add helper:
   ```rust
   pub fn use_guard(&self) -> bool {
       !self.no_guard
   }
   ```
2. In `commands/daemon.rs`:
   Forward `guard_file_reads: args.use_guard()` into `DaemonConfig`.

### 3. Integration Testing
1. In `crates/superstate-core/tests/test_daemon_guard.rs`:
   - Verify `DaemonRunner` with `guard_file_reads: true` provisions `.agents/hooks.json`.
   - Verify `DaemonRunner` with `guard_file_reads: false` does not provision `.agents/hooks.json`.
   - Verify valid JSON structure in provisioned file.
2. In `crates/superstate-cli/tests/test_cli_daemon_guard.rs`:
   - Verify CLI parsing of `--no-guard`.

## Steps (Follow TDD Strictly)
1. Write failing integration tests in `crates/superstate-core/tests/test_daemon_guard.rs` and `crates/superstate-cli/tests/test_cli_daemon_guard.rs`.
2. Run `cargo test --test test_daemon_guard --test test_cli_daemon_guard` to confirm failure.
3. Implement `guard_file_reads` in `daemon.rs`, `state_cli.rs`, and `commands/daemon.rs`.
4. Run `cargo test --test test_daemon_guard --test test_cli_daemon_guard` to confirm it passes.
5. Run `cargo test --all` and `cargo clippy --all-targets -- -D warnings`.
6. Commit using `git commit --no-gpg-sign -m "feat(daemon): provision automated file read guard hooks in workspace"`.
7. Write your implementer report to `docs/superpowers/plans/task-3-report.md`.
