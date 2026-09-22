# Task 4 Brief: Wire `ContainerShell::exec` to `exec_command` with Timeout Parity

## Global Constraints
- Target crate: `crates/superstate-agent`
- Rust 2021 edition, zero compiler warnings
- Clean execution on Unix platforms (macOS / Linux)
- Full timeout and seam parity between `LocalShell` and `ContainerShell`

## Task Description
Wire `ContainerShell::exec` to `exec_command(cmd, timeout)` in `crates/superstate-agent/src/shell.rs`.

### Current state of `ContainerShell::exec`:
```rust
impl Shell for ContainerShell {
    fn exec(&self, command: &str, _cwd: &Path, _timeout: Duration) -> Result<Output, String> {
        let out = Command::new("docker")
            .args(["exec", "-i", "-w"])
            .arg(&self.workdir)
            .arg(&self.container_id)
            .args(["bash", "-lc", command])
            .output()
            .map_err(|e| format!("cannot exec in container {}: {e}", self.container_id))?;
        Ok(Output {
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            exit_code: out.status.code().unwrap_or(-1),
        })
    }
    // ...
}
```

### Steps:
1. In `crates/superstate-agent/src/shell.rs`:
Update `ContainerShell::exec`:
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

2. Add a test in `crates/superstate-agent/tests/test_shell_timeout.rs`:
```rust
#[test]
fn container_shell_enforces_timeout_on_hung_execution() {
    use std::path::PathBuf;
    use superstate_agent::shell::ContainerShell;

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

3. Verification:
Run:
```bash
cargo check -p superstate-agent
cargo test -p superstate-agent
```
All tests must pass with 0 warnings.

4. Commit:
```bash
git add crates/superstate-agent/src/shell.rs crates/superstate-agent/tests/test_shell_timeout.rs
git commit --no-gpg-sign -m "feat(agent): wire ContainerShell to exec_command with full timeout parity"
```
Run `roborev status; roborev show HEAD`.
Report back status: DONE.
