# Task 2 Brief: Failing Regression Test for Grandchild Process Tree Termination

## Global Constraints
- Target crate: `crates/superstate-agent`
- Rust 2021 edition, zero compiler warnings
- Clean execution on Unix platforms (macOS / Linux)

## Task Description
Write a regression test in `crates/superstate-agent/tests/test_shell_timeout.rs` named `grandchild_process_tree_is_killed_at_deadline`.

### Context:
When an agent runs a command that forks grandchild processes (e.g. `sh -c '(sleep 30) & wait'`), killing only the parent PID leaves the grandchild alive. The grandchild inherits the write descriptors of stdout and stderr, causing reader threads to deadlock or wait until the grandchild exits.

### Steps:
1. In `crates/superstate-agent/tests/test_shell_timeout.rs`, add:
```rust
#[test]
fn grandchild_process_tree_is_killed_at_deadline() {
    // If only the parent bash process is killed, background grandchildren
    // (such as `find /` or subshells) stay alive, keep pipe handles open,
    // and cause reader thread joins to hang.
    let d = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let out = LocalShell
        .exec("sh -c '(sleep 30) & wait'", d.path(), Duration::from_secs(1))
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(4),
        "must not deadlock on grandchild pipe handles: took {elapsed:?}"
    );
    assert_eq!(out.exit_code, 124, "expected exit code 124 on timeout");
    assert!(
        out.stderr.contains("killed by superstate"),
        "kill notice missing from stderr: {:?}",
        out.stderr
    );
}
```
2. Run the test:
```bash
cargo test -p superstate-agent --test test_shell_timeout grandchild_process_tree_is_killed_at_deadline -- --nocapture
```
Confirm whether it fails or hangs (demonstrating the bug). If it hangs, kill it after a few seconds or note the timeout.
3. Commit the test:
```bash
git add crates/superstate-agent/tests/test_shell_timeout.rs
git commit --no-gpg-sign -m "test(agent): add failing test for grandchild process tree termination"
```
4. Run `roborev status; roborev show HEAD`.
5. Report back status: DONE (or DONE_WITH_CONCERNS) with the failure reproduction evidence.
