# Fast Tools Implementation Plan: persistent sessions, native ripgrep, reused connections

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** Remove the harness's own overhead without changing what the model sees.

**Baseline** (`harness_bench`, 200 turns, #62): the harness's own time is 7.9s locally and **43.2s in a container**. Nearly all of it is process spawning. Each container tool call costs 2–5 `docker exec` (read 191 ms, write 243 ms, edit 299 ms, grep 345 ms, bash 109 ms p50). Locally, `bash` costs 56 ms (a login shell per command) and `grep` 111 ms (two spawns). Context building is under 1 ms and is left alone.

**Design:**
1. **Persistent sessions (`PersistentShell`, `SessionFs`).** One long-lived `bash -l` per role:
   - **Harness session:** runs the harness's own commands (grep, glob, the gate, git), each in a fresh subshell in the directory asked for, so none of them persists anything.
   - **User session:** runs the model's `bash`, so its `cd` and `export` carry over, as in a terminal. This is an intentional behaviour change, stated in the system prompt.
   - **SessionFs:** does container file I/O through a session instead of a `docker exec` per operation.
   - **Protocol:** commands and outputs travel base64-encoded, so no quoting or framing can break. Each command's stdout and stderr go to their own files, so background jobs never block.
   - **Timeouts:** a timeout kills the session and reports what the command wrote, exactly as today (exit 124, the same `[killed by blyte after Ns]` note).
   - **Fallback:** if a session cannot start (for example, no `base64` in the environment), everything falls back to today's process per command.
2. **Native ripgrep in `grep`** for local workspaces, using ripgrep's own libraries (`grep-searcher`, `grep-regex`, `ignore`):
   - it is in-process;
   - it respects `.gitignore` everywhere, with ripgrep's output format and regex semantics;
   - results come in a deterministic order.
   
   In containers, `grep` is one command that uses `rg` when present, instead of a probe plus a search.
3. **`sd` and `tokei` join the accelerators** the model is told about when present. Pretty printers and interactive tools are deliberately excluded.
4. **One HTTP client per process**, so every model call reuses the connection.

**Guardrail:** equivalence tests. Harness commands through a session must return byte-identical `Output` to a fresh `bash -lc` for the same command. Container file I/O through a session must write and read identical bytes. Native `grep` must match `rg`'s line format.

**Tech Stack:** Rust 2021; new dependencies `grep-searcher`, `grep-regex`, `grep-matcher`, `ignore` (all from ripgrep).

## Global Constraints

- Base branch: `feat/fast-tools` (from `main`). Each task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back. The tasks build on each other in order.
- Do not run Docker (no `--image`); Claude runs container benchmarks.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `blyte-mcp`'s `test_blyte_status_and_cancel_lifecycle` and `blyte-cli`'s `test_cli_daemon_detached_forwarding` are known to be timing-flaky: if one alone fails, re-run once and report both results.
- Existing tests must pass unmodified unless a step says otherwise. **If any other struct literal or exhaustive match breaks the build only because of a change in this plan, fix it mechanically and report it.**
- Commit with `--no-gpg-sign` and the exact message in each task.

---

### Task 1: `Session`, `PersistentShell`, `SessionFs`

**Files:**
- Modify: `crates/blyte-agent/src/shell.rs`, `crates/blyte-agent/src/fs.rs`
- Test: `crates/blyte-agent/tests/test_persistent_shell.rs` (create)

- [x] **Step 1: Write the failing tests.** Create `crates/blyte-agent/tests/test_persistent_shell.rs`:

```rust
//! Persistent sessions: the harness's commands match a fresh process exactly, the model's shell
//! keeps its directory and environment, and failures recover.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use blyte_agent::fs::{Fs, LocalFs, SessionFs};
use blyte_agent::shell::{LocalShell, PersistentShell, Shell};

const T: Duration = Duration::from_secs(30);

fn setup() -> (tempfile::TempDir, PathBuf, PersistentShell) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    let p = PersistentShell::local(root.clone());
    (d, root, p)
}

#[test]
fn harness_commands_match_a_fresh_process_exactly() {
    let (_d, root, p) = setup();
    for cmd in [
        "echo hi",
        "printf 'no newline'",
        "echo out; echo err >&2; exit 3",
        "cd / && pwd",
        "cat",
        "seq 1 20000",
        "echo héllo wörld",
        "false",
        "echo $((6*7))",
        "printf 'a\\tb\\n\\n\\n'",
        "for i in 1 2 3; do echo line $i; done",
    ] {
        let old = LocalShell.exec(cmd, &root, T).unwrap();
        let new = p.exec(cmd, &root, T).unwrap();
        assert_eq!(new, old, "{cmd}");
    }
}

#[test]
fn harness_commands_never_persist_state() {
    let (_d, root, p) = setup();
    p.exec("cd sub; export FOO=1", &root, T).unwrap();
    let o = p.exec("pwd; echo ${FOO:-unset}", &root, T).unwrap();
    assert_eq!(o.stdout, format!("{}\nunset\n", root.display()));
}

#[test]
fn the_models_shell_keeps_its_directory_and_environment() {
    let (_d, root, p) = setup();
    p.exec_user("cd sub && export FOO=bar", &root, T).unwrap();
    let o = p.exec_user("pwd; echo $FOO", &root, T).unwrap();
    assert_eq!(o.stdout, format!("{}/sub\nbar\n", root.display()));
    // ...and none of it leaks into the harness's own commands.
    let h = p.exec("pwd; echo ${FOO:-unset}", &root, T).unwrap();
    assert_eq!(h.stdout, format!("{}\nunset\n", root.display()));
}

#[test]
fn a_timeout_kills_the_command_and_keeps_its_output() {
    let (_d, root, p) = setup();
    let o = p
        .exec_user("echo start; sleep 10", &root, Duration::from_secs(1))
        .unwrap();
    assert_eq!(o.exit_code, 124);
    assert_eq!(o.stdout, "start\n");
    assert!(o.stderr.ends_with("[killed by blyte after 1s]"), "{:?}", o.stderr);
    assert_eq!(p.exec_user("echo alive", &root, T).unwrap().stdout, "alive\n");
}

#[test]
fn exit_ends_the_shell_and_the_next_command_gets_a_fresh_one() {
    let (_d, root, p) = setup();
    p.exec_user("cd sub", &root, T).unwrap();
    let o = p.exec_user("echo bye; exit 7", &root, T).unwrap();
    assert_eq!(o.stdout, "bye\n");
    assert!(o.stderr.contains("the shell exited"), "{:?}", o.stderr);
    let fresh = p.exec_user("pwd", &root, T).unwrap();
    assert_eq!(fresh.stdout, format!("{}\n", root.display()));
}

#[test]
fn a_background_job_does_not_hold_the_command() {
    let (_d, root, p) = setup();
    let t = Instant::now();
    let o = p.exec_user("sleep 5 & echo started", &root, T).unwrap();
    assert_eq!(o.stdout, "started\n");
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
}

#[test]
fn it_is_faster_than_a_process_per_command() {
    let (_d, root, p) = setup();
    p.exec("true", &root, T).unwrap(); // start the session
    let t = Instant::now();
    for _ in 0..30 {
        LocalShell.exec("true", &root, T).unwrap();
    }
    let old = t.elapsed();
    let t = Instant::now();
    for _ in 0..30 {
        p.exec("true", &root, T).unwrap();
    }
    let new = t.elapsed();
    assert!(new < old, "session {new:?} vs process-per-command {old:?}");
}

#[test]
fn session_files_write_and_read_exact_bytes() {
    let (_d, root, _p) = setup();
    let fs = SessionFs::local(root.clone());
    let content = "quotes ' \" `ticks` $HOME \\ back\nünïcode ✓\nno trailing newline";
    let path = root.join("deep/dir/f.txt");
    fs.write(&path, content).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    assert_eq!(fs.read(&path).unwrap(), LocalFs.read(&path).unwrap());
    std::os::unix::fs::symlink(&path, root.join("link")).unwrap();
    assert_eq!(
        fs.canonicalize(&root.join("link")).unwrap(),
        LocalFs.canonicalize(&root.join("link")).unwrap()
    );
    assert!(fs.read(&root.join("missing.txt")).is_err());
    let big = "x".repeat(200_000);
    fs.write(&root.join("big.txt"), &big).unwrap();
    assert_eq!(fs.read(&root.join("big.txt")).unwrap(), big);
}
```

- [x] **Step 2: Run to confirm they fail.** Run: `cargo test -p blyte-agent --test test_persistent_shell`. Expected: compile errors, because `PersistentShell`, `exec_user` and `SessionFs` do not exist.

- [x] **Step 3: The session machinery.** In `crates/blyte-agent/src/shell.rs`:
  1. Change the imports to:

```rust
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};
```

     (Remove any now-duplicated `use std::io::Read;` or `use std::process::Stdio;` inside `exec_command` only if clippy reports them as unused. Keep them otherwise.)
  2. In `pub trait Shell`, after `fn exec(...)`, add:

```rust
    /// Run a command the **model** issued. Where the executor keeps a persistent session, its
    /// `cd` and `export` carry over to the model's next command, as in a terminal. The default
    /// is `exec`: a fresh process in `cwd`.
    fn exec_user(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        self.exec(command, cwd, timeout)
    }
```

  3. At the end of the file, add:

```rust
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding: how commands and outputs cross a session, so no quoting or
/// framing can break the protocol.
pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Decode standard base64, ignoring whitespace. `None` if malformed.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for q in bytes.chunks(4) {
        let pad = q.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 {
            return None;
        }
        let mut n = 0u32;
        for (i, &c) in q.iter().enumerate() {
            let v = if i >= 4 - pad { 0 } else { val(c)? };
            n = (n << 6) | v;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// Single-quote for a POSIX shell.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// How to start a session's shell: a login shell, as `bash -lc` gives today.
#[derive(Debug, Clone)]
pub enum Launcher {
    /// `bash -l` on this machine, starting in `root`.
    Local { root: PathBuf },
    /// `bash -l` inside a running container, starting in `workdir`.
    Container {
        container_id: String,
        workdir: PathBuf,
    },
}

impl Launcher {
    fn command(&self) -> Command {
        match self {
            Launcher::Local { root } => {
                let mut c = Command::new("bash");
                c.arg("-l").current_dir(root);
                c
            }
            Launcher::Container {
                container_id,
                workdir,
            } => {
                let mut c = Command::new("docker");
                c.args(["exec", "-i", "-w"])
                    .arg(workdir)
                    .arg(container_id)
                    .args(["bash", "-l"]);
                c
            }
        }
    }

    /// The process-per-command executor this launcher replaces, used whenever a session
    /// cannot start.
    fn fallback(&self) -> Box<dyn Shell> {
        match self {
            Launcher::Local { .. } => Box::new(LocalShell),
            Launcher::Container {
                container_id,
                workdir,
            } => Box::new(ContainerShell {
                container_id: container_id.clone(),
                workdir: workdir.clone(),
            }),
        }
    }
}

/// How a session command ended.
pub enum RunResult {
    Done(Output),
    /// The deadline passed; the session must be killed. Paths hold what the command wrote.
    TimedOut { out: String, err: String },
    /// The shell ended before reporting (e.g. the command ran `exit`).
    Died { out: String, err: String },
}

/// One long-lived `bash -l`. Each command's stdout and stderr go to their own files and come
/// back base64-encoded on one marker line, so neither quoting nor background jobs can break
/// the framing.
pub struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    tmp: String,
    next: u64,
}

const READY: &str = "__BLYTE_READY__ ";

impl Session {
    pub fn start(launcher: &Launcher) -> Result<Session, String> {
        let mut cmd = launcher.command();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot start a shell session: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut s = Session {
            child,
            stdin,
            lines: rx,
            tmp: String::new(),
            next: 0,
        };
        s.send(
            "__blyte_t=$(mktemp -d 2>/dev/null || { mkdir -p /tmp/blyte.$$ && echo /tmp/blyte.$$; }); \
             if command -v base64 >/dev/null 2>&1; then echo \"__BLYTE_READY__ $__blyte_t\"; \
             else echo __BLYTE_NO_BASE64__; fi\n",
        )?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match s
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(l) if l.starts_with(READY) => {
                    s.tmp = l[READY.len()..].trim().to_string();
                    return Ok(s);
                }
                Ok(l) if l.starts_with("__BLYTE_NO_BASE64__") => {
                    s.kill();
                    return Err("no base64 in the execution environment".into());
                }
                Ok(_) => continue, // profile output on stdout
                Err(_) => {
                    s.kill();
                    return Err("the shell session did not start".into());
                }
            }
        }
    }

    fn send(&mut self, script: &str) -> Result<(), String> {
        self.stdin
            .write_all(script.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|e| format!("the shell session closed: {e}"))
    }

    /// Kill the session and everything it started.
    pub fn kill(&mut self) {
        #[cfg(unix)]
        {
            let pgid = self.child.id() as i32;
            if pgid > 0 {
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Run `command` to completion. `cwd: Some(dir)` runs it in a subshell in `dir`, so nothing
    /// it does persists; `None` runs it in the session itself, so `cd` and `export` carry over.
    pub fn run(&mut self, command: &str, cwd: Option<&Path>, timeout: Duration) -> RunResult {
        let id = self.next;
        self.next += 1;
        let (out, err) = (format!("{}/{id}.o", self.tmp), format!("{}/{id}.e", self.tmp));
        let decoded = format!(
            "\"$(printf %s {} | base64 -d)\"",
            b64_encode(command.as_bytes())
        );
        let body = match cwd {
            Some(d) => format!("( cd {} && eval {decoded} )", quote(&d.to_string_lossy())),
            None => format!("eval {decoded}"),
        };
        let (qo, qe) = (quote(&out), quote(&err));
        let script = format!(
            "{body} </dev/null >{qo} 2>{qe}; __blyte_r=$?; \
             printf '__BLYTE_{id}__ %s %s %s\\n' \"$__blyte_r\" \
             \"$(base64 < {qo} | tr -d '\\n')\" \"$(base64 < {qe} | tr -d '\\n')\"; \
             rm -f {qo} {qe}\n"
        );
        if self.send(&script).is_err() {
            return RunResult::Died { out, err };
        }
        let marker = format!("__BLYTE_{id}__ ");
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(l) => {
                    let Some(rest) = l.strip_prefix(&marker) else {
                        continue;
                    };
                    let mut f = rest.splitn(3, ' ');
                    let exit_code = f.next().and_then(|x| x.trim().parse().ok()).unwrap_or(-1);
                    let dec = |x: Option<&str>| {
                        x.and_then(b64_decode)
                            .map(|b| String::from_utf8_lossy(&b).into_owned())
                            .unwrap_or_default()
                    };
                    let stdout = dec(f.next());
                    let stderr = dec(f.next());
                    return RunResult::Done(Output {
                        stdout,
                        stderr,
                        exit_code,
                    });
                }
                Err(RecvTimeoutError::Timeout) => return RunResult::TimedOut { out, err },
                Err(RecvTimeoutError::Disconnected) => return RunResult::Died { out, err },
            }
        }
    }

    /// Remove the session's scratch directory, then end it.
    fn close(&mut self) {
        let _ = self.send(&format!("rm -rf {}; exit 0\n", quote(&self.tmp)));
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.kill();
    }
}

/// Commands through persistent sessions instead of a process each: one session for the
/// harness's own commands (each in a fresh subshell in the directory asked for, never affected
/// by the model's) and one for the model's `bash` (its `cd` and `export` carry over). Falls back
/// to a process per command, exactly as [`LocalShell`] / [`ContainerShell`], when a session
/// cannot start.
///
/// A `docker exec` per call cost 60-110 ms and a container `edit` about 300 ms; a session round
/// trip costs a few milliseconds (harness_bench, 2026-09-24).
pub struct PersistentShell {
    launcher: Launcher,
    harness: Mutex<Option<Session>>,
    user: Mutex<Option<Session>>,
    fallback: Box<dyn Shell>,
}

impl PersistentShell {
    pub fn new(launcher: Launcher) -> Self {
        let fallback = launcher.fallback();
        Self {
            launcher,
            harness: Mutex::new(None),
            user: Mutex::new(None),
            fallback,
        }
    }

    pub fn local(root: PathBuf) -> Self {
        Self::new(Launcher::Local { root })
    }

    pub fn container(container_id: String, workdir: PathBuf) -> Self {
        Self::new(Launcher::Container {
            container_id,
            workdir,
        })
    }

    /// Whether the harness session is (or can be) running. False means every call falls back.
    pub fn is_persistent(&self) -> bool {
        let mut g = self.harness.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Session::start(&self.launcher).ok();
        }
        g.is_some()
    }

    /// Run through `slot`'s session, starting it if needed. `None` when no session can start.
    fn run(
        &self,
        slot: &Mutex<Option<Session>>,
        command: &str,
        cwd: Option<&Path>,
        timeout: Duration,
    ) -> Option<Result<Output, String>> {
        let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Session::start(&self.launcher).ok();
        }
        let session = g.as_mut()?;
        let (out, err, note, code) = match session.run(command, cwd, timeout) {
            RunResult::Done(o) => return Some(Ok(o)),
            RunResult::TimedOut { out, err } => (
                out,
                err,
                format!("\n[killed by blyte after {}s]", timeout.as_secs()),
                124,
            ),
            RunResult::Died { out, err } => (
                out,
                err,
                "[the shell exited; the next command starts a fresh shell in the workspace root]"
                    .to_string(),
                1,
            ),
        };
        session.kill();
        *g = None;
        drop(g);
        // What the command wrote before it was cut short, read through a fresh process.
        let cat = |p: &str| {
            self.fallback
                .exec(
                    &format!("cat -- {q} 2>/dev/null; rm -f -- {q}", q = quote(p)),
                    Path::new("/"),
                    Duration::from_secs(30),
                )
                .map(|o| o.stdout)
                .unwrap_or_default()
        };
        let (stdout, stderr) = (cat(&out), cat(&err));
        Some(Ok(Output {
            stdout,
            stderr: format!("{stderr}{note}"),
            exit_code: code,
        }))
    }
}

impl Shell for PersistentShell {
    fn exec(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        self.run(&self.harness, command, Some(cwd), timeout)
            .unwrap_or_else(|| self.fallback.exec(command, cwd, timeout))
    }

    fn exec_user(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        self.run(&self.user, command, None, timeout)
            .unwrap_or_else(|| self.fallback.exec(command, cwd, timeout))
    }

    fn describe(&self) -> String {
        format!(
            "{} Your `bash` commands share one shell session: `cd` and `export` carry over to \
             later commands. File tools always resolve paths from the workspace root.",
            self.fallback.describe()
        )
    }
}

impl Drop for PersistentShell {
    fn drop(&mut self) {
        for slot in [&self.harness, &self.user] {
            if let Some(mut s) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                s.close();
            }
        }
    }
}
```

- [x] **Step 4: `SessionFs`.** In `crates/blyte-agent/src/fs.rs`, add at the end:

```rust
/// Files reached through a persistent session: the container counterpart of [`LocalFs`] at a
/// round trip of a few milliseconds, instead of 2-5 `docker exec` per operation (a container
/// `edit` cost about 300 ms). Content crosses base64-encoded, so no byte is interpreted on the
/// way. Falls back to [`ContainerFs`] (or [`LocalFs`] for a local launcher) when no session can
/// start. Error messages match [`ContainerFs`].
pub struct SessionFs {
    shell: crate::shell::PersistentShell,
    fallback: Box<dyn Fs>,
}

impl SessionFs {
    pub fn container(container_id: String, workdir: PathBuf) -> Self {
        Self {
            shell: crate::shell::PersistentShell::container(container_id.clone(), workdir),
            fallback: Box::new(ContainerFs { container_id }),
        }
    }

    /// A session on this machine: for tests, and for executors without a container.
    pub fn local(root: PathBuf) -> Self {
        Self {
            shell: crate::shell::PersistentShell::local(root),
            fallback: Box::new(LocalFs),
        }
    }

    fn run(&self, command: &str) -> Option<crate::shell::Output> {
        use crate::shell::Shell;
        if !self.shell.is_persistent() {
            return None;
        }
        self.shell
            .exec(command, Path::new("/"), std::time::Duration::from_secs(120))
            .ok()
    }
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

impl Fs for SessionFs {
    fn read(&self, path: &Path) -> Result<String, String> {
        let p = path.to_string_lossy().to_string();
        match self.run(&format!("cat -- {}", sh_quote(&p))) {
            Some(o) if o.exit_code == 0 => Ok(o.stdout),
            Some(_) => Err(format!("cannot read {p} in container")),
            None => self.fallback.read(path),
        }
    }

    fn write(&self, path: &Path, content: &str) -> Result<(), String> {
        let p = path.to_string_lossy().to_string();
        let dir = path
            .parent()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();
        let mkdir = if dir.is_empty() {
            String::new()
        } else {
            format!("mkdir -p -- {} && ", sh_quote(&dir))
        };
        let cmd = format!(
            "{mkdir}printf %s {} | base64 -d > {}",
            crate::shell::b64_encode(content.as_bytes()),
            sh_quote(&p)
        );
        match self.run(&cmd) {
            Some(o) if o.exit_code == 0 => Ok(()),
            Some(_) => Err(format!("cannot write {p} in container")),
            None => self.fallback.write(path, content),
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, String> {
        let p = path.to_string_lossy().to_string();
        match self.run(&format!("readlink -f -- {}", sh_quote(&p))) {
            Some(o) if o.exit_code == 0 && !o.stdout.trim().is_empty() => {
                Ok(PathBuf::from(o.stdout.trim()))
            }
            Some(_) => Err(format!("cannot resolve {p} in container")),
            None => self.fallback.canonicalize(path),
        }
    }
}
```

- [x] **Step 5: Run the tests.** Run: `cargo test -p blyte-agent --test test_persistent_shell`. Expected: 8 passed. If `harness_commands_match_a_fresh_process_exactly` fails on a specific command, report the command and both outputs verbatim and stop. Do not change the test.

- [x] **Step 6: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [x] **Step 7: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(agent): persistent shell sessions and session-backed container files"
```

---

### Task 2: Native ripgrep in `grep`; `sd` and `tokei` accelerators

**Files:**
- Modify: `Cargo.toml` (workspace dependencies), `crates/blyte-agent/Cargo.toml`, `crates/blyte-agent/src/lib.rs`, `crates/blyte-agent/src/fs.rs`, `crates/blyte-agent/src/tools.rs`
- Create: `crates/blyte-agent/src/search.rs`
- Test: `crates/blyte-agent/tests/test_native_grep.rs` (create)

- [x] **Step 1: Write the failing tests.** Create `crates/blyte-agent/tests/test_native_grep.rs`:

```rust
//! `grep` on a local workspace runs ripgrep's own libraries in-process: rg's line format and
//! regex semantics, `.gitignore` respected, in a deterministic order, with no process spawned.

use std::path::Path;

use serde_json::json;
use blyte_agent::fs::LocalFs;
use blyte_agent::search::grep_lines;
use blyte_agent::shell::{Output, Shell};
use blyte_agent::tools::execute;

/// Fails the test if anything tries to spawn a command.
struct NoShell;
impl Shell for NoShell {
    fn exec(&self, command: &str, _: &Path, _: std::time::Duration) -> Result<Output, String> {
        panic!("grep spawned a command: {command}");
    }
    fn describe(&self) -> String {
        String::new()
    }
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::create_dir_all(r.join("src/deep")).unwrap();
    std::fs::create_dir_all(r.join("node_modules/pkg")).unwrap();
    std::fs::create_dir_all(r.join(".git")).unwrap();
    std::fs::write(r.join(".gitignore"), "node_modules/\n").unwrap();
    std::fs::write(r.join("src/a.rs"), "fn alpha() {}\nlet x = 1;\nfn beta() {}\n").unwrap();
    std::fs::write(r.join("src/deep/b.rs"), "fn gamma() {}\n").unwrap();
    std::fs::write(r.join("src/c.txt"), "fn not_rust\n").unwrap();
    std::fs::write(r.join("node_modules/pkg/i.js"), "fn ignored() {}\n").unwrap();
    d
}

#[test]
fn matches_rg_format_and_respects_gitignore() {
    let d = repo();
    let lines = grep_lines(d.path(), ".", "fn [a-z]+\\(", "", 200).unwrap();
    assert_eq!(
        lines,
        vec![
            "./src/a.rs:1:fn alpha() {}",
            "./src/a.rs:3:fn beta() {}",
            "./src/deep/b.rs:1:fn gamma() {}",
        ]
    );
}

#[test]
fn a_sub_path_prefixes_as_given_and_a_single_file_has_no_prefix() {
    let d = repo();
    assert_eq!(
        grep_lines(d.path(), "src/deep", "gamma", "", 200).unwrap(),
        vec!["src/deep/b.rs:1:fn gamma() {}"]
    );
    assert_eq!(
        grep_lines(d.path(), "src/a.rs", "beta", "", 200).unwrap(),
        vec!["3:fn beta() {}"]
    );
}

#[test]
fn a_glob_filters_like_rg_and_matches_are_capped() {
    let d = repo();
    let only_txt = grep_lines(d.path(), ".", "fn", "*.txt", 200).unwrap();
    assert_eq!(only_txt, vec!["./src/c.txt:1:fn not_rust"]);
    assert_eq!(grep_lines(d.path(), ".", "fn", "", 2).unwrap().len(), 2);
    assert!(grep_lines(d.path(), ".", "(unclosed", "", 200).is_err());
}

#[test]
fn the_grep_tool_uses_it_on_a_local_workspace_without_spawning() {
    let d = repo();
    let o = execute(
        d.path(),
        &NoShell,
        &LocalFs,
        "grep",
        &json!({"pattern": "gamma"}),
    );
    assert!(!o.is_error, "{}", o.content);
    assert_eq!(o.content, "1 match(es):\n./src/deep/b.rs:1:fn gamma() {}");
    let none = execute(d.path(), &NoShell, &LocalFs, "grep", &json!({"pattern": "zzz"}));
    assert_eq!(none.content, "no matches for zzz");
}

#[test]
fn sd_and_tokei_are_accelerators_and_pretty_printers_are_not() {
    use blyte_agent::tools::ACCELERATORS;
    for t in ["rg", "fd", "sd", "tokei"] {
        assert!(ACCELERATORS.contains(&t), "{t}");
    }
    for t in ["bat", "eza", "dust", "delta"] {
        assert!(!ACCELERATORS.contains(&t), "{t}");
    }
}
```

- [x] **Step 2: Run to confirm they fail.** Run: `cargo test -p blyte-agent --test test_native_grep`. Expected: compile errors, because `blyte_agent::search` does not exist.

- [x] **Step 3: Dependencies.** In the root `Cargo.toml` `[workspace.dependencies]`, add:

```toml
grep-searcher = "0.1.17"
grep-regex = "0.1.14"
grep-matcher = "0.1.9"
ignore = "0.4.33"
```

  In `crates/blyte-agent/Cargo.toml` `[dependencies]`, add `grep-searcher.workspace = true`, `grep-regex.workspace = true`, `grep-matcher.workspace = true` and `ignore.workspace = true`.

- [x] **Step 4: The search module.** Create `crates/blyte-agent/src/search.rs`:

```rust
//! `grep` in-process, with ripgrep's own libraries: rg's regex engine and semantics, `.gitignore`
//! respected (in a git repository, as rg does), binary files skipped, and rg's line format
//! (`path:line:text` with the path as searched, or `line:text` for a single file). Results are
//! sorted by path, so the same search always returns the same text. No process is spawned: two
//! spawns per grep cost about 110 ms locally and 345 ms in a container (harness_bench).

use std::path::Path;

use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;

/// Matching lines for `pattern` under `root.join(sub)`, at most `max`, filtered by `glob` (rg
/// `--glob` semantics) when non-empty. `Err` for an invalid pattern or glob.
pub fn grep_lines(
    root: &Path,
    sub: &str,
    pattern: &str,
    glob: &str,
    max: usize,
) -> Result<Vec<String>, String> {
    let matcher = RegexMatcher::new_line_matcher(pattern)
        .map_err(|e| format!("invalid pattern: {e}"))?;
    let base = root.join(sub);
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .build();
    let mut out = Vec::new();

    if base.is_file() {
        let _ = searcher.search_path(
            &matcher,
            &base,
            UTF8(|n, line| {
                out.push(format!("{n}:{}", line.trim_end_matches(['\n', '\r'])));
                Ok(out.len() < max)
            }),
        );
        return Ok(out);
    }

    let mut walk = WalkBuilder::new(&base);
    if !glob.is_empty() {
        let mut ob = OverrideBuilder::new(&base);
        ob.add(glob).map_err(|e| format!("invalid glob: {e}"))?;
        walk.overrides(ob.build().map_err(|e| format!("invalid glob: {e}"))?);
    }
    walk.sort_by_file_path(|a, b| a.cmp(b));
    for entry in walk.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = entry.path().strip_prefix(&base).unwrap_or(entry.path());
        let shown = if sub == "." {
            format!("./{}", rel.display())
        } else {
            format!("{}/{}", sub.trim_end_matches('/'), rel.display())
        };
        let _ = searcher.search_path(
            &matcher,
            entry.path(),
            UTF8(|n, line| {
                out.push(format!("{shown}:{n}:{}", line.trim_end_matches(['\n', '\r'])));
                Ok(out.len() < max)
            }),
        );
        if out.len() >= max {
            break;
        }
    }
    Ok(out)
}
```

  Add `pub mod search;` to `crates/blyte-agent/src/lib.rs` (keep the list alphabetical).

- [x] **Step 5: `Fs::is_local`.** In `crates/blyte-agent/src/fs.rs`, in `pub trait Fs`, add:

```rust
    /// Whether paths name files on this machine, so tools may read them in-process.
    fn is_local(&self) -> bool {
        false
    }
