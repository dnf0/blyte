# Task 2 Report: Failing Regression Test for Grandchild Process Tree Termination

**Status: DONE**

## 1. Test Added
Added `grandchild_process_tree_is_killed_at_deadline` to `crates/superstate-agent/tests/test_shell_timeout.rs`:

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

## 2. Test Execution Evidence (Reproduction of Bug)
Ran command:
```bash
cargo test -p superstate-agent --test test_shell_timeout grandchild_process_tree_is_killed_at_deadline -- --nocapture
```

Output:
```text
running 1 test

thread 'grandchild_process_tree_is_killed_at_deadline' (62790836) panicked at crates/superstate-agent/tests/test_shell_timeout.rs:77:5:
must not deadlock on grandchild pipe handles: took 30.08749125s
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test grandchild_process_tree_is_killed_at_deadline ... FAILED

failures:

failures:
    grandchild_process_tree_is_killed_at_deadline

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 5 filtered out; finished in 30.09s
```

## 3. Commit Details
- Commit SHA: `b22b15b`
- Message: `test(agent): add failing test for grandchild process tree termination`
