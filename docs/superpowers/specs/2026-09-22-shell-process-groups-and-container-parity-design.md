# Design Specification: Shell Process Groups & Container Seam Parity

**Status:** Proposed  
**Date:** 2026-09-22  
**Target:** `crates/superstate-agent` (`shell.rs`, `Cargo.toml`, `tests/test_shell_timeout.rs`)  

---

## 1. Executive Summary & Problem Statement

### 1.1 Context & Incident
During an autonomous evaluation run of `superstate-agent`, the model issued:
```bash
which pytest; which python3; find / -name pytest 2>/dev/null
```
The command scanned the entire macOS host filesystem for 3 minutes and 28 seconds before being killed by hand.

While `LocalShell::exec` accepted a `timeout: Duration` parameter, the implementation had two critical latent defects:
1. **Grandchild Orphan Leaks:** `child.kill()` was issued only to the top-level `bash` PID. The subshell/pipeline grandchild `find /` was reparented to `init` and continued running.
2. **Pipe Drain Deadlock:** Because the orphaned `find /` process inherited the write ends of `stdout` and `stderr`, the harness reader threads (`t_out.join()` and `t_err.join()`) **deadlocked waiting for EOF**, defeating the timeout and hanging the harness.

Furthermore, an audit of the `Shell` seam revealed that **`ContainerShell::exec` was an unfinished stub**:
- It prefixed `_timeout: Duration` with an underscore and completely ignored it.
- It used synchronous `Command::output()`. Any hung command inside the container would hang the harness indefinitely.

### 1.2 Objective
1. **Process Group Isolation (`setpgid(0)` + `kill(-pgid, SIGKILL)`):** Ensure all commands spawned by `LocalShell` run in their own process group, and deadline expiry terminates the entire process tree simultaneously, releasing pipe handles and unblocking the harness.
2. **Container Seam Parity:** Wire timeouts and process group management into `ContainerShell::exec` using the exact same robust execution engine.
3. **Evaluation Sandboxing:** Ensure evaluation runs have full container isolation so unguided commands like `find /` are confined to the container root rather than the host.

---

## 2. Architecture & Invariants

### 2.1 Core Invariants
- **Invariant I (No Unbounded Waits):** A command executed through `Shell::exec` must **never** be able to hang the harness process, regardless of whether it runs on the host or inside a container.
- **Invariant II (Process Tree Quiescence):** When a command times out, all descendant processes must be killed. No orphan processes may survive to consume host CPU or keep pipe descriptors open.
- **Invariant III (Seam Parity):** `LocalShell` and `ContainerShell` must exhibit identical timeout semantics:
  - Both enforce the given `timeout: Duration`.
  - Both return exit code `124` on timeout.
  - Both preserve and return whatever partial `stdout` and `stderr` was buffered before termination.
  - Both append `\n[killed by superstate after Xs]` to `stderr`.

```
                  +----------------------------------+
                  |         Agent Turn Loop          |
                  |          (Runner / Gate)         |
                  +-----------------+----------------+
                                    |
                            shell.exec(cmd, timeout)
                                    |
                     +--------------+--------------+
                     |                             |
             LocalShell::exec             ContainerShell::exec
                     |                             |
           bash -lc <cmd>                docker exec ... bash -lc <cmd>
                     \                             /
                      +-------------+-------------+
                                    |
                             exec_command()
                       - Stdio::null() stdin
                       - process_group(0) (Unix)
                       - Pipe drain worker threads
                       - Deadline polling loop
                       - On timeout: kill(-pgid, SIGKILL)
                       - Exit code 124 + partial output
```

---

## 3. Detailed Component Design

### 3.1 Shared Subprocess Execution Engine (`exec_command`)

In `crates/superstate-agent/src/shell.rs`, extract the execution loop into a private helper function:

```rust
fn exec_command(mut cmd: Command, timeout: Duration) -> Result<Output, String>
```

#### Step 1: Process Group & Stdio Configuration
- `cmd.stdin(Stdio::null())`: Prevents commands from hanging on interactive input.
- `cmd.stdout(Stdio::piped())` and `cmd.stderr(Stdio::piped())`: Non-blocking pipe capture.
- `#[cfg(unix)]`: Call `std::os::unix::process::CommandExt::process_group(&mut cmd, 0)`.
  This calls `setpgid(0, 0)` in the child before `exec`, placing the child and all future grandchildren in a distinct process group where `pgid == child.id()`.

#### Step 2: Concurrently Drained Pipes
- Spawn `t_out` and `t_err` threads reading `stdout` and `stderr` into strings.
- Because background reading starts immediately upon spawn, pipe buffer deadlock (typically 64KB OS pipe buffer saturation) is impossible.

#### Step 3: Polling Loop & Process Tree Teardown
- Loop with `child.try_wait()` every 50ms against `deadline = Instant::now() + timeout`.
- If `Instant::now() >= deadline`:
  - `#[cfg(unix)]`:
    ```rust
    let pgid = child.id() as i32;
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
    ```
  - `#[cfg(not(unix))]`:
    ```rust
    let _ = child.kill();
    ```
  - Call `let _ = child.wait();` to reap the parent zombie.
  - The SIGKILL delivered to `-pgid` terminates all grandchildren (e.g. `find /`, subshells, pipeline stages).
  - All write descriptors associated with the pipe are closed immediately.
  - `t_out.join()` and `t_err.join()` unblock cleanly without delay.
  - Return `exit_code: 124` with stderr marker `[killed by superstate after {}s]`.

### 3.2 `LocalShell::exec` Refactoring
`LocalShell::exec` becomes a clean wrapper constructing the local `bash` command:
```rust
impl Shell for LocalShell {
    fn exec(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        let mut cmd = Command::new("bash");
        cmd.arg("-lc").arg(command).current_dir(cwd);
        exec_command(cmd, timeout)
    }
    // ...
}
```

### 3.3 `ContainerShell::exec` Parity Implementation
`ContainerShell::exec` constructs the containerized execution command and delegates to `exec_command`:
```rust
impl Shell for ContainerShell {
    fn exec(&self, command: &str, _cwd: &Path, timeout: Duration) -> Result<Output, String> {
        let mut cmd = Command::new("docker");
        cmd.args(["exec", "-i", "-w"])
            .arg(&self.workdir)
            .arg(&self.container_id)
            .args(["bash", "-lc", command]);
        exec_command(cmd, timeout)
    }
    // ...
}
```

### 3.4 Dependency Updates
Add `libc` to `crates/superstate-agent/Cargo.toml`:
```toml
[target.'cfg(unix)'.dependencies]
libc = "0.2"
```

---

## 4. Verification & Testing Strategy

### 4.1 Unit & Integration Tests in `tests/test_shell_timeout.rs`
1. **Grandchild Process Tree Termination Test:**
   - Execute a command that spawns background / grandchild subshells:
     `bash -lc '(sleep 30) & wait'` or `bash -lc 'sh -c "sleep 30"'`.
   - Bound timeout to 1 second.
   - Verify execution finishes in $< 3$ seconds (proving reader threads did NOT deadlock on surviving grandchild write ends).
   - Verify exit code is `124`.
   - Verify partial output and kill notice.
2. **Container Timeout Verification:**
   - Unit test verifying `ContainerShell::exec` passes through `exec_command` with timeout bounds.

### 4.2 Linter & Typechecks
- `cargo check -p superstate-agent`
- `cargo test -p superstate-agent`
- Clean compilation on macOS (Unix).