```

  and in `impl Fs for LocalFs`, add `fn is_local(&self) -> bool { true }`.

- [x] **Step 6: The `grep` tool.** In `crates/blyte-agent/src/tools.rs`:
  1. Change `ACCELERATORS` to `&["rg", "fd", "sd", "tokei", "jq", "ast-grep"]`. Extend its doc comment with: `Pretty printers (bat, eza, dust, delta) and interactive tools are deliberately absent: their colour, icons and pagers cost a model tokens it cannot use.`
  2. In the `"grep" =>` arm, replace everything from `let cmd = grep_command(have(shell, root, "rg"), &pattern, &sub, &glob_str);` through the end of the `match shell.exec(&cmd, root, COMMAND_TIMEOUT) { ... }` expression with:

```rust
            // A local workspace is searched in-process with ripgrep's libraries; elsewhere one
            // command, using rg when the environment has it (no separate probe).
            let lines: Vec<String> = if fs.is_local() {
                match crate::search::grep_lines(root, &sub, &pattern, &glob_str, MAX_MATCHES + 1)
                {
                    Ok(l) => l,
                    Err(e) => return ToolOutcome::err(format!("grep failed: {e}")),
                }
            } else {
                let cmd = format!(
                    "if command -v rg >/dev/null 2>&1; then {}; else {}; fi",
                    grep_command(true, &pattern, &sub, &glob_str),
                    grep_command(false, &pattern, &sub, &glob_str)
                );
                match shell.exec(&cmd, root, COMMAND_TIMEOUT) {
                    Ok(o) => o
                        .stdout
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(String::from)
                        .collect(),
                    Err(e) => return ToolOutcome::err(format!("grep failed: {e}")),
                }
            };
            if lines.is_empty() {
                return ToolOutcome::ok(format!("no matches for {pattern}"));
            }
            if lines.len() > MAX_MATCHES {
                ToolOutcome::ok(format!(
                    "{}\n\n[capped at {MAX_MATCHES} matches; narrow the pattern or pass `glob` \
                     to see the rest]",
                    lines[..MAX_MATCHES].join("\n")
                ))
            } else {
                ToolOutcome::ok(format!("{} match(es):\n{}", lines.len(), lines.join("\n")))
            }
