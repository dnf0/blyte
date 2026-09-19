# Task 2 Brief: Reinstall CLI & Verify Cli Tests in `superstate`

## Workspace & Context
- Repo: `/Users/danielfisher/repos/superstate`
- Branch: `feat/review-goal-forwarding`

## What to Implement & Verify
1. Run CLI blocking review tests:
   `cargo test -p superstate --test test_cli_blocking_review`
2. Run full workspace clippy and test suite:
   `cargo clippy --all-targets -- -D warnings`
   `cargo test --all`
3. Reinstall release binary to system:
   `cargo install --path crates/superstate-cli --locked --force`
4. Verify the installed binary in `~/.cargo/bin/superstate`:
   `~/.cargo/bin/superstate state daemon --help`
   Confirm flags `--advisory-review`, `--review-blocking`, `--auto-rollback` are present.
5. Write summary report to `docs/superpowers/plans/task-2-cli-reinstall-report.md`.
