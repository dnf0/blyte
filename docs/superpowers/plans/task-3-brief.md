# Task 3 Brief: Shared `exec_command` with Process Groups and Negative PGID Termination

## Global Constraints
- Target crate: `crates/superstate-agent`
- Rust 2021 edition, zero compiler warnings
- Clean execution on Unix platforms (macOS / Linux)
- Exit code 124 on timeout with `\n[killed by superstate after {}s]` appended to stderr
- Partial output kept

## Task Description
Extract the subprocess execution logic in `crates/superstate-agent/src/shell.rs` into a shared function:
```rust
fn exec_command(mut cmd: Command, timeout: Duration) -> Result<Output, String>
```
configured with:
1. `cmd.stdin(Stdio::null())`
2. `cmd.stdout(Stdio::piped())` and `cmd.stderr(Stdio::piped())`
3. Unix process group:
   ```rust
   #[cfg(unix)]
   {
       use std::os::unix::process::CommandExt;
       cmd.process_group(0);
   }
   ```
4. Background thread drains for stdout and stderr.
5. Deadline loop polling `child.try_wait()` every 50ms.
6. On timeout:
   - On Unix:
     ```rust
     let pgid = child.id() as i32;
     unsafe {
         libc::kill(-pgid, libc::SIGKILL);
     }
     ```
   - On non-Unix:
     ```rust
     let _ = child.kill();
     ```
   - Reap child: `let _ = child.wait();`
7. Reader thread joins (`t_out.join()`, `t_err.join()`).
8. Return `Output` with exit code and stdout/stderr (or exit code 124 and kill notice on timeout).
9. Refactor `LocalShell::exec` to construct the local `bash -lc` Command and delegate to `exec_command(cmd, timeout)`.

### Verification:
Run:
```bash
cargo test -p superstate-agent --test test_shell_timeout
```
All tests including `grandchild_process_tree_is_killed_at_deadline` MUST pass in < 2s.

### Commit:
```bash
git add crates/superstate-agent/src/shell.rs
git commit --no-gpg-sign -m "fix(agent): kill process group on timeout to prevent orphan grandchild leaks"
```
Run `roborev status; roborev show HEAD`.
Report back status: DONE.