```

  3. If `have` becomes unused and clippy reports dead code, keep it `pub` (it is public API used by tests); do not delete it.

- [x] **Step 7: Run the tests.** Run: `cargo test -p blyte-agent --test test_native_grep --test test_bound_output --test test_tools`. Expected: all pass (5 new). If an existing grep test fails because it asserted the old two-exec behaviour or rg's unsorted order, report it verbatim and stop.

- [x] **Step 8: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed.

- [x] **Step 9: Commit.**

```bash
git add Cargo.toml Cargo.lock crates/
git commit --no-gpg-sign -m "feat(agent): native ripgrep grep for local workspaces; sd and tokei accelerators"
```

---

### Task 3: Wiring, the benchmark, and one HTTP client

**Files:**
- Modify: `crates/blyte-agent/src/tools.rs`, `crates/blyte-cli/src/task_runner.rs`, `crates/blyte-agent/examples/harness_bench.rs`, `crates/blyte-provider/src/lib.rs`, `crates/blyte-provider/src/gemini.rs`, `crates/blyte-provider/src/anthropic.rs`, `crates/blyte-provider/src/ollama.rs`
- Test: `crates/blyte-agent/tests/test_bash_session_routing.rs` (create), `crates/blyte-provider/tests/test_http_client.rs` (create)

- [x] **Step 1: Write the failing tests.**

  Create `crates/blyte-agent/tests/test_bash_session_routing.rs`:

```rust
//! The model's `bash` goes through `exec_user` (its session); the harness's commands through
//! `exec`.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use blyte_agent::fs::LocalFs;
use blyte_agent::shell::{Output, Shell};
use blyte_agent::tools::execute;

