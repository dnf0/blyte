# Shell Process Groups & Container Seam Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Unix process group isolation (`setpgid(0)` + `kill(-pgid, SIGKILL)`) and wire full timeout and execution parity into `ContainerShell`, preventing orphan grandchild leaks and pipe drain deadlocks across all execution environments.

**Architecture:** Extract a shared `exec_command(cmd, timeout)` execution engine in `crates/superstate-agent/src/shell.rs` configured with `process_group(0)` and null stdin. On deadline expiry, terminate the entire process group simultaneously with `libc::kill(-pgid, SIGKILL)`, allowing pipe reader threads to unblock without delay. Delegate both `LocalShell::exec` and `ContainerShell::exec` to this shared engine.

**Tech Stack:** Rust (2021 edition), `std::process::Command`, `std::os::unix::process::CommandExt`, `libc`, `tempfile`.

## Global Constraints
- Target crate: `crates/superstate-agent`
- Rust 2021 edition, zero compiler warnings (`cargo check -p superstate-agent`)
- Preserve existing timeout exit code (124) and stderr notice: `\n[killed by superstate after {}s]`
- Preserve partial output captured prior to termination
- Clean execution on Unix platforms (macOS / Linux)

---

### Task 1: Add `libc` Dependency to `crates/superstate-agent`

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-agent/Cargo.toml:11-17`

**Interfaces:**
- Consumes: none
- Produces: `libc` available on Unix targets for POSIX process group signalling.

- [ ] **Step 1: Add Unix target dependency to `Cargo.toml`**

Edit `crates/superstate-agent/Cargo.toml` to add:
```toml
[target.'cfg(unix)'.dependencies]
libc = "0.2"
```

- [ ] **Step 2: Verify `Cargo.toml` parses and workspace compiles**

Run:
```bash
cargo check -p superstate-agent
```
Expected: PASS with 0 warnings.

- [ ] **Step 3: Commit dependency change**

```bash
git add crates/superstate-agent/Cargo.toml
git commit --no-gpg-sign -m "build: add libc dependency for Unix process group killing"
```

---

### Task 2: Failing Regression Test for Grandchild Process Tree Termination

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-agent/tests/test_shell_timeout.rs`

**Interfaces:**
- Consumes: `LocalShell`, `Shell::exec`
- Produces: Failing regression test proving single PID killing leaves grandchildren running and deadlocks pipe reader threads.

- [ ] **Step 1: Write the failing regression test**

Add `grandchild_process_tree_is_killed_at_deadline` to `crates/superstate-agent/tests/test_shell_timeout.rs`:

