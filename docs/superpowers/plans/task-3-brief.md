# Task 3 Brief: Driver Wrapper & Daemon Integration in `crates/superstate-core` and `crates/superstate-cli`

## Task Spec
Implement automatic environment injection and shell shimming for the daemon driver worker:
- Add `shunt_driver_output: bool` to `DaemonConfig` (defaulting to `true`).
- When running the driver worker, if `shunt_driver_output` is true:
  - Create `.agent-state/bin/shunt` wrapper script with executable permissions (`0o755`).
  - Prepend `.agent-state/bin` to child process `PATH`.
  - Export `SUPERSTATE_SHUNT_ENABLED=1`.
- Add `--no-shunt` flag to `DaemonArgs` in `crates/superstate-cli/src/state_cli.rs`.
- Wire `shunt_driver_output: args.use_shunt()` through CLI commands and detached child invocation.
- Apply stream flush polish in `crates/superstate-cli/src/commands/shunt.rs`.
- Tests in `crates/superstate-core/tests/test_daemon_shunt.rs`.
- Must compile cleanly and pass with `cargo test --all` and `cargo clippy --all -- -D warnings`.
- Conventional commit skipping GPG verification.