#[derive(Default)]
struct Recorder(Mutex<Vec<&'static str>>);
impl Shell for Recorder {
    fn exec(&self, _: &str, _: &Path, _: Duration) -> Result<Output, String> {
        self.0.lock().unwrap().push("exec");
        Ok(Output { stdout: String::new(), stderr: String::new(), exit_code: 0 })
    }
    fn exec_user(&self, _: &str, _: &Path, _: Duration) -> Result<Output, String> {
        self.0.lock().unwrap().push("exec_user");
        Ok(Output { stdout: "ok\n".into(), stderr: String::new(), exit_code: 0 })
    }
    fn describe(&self) -> String {
        String::new()
    }
}

#[test]
fn the_models_bash_uses_its_session() {
    let d = tempfile::tempdir().unwrap();
    let r = Recorder::default();
    execute(d.path(), &r, &LocalFs, "bash", &json!({"command": "cd src"}));
    assert_eq!(*r.0.lock().unwrap(), vec!["exec_user"]);
}
```

  Create `crates/blyte-provider/tests/test_http_client.rs`:

```rust
//! One HTTP client per process, so model calls reuse connections instead of a TCP and TLS
//! handshake each (15-45 ms per call).

#[test]
fn the_http_client_is_shared() {
    let a = blyte_provider::http_client() as *const ureq::Agent;
    let b = blyte_provider::http_client() as *const ureq::Agent;
    assert_eq!(a, b);
}
```

  If `ureq` is not a dev-dependency of `blyte-provider`, it is a normal dependency, so the test can name `ureq::Agent`. If the compiler says otherwise, add `ureq` to `[dev-dependencies]` with the same version spec as `[dependencies]`.

- [x] **Step 2: Run to confirm they fail.** Run: `cargo test -p blyte-agent --test test_bash_session_routing` and `cargo test -p blyte-provider --test test_http_client`. Expected: the routing test fails (it records `exec`, because the tool still calls `exec`), and the HTTP test does not compile (`http_client` does not exist).

- [x] **Step 3: Route the model's bash.** In `crates/blyte-agent/src/tools.rs`, in the `"bash" =>` arm, change `shell.exec(&s("command"), root, COMMAND_TIMEOUT)` to `shell.exec_user(&s("command"), root, COMMAND_TIMEOUT)`.

- [x] **Step 4: One HTTP client.** In `crates/blyte-provider/src/lib.rs`, add:

```rust
/// The process's one HTTP client. `ureq::Agent` pools connections, so every model call after
/// the first reuses one instead of paying a TCP and TLS handshake (15-45 ms) each time.
pub fn http_client() -> &'static ureq::Agent {
    static CLIENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(20))
            .timeout_read(std::time::Duration::from_secs(300))
            .build()
    })
}
```

  In each of `gemini.rs`, `anthropic.rs` and `ollama.rs`, in both `complete` and `list_models`, replace the `let agent = ureq::AgentBuilder::new() ... .build();` statement with `let agent = crate::http_client();`. The timeouts are the same values those files' `CONNECT_TIMEOUT_SECS` (20) and `READ_TIMEOUT_SECS` (300) give; if a file's constants differ from 20 and 300, stop and report it. Remove constants that become unused only if clippy reports them.

- [x] **Step 5: Wire the executors.** In `crates/blyte-cli/src/task_runner.rs`, replace the construction of `local_shell`, `local_fs`, `container_shell`, `container_fs` and the `let (shell, fs, root) = match ...` statement with:

```rust
    // Persistent sessions: one for the harness's commands, one for the model's `bash` (its `cd`
    // and `export` carry over). Container files go through a session too. Each falls back to a
    // process per command if a session cannot start.
    let local_fs = blyte_agent::fs::LocalFs;
    let (persistent, session_fs, root) = match &config.container {
        Some(cid) => (
            blyte_agent::shell::PersistentShell::container(
                cid.clone(),
                PathBuf::from(&config.container_workdir),
            ),
            Some(blyte_agent::fs::SessionFs::container(
                cid.clone(),
                PathBuf::from(&config.container_workdir),
            )),
            PathBuf::from(&config.container_workdir),
        ),
        None => (
            blyte_agent::shell::PersistentShell::local(config.root.clone()),
            None,
            config.root.clone(),
        ),
    };
    let shell: &dyn blyte_agent::shell::Shell = &persistent;
    let fs: &dyn blyte_agent::fs::Fs = match &session_fs {
        Some(f) => f,
        None => &local_fs,
    };
