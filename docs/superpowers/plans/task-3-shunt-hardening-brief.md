# Task 3 Brief: Shunt CLI Stdin Streaming & Exit Code Verification

## Objective
Harden and verify `superstate shunt -- <command> [args...]` in `crates/superstate-cli` so that:
1. Piped `stdin` is streamed directly into the spawned child process without blocking or truncation.
2. The exact integer exit code of the child process (0, 1, 42, 127) is returned to the parent caller.
3. Outputs over threshold (default 50 lines or 4 KB) are distilled and written to `.agent-state/logs/` with summary emitted to stdout, while outputs under threshold are emitted verbatim.
4. The CLI supports passing `--threshold-lines <N>` and `--logs-dir <DIR>`.

## Working Directory
`/Users/danielfisher/repos/superstate/.worktrees/feat-shunt-hardening`

## Global Constraints
- Compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No new external crate dependencies.
- Zero network I/O.
- Conventional commit with `--no-gpg-sign`.

## Files to Touch
- Modify: `crates/superstate-cli/src/commands/shunt.rs` (and `args.rs` if needed)
- Create: `crates/superstate-cli/tests/test_cli_shunt.rs`

## Detailed Steps
1. Write integration tests in `crates/superstate-cli/tests/test_cli_shunt.rs`:
   - `test_shunt_under_threshold_outputs_verbatim`: Run `superstate shunt -- sh -c 'echo "hello verbatim"'` -> exit 0, stdout contains `"hello verbatim"`.
   - `test_shunt_over_threshold_distills_success`: Run `superstate shunt --threshold-lines 5 -- sh -c 'for i in $(seq 1 20); do echo "line $i"; done'` -> exit 0, stdout contains `[superstate-shunt] Command succeeded (exit 0)`.
   - `test_shunt_preserves_exit_code`: Run `superstate shunt -- sh -c 'exit 42'` -> exit code is 42.
   - `test_shunt_failure_distills_with_excerpt`: Run `superstate shunt --threshold-lines 5 -- sh -c 'for i in $(seq 1 20); do echo "line $i"; done; echo "FAIL: assertion failed"; exit 1'` -> exit code 1, stdout contains `[superstate-shunt] Command failed with exit code 1` and `FAIL: assertion failed`.
   - `test_shunt_forwards_stdin`: Run `superstate shunt -- sh -c 'cat; exit 17'` with stdin `"test stdin content\n"` -> exit code 17, stdout contains `"test stdin content"`.
2. Run `cargo test -p superstate-cli --test test_cli_shunt` (Verify tests pass or reveal fixes).
3. If any issues found in `execute_shunt` (e.g. `Stdio::inherit()` for stdin or piping stdin cleanly), fix in `crates/superstate-cli/src/commands/shunt.rs`.
4. Run `cargo test --all` and `cargo clippy --all-targets -- -D warnings`.
5. Install updated binary: `cargo install --path crates/superstate-cli --locked --force`.
6. Verify `superstate shunt --version` or `superstate shunt --help`.
7. Commit: `feat(cli): verify and harden superstate shunt stdin streaming and exit propagation` (`--no-gpg-sign`).
8. Write report to `docs/superpowers/plans/task-3-shunt-hardening-report.md`.
