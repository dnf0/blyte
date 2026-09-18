# Task 3 Brief: CLI Flag `--auto-rollback` in `crates/superstate-cli`

## Context & Purpose
Working directory: `/Users/danielfisher/repos/superstate`
Branch: `feat/rapid-canary-eval-and-rollback`

Now that `DaemonConfig.auto_rollback` is supported in `superstate-core`, we must expose `--auto-rollback` in the CLI (`superstate daemon` and `superstate state daemon`), and pass it through detachment when spawning background daemon processes.

## Requirements
1. **CLI Flag Definition** (`crates/superstate-cli/src/state_cli.rs`):
   - In `DaemonArgs`:
     ```rust
     #[arg(
         long,
         help = "Automatically roll back working tree to milestone baseline on verification failure"
     )]
     pub auto_rollback: bool,
     ```
2. **Command Handler Wiring** (`crates/superstate-cli/src/commands/daemon.rs`):
   - In `cmd_daemon`:
     - Set `config.auto_rollback = args.auto_rollback;`
     - If `args.detach`, append `"--auto-rollback"` to `daemon_args` when spawning the detached child process.
3. **Environment Variable Fallback**:
   - Also allow enabling via environment variable `SUPERSTATE_AUTO_ROLLBACK=1` or `true` if flag is not explicitly passed.
4. **Integration Test** (`crates/superstate-cli/tests/test_cli_rollback.rs`):
   - Add integration tests verifying:
     - `superstate state daemon --auto-rollback` parses `args.auto_rollback == true`.
     - `superstate daemon --help` shows `--auto-rollback`.
5. **System Binary Reinstallation**:
   - `cargo install --path crates/superstate-cli --locked --force` so `~/.cargo/bin/superstate` and `state` are updated.
6. **Compile & Verification**:
   - `cargo test -p superstate` must pass cleanly.
   - `cargo check --all` and `cargo clippy --all-targets -- -D warnings` must pass with 0 warnings.
7. **Commit**:
   - Commit with message `feat(cli): wire --auto-rollback flag and detachment forwarding` (use `--no-gpg-sign`).
8. **Report**:
   - Write full report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-3-cli-rollback-report.md`.