```

  Keep every later use of `shell`, `fs` and `root` as it is.

- [x] **Step 6: The benchmark measures the new executors, with `--legacy` for the old.** In `crates/blyte-agent/examples/harness_bench.rs`:
  1. Add `let legacy = args.iter().any(|a| a == "--legacy");` after `let out = ...;`, and add `[--legacy]` to the usage line in the module doc.
  2. Replace the block from `let (local_shell, local_fs) = (LocalShell, LocalFs);` through the `let (shell, fs, root): (...) = match ... ;` statement with:

```rust
    let (local_shell, local_fs) = (LocalShell, LocalFs);
    let legacy_shell = container.as_ref().map(|id| ContainerShell {
        container_id: id.clone(),
        workdir: PathBuf::from("/work"),
    });
    let legacy_fs = container.as_ref().map(|id| ContainerFs {
        container_id: id.clone(),
    });
    let persistent = match &container {
        Some(id) => PersistentShell::container(id.clone(), PathBuf::from("/work")),
        None => PersistentShell::local(fixture.clone()),
    };
    let session_fs = container
        .as_ref()
        .map(|id| SessionFs::container(id.clone(), PathBuf::from("/work")));
    let root = if container.is_some() {
        PathBuf::from("/work")
    } else {
        fixture.clone()
    };
    let shell: &dyn Shell = match (legacy, &legacy_shell) {
        (true, Some(s)) => s,
        (true, None) => &local_shell,
        (false, _) => &persistent,
    };
    let fs: &dyn Fs = match (legacy, &legacy_fs, &session_fs) {
        (true, Some(f), _) => f,
        (false, _, Some(f)) => f,
        _ => &local_fs,
    };
```

  3. Update the imports: add `PersistentShell` to the `blyte_agent::shell::{...}` import and `SessionFs` to the `blyte_agent::fs::{...}` import.
  4. Make the printed and JSON `mode` include `legacy` when set: `let mode = format!("{}{}", image.map_or("local".to_string(), |i| format!("container:{i}")), if legacy { " legacy" } else { "" });` (replacing the existing `let mode = ...;`). Also move the `docker rm -f` of the container to after `drop` of the executors, by wrapping the run in a block, so sessions close before the container is removed. If that restructuring is awkward, leave the order and report it.

- [x] **Step 7: Run the tests and the local benchmark.** Run `cargo test -p blyte-agent --test test_bash_session_routing --test test_persistent_shell --test test_native_grep` and `cargo test -p blyte-provider --test test_http_client`. Expected: all pass. Then run `cargo run --release -p blyte-agent --example harness_bench -- --turns 200 --legacy` and `cargo run --release -p blyte-agent --example harness_bench -- --turns 200`, and report both outputs verbatim.

- [x] **Step 8: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed.

- [x] **Step 9: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat: persistent sessions wired in; model's bash keeps cd/env; one HTTP client"
```
