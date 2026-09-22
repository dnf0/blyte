# Task 3 Report: Shared `exec_command` with Process Groups and Negative PGID Termination

**Status: DONE**

## 1. Summary of Changes
- Extracted subprocess execution logic in `crates/superstate-agent/src/shell.rs` into `fn exec_command(mut cmd: Command, timeout: Duration) -> Result<Output, String>`.
- Configured child process on Unix with `cmd.process_group(0)` (`setpgid(0)`).
- On timeout deadline expiry on Unix, invoked `libc::kill(-pgid, libc::SIGKILL)` to signal the entire process group simultaneously.
- Ensured stdout and stderr reader threads join cleanly without deadlocks caused by leaked open pipe handles in background grandchild processes.
- Refactored `LocalShell::exec` to delegate to `exec_command`.

## 2. Verification Evidence
Ran command:
```bash
cargo test -p superstate-agent --test test_shell_timeout
```

Output:
```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.20s
     Running tests/test_shell_timeout.rs (target/debug/deps/test_shell_timeout-835d8b3741828074)

running 6 tests
test a_command_that_reads_stdin_does_not_block_forever ... ok
test a_normal_command_is_unaffected ... ok
test a_command_producing_more_output_than_the_pipe_buffer_does_not_deadlock ... ok
test a_runaway_command_is_killed_at_the_deadline ... ok
test grandchild_process_tree_is_killed_at_deadline ... ok
test output_produced_before_the_kill_is_kept ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s
```

All 6 tests passed in ~2.04 seconds (specifically, `grandchild_process_tree_is_killed_at_deadline` was killed at the 1s timeout and finished well under the 4s threshold without hanging).

Full test suite verification:
```bash
cargo test -p superstate-agent
```
Output:
```text
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s (lib)
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.64s (test_run)
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s (test_shell_timeout)
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s (test_state_authority)
test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.08s (test_tools)
```

Static analysis verification:
- `cargo check -p superstate-agent`: 0 warnings
- `cargo clippy -p superstate-agent`: 0 warnings

## 3. Commit Details
- **Commit SHA:** `5d5809c`
- **Message:** `fix(agent): kill process group on timeout to prevent orphan grandchild leaks`
- **GPG Verification:** Skipped (`--no-gpg-sign`)