```rust
#[test]
fn grandchild_process_tree_is_killed_at_deadline() {
    // If only the parent bash process is killed, background grandchildren
    // (such as `find /` or subshells) stay alive, keep pipe handles open,
    // and cause reader thread joins to hang.
    let d = tempfile::tempdir().unwrap();
    let started = Instant::now();
    // Subshell spawning a grandchild and waiting:
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

- [ ] **Step 2: Run test to confirm it reproduces the defect (fails or hangs)**

Run:
```bash
cargo test -p superstate-agent --test test_shell_timeout grandchild_process_tree_is_killed_at_deadline -- --nocapture
```
Expected: The test will either hang for 30 seconds or take > 4 seconds, failing the assertion.

---

### Task 3: Shared `exec_command` with Process Groups and Negative PGID Termination

- **Depends on:** Task 2

**Files:**
- Modify: `crates/superstate-agent/src/shell.rs:70-130`

**Interfaces:**
- Consumes: `std::os::unix::process::CommandExt`, `libc::kill`, `libc::SIGKILL`
- Produces: `fn exec_command(mut cmd: Command, timeout: Duration) -> Result<Output, String>` shared by all shell implementations.

- [ ] **Step 1: Implement `exec_command` and update `LocalShell::exec`**

In `crates/superstate-agent/src/shell.rs`, extract the shared execution logic into `exec_command`:

```rust
fn exec_command(mut cmd: Command, timeout: Duration) -> Result<Output, String> {
    use std::io::Read;
    use std::process::Stdio;

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd.spawn().map_err(|e| format!("cannot run command: {e}"))?;

    let mut so = child.stdout.take().ok_or("no stdout")?;
    let mut se = child.stderr.take().ok_or("no stderr")?;
    let t_out = std::thread::spawn(move || {
        let mut b = String::new();
        let _ = so.read_to_string(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = String::new();
        let _ = se.read_to_string(&mut b);
        b
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait().map_err(|e| format!("wait failed: {e}"))? {
            Some(s) => break Some(s),
            None if std::time::Instant::now() >= deadline => {
                #[cfg(unix)]
                {
                    let pgid = child.id() as i32;
                    unsafe {
                        libc::kill(-pgid, libc::SIGKILL);
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = child.kill();
                }
                let _ = child.wait();
                break None;
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();

    match status {
        Some(s) => Ok(Output {
            stdout,
            stderr,
            exit_code: s.code().unwrap_or(-1),
        }),
        None => Ok(Output {
            stdout,
            stderr: format!("{stderr}\n[killed by superstate after {}s]", timeout.as_secs()),
            exit_code: 124,
        }),
    }
}
```

Update `LocalShell::exec`:
```rust
impl Shell for LocalShell {
    fn exec(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        let mut cmd = Command::new("bash");
        cmd.arg("-lc").arg(command).current_dir(cwd);
        exec_command(cmd, timeout)
    }

    fn describe(&self) -> String {
        "Commands run directly on this machine, in the workspace directory.".into()
    }
}
```

- [ ] **Step 2: Run tests to verify the regression test now passes**

Run:
```bash
cargo test -p superstate-agent --test test_shell_timeout
```
Expected: All tests pass including `grandchild_process_tree_is_killed_at_deadline` in under 2 seconds.

- [ ] **Step 3: Commit implementation**

```bash
git add crates/superstate-agent/src/shell.rs crates/superstate-agent/tests/test_shell_timeout.rs
git commit --no-gpg-sign -m "fix(agent): kill process group on timeout to prevent orphan grandchild leaks"
```

---

### Task 4: Wire `ContainerShell::exec` to `exec_command` with Timeout Parity

- **Depends on:** Task 3

**Files:**
- Modify: `crates/superstate-agent/src/shell.rs:140-170`
- Modify: `crates/superstate-agent/tests/test_shell_timeout.rs`

**Interfaces:**
- Consumes: `exec_command`
- Produces: `ContainerShell::exec` enforcing timeout bounds, process groups, and partial output parity.

- [ ] **Step 1: Write unit test for `ContainerShell` timeout enforcement**

In `crates/superstate-agent/tests/test_shell_timeout.rs`, add:

```rust
#[test]
fn container_shell_enforces_timeout_on_hung_execution() {
    use std::path::PathBuf;
    use superstate_agent::shell::ContainerShell;

    // Command that invokes a non-existent or deadlocked docker command with a short timeout.
    // If docker is absent, it returns an error immediately. If present, it must be bounded by timeout.
    let shell = ContainerShell {
        container_id: "non-existent-container-id-12345".into(),
        workdir: PathBuf::from("/workspace"),
    };
    let d = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let res = shell.exec("sleep 30", d.path(), Duration::from_millis(500));
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "container exec must obey timeout bound: took {elapsed:?}"
    );
    // Either fails to find container or exits cleanly with timeout/error
    match res {
        Ok(out) => {
            assert!(out.exit_code != 0);
        }
        Err(e) => {
            assert!(!e.is_empty());
        }
    }
}
```

- [ ] **Step 2: Implement `ContainerShell::exec` delegation to `exec_command`**

In `crates/superstate-agent/src/shell.rs`:
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

    fn describe(&self) -> String {
        format!(
            "Commands run inside the project container at {}. Project dependencies are \
             installed there, not on the host.",
            self.workdir.display()
        )
    }
}
```

- [ ] **Step 3: Run all agent test suites**

Run:
```bash
cargo test -p superstate-agent
```
Expected: All tests in `test_run.rs`, `test_shell_timeout.rs`, `test_state_authority.rs`, `test_tools.rs` PASS.

- [ ] **Step 4: Commit container parity changes**

```bash
git add crates/superstate-agent/src/shell.rs crates/superstate-agent/tests/test_shell_timeout.rs
git commit --no-gpg-sign -m "feat(agent): wire ContainerShell to exec_command with full timeout parity"
```

---

## Verification Checklist
- Run `cargo check -p superstate-agent` (0 errors, 0 warnings).
- Run `cargo test -p superstate-agent` (all suites pass).
- Verify `roborev status` and `roborev show HEAD` after each commit.
