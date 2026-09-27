//! Where commands actually run. A seam, not a hardcoded `bash -lc`.
//!
//! # Why this is a trait
//!
//! The model-facing `bash` tool must mean the same thing regardless of whether execution lands
//! on the host, inside a container, or on another machine. Baking the target into the tool --
//! or worse, into the prompt -- is how the previous harness produced its most expensive class
//! of bug: dependencies lived in a container, the worker was *told in prose* to reach them via
//! `./run-in-container`, and when that instruction went missing it edited the host while its
//! verification ran in the sandbox. Nothing was wrong with the tool; the environment was a
//! convention rather than a property.
//!
//! With a seam there is nothing to instruct and nothing to forget. A deployment composes the
//! executor; the model sees `bash` either way. This is the shape DeepSeek's harness settled on
//! (`ctx.shell` backed by `bash-local` or `bash-sandbox`, `ctx.sandbox` by `sandbox-local` or
//! `sandbox-ssh`) and it is right for the same reason.
//!
//! It also makes the environment probe honest: [`Shell::available_binaries`] runs *through* the
//! executor, so a container deployment reports what is reachable inside the container rather
//! than what happens to be installed on the host.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::sandbox::Profile;

/// Variables the harness itself uses, never passed to the commands it runs: provider credentials
/// (the model could otherwise read them with `env`) and blyte's own configuration (a
/// `BLYTE_MODEL` in the environment broke the repository's provider tests). Every variable named
/// `BLYTE_*` is removed as well. Container commands never inherit the host environment.
pub const HARNESS_ENV: &[&str] = &[
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "ANTHROPIC_API_KEY",
    "OLLAMA_API_KEY",
    "OLLAMA_HOST",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "OPENROUTER_API_KEY",
];

const CREDENTIAL_SEGMENTS: &[&str] = &[
    "TOKEN",
    "SECRET",
    "SECRETS",
    "PASSWORD",
    "PASSWD",
    "PASS",
    "KEY",
    "CREDENTIAL",
    "CREDENTIALS",
    "AUTH",
    "COOKIE",
    "SESSION",
];

static PASS_ENV: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// True when either the upper-cased name, split on `_`, has a credential segment,
/// or the value contains a URL with inline credentials.
pub fn credential_like(name: &str, value: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if upper
        .split('_')
        .any(|seg| CREDENTIAL_SEGMENTS.contains(&seg))
    {
        return true;
    }
    has_inline_credentials(value)
}

fn has_inline_credentials(value: &str) -> bool {
    let mut rest = value;
    while let Some(idx) = rest.find("://") {
        let after_scheme = &rest[idx + 3..];
        let len = after_scheme
            .find(|c: char| c == '/' || c.is_whitespace())
            .unwrap_or(after_scheme.len());
        let authority_prefix = &after_scheme[..len];
        if let Some((userinfo, _)) = authority_prefix.split_once('@') {
            if let Some((_user, pass)) = userinfo.split_once(':') {
                if !pass.is_empty() {
                    return true;
                }
            }
        }
        rest = after_scheme;
    }
    false
}

/// Process-wide allow-list of environment variable names to keep even if they look like credentials.
///
/// Calling this a second time is a no-op.
pub fn set_pass_env(names: Vec<String>) {
    let _ = PASS_ENV.set(names);
}

/// The sorted names of current process environment variables withheld by the credential filter.
pub fn withheld_env() -> Vec<String> {
    let pass_env = PASS_ENV.get();
    let mut withheld = Vec::new();
    for (key, val) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if HARNESS_ENV.iter().any(|&h| h == name.as_ref()) {
            continue;
        }
        if name.starts_with("BLYTE_") {
            continue;
        }
        if let Some(passed) = pass_env {
            if passed.iter().any(|p| p == name.as_ref()) {
                continue;
            }
        }
        let value = val.to_string_lossy();
        if credential_like(&name, &value) {
            withheld.push(name.into_owned());
        }
    }
    withheld.sort();
    withheld.dedup();
    withheld
}

/// Remove the harness's own variables and credential-looking variables from a command about to run on this machine.
fn scrub(cmd: &mut Command) {
    for name in HARNESS_ENV {
        cmd.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("BLYTE_") {
            cmd.env_remove(&name);
        }
    }
    for name in withheld_env() {
        cmd.env_remove(name);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl Output {
    /// stdout and stderr as the model should see them: interleaved is not reconstructable
    /// after the fact, so stdout first then stderr, which is how a terminal usually reads.
    pub fn merged(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

pub trait Shell: Send + Sync {
    /// Run a command to completion. Blocking by contract -- see `tools` for why.
    fn exec(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String>;

    /// Run a command the **model** issued. Where the executor keeps a persistent session, its
    /// `cd` and `export` carry over to the model's next command, as in a terminal. The default
    /// is `exec`: a fresh process in `cwd`.
    fn exec_user(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        self.exec(command, cwd, timeout)
    }

    /// Human-readable description of where commands land, for the system prompt.
    /// A worker that knows it is inside a container does not go looking for the host.
    fn describe(&self) -> String;

    /// Which binaries exist **in the execution environment**, not on the orchestrator's host.
    fn available_binaries(&self, cwd: &Path, candidates: &[&'static str]) -> Vec<String> {
        // One probe rather than N: a remote or containerised shell pays a round trip per call,
        // and sixteen of them before the first turn is a poor way to start.
        let probe = candidates
            .iter()
            .map(|b| format!("command -v {b} >/dev/null 2>&1 && echo {b}"))
            .collect::<Vec<_>>()
            .join("; ");
        match self.exec(&probe, cwd, Duration::from_secs(30)) {
            Ok(o) => o
                .stdout
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// Executes a command with timeout enforcement, pipe draining, and process group isolation.
///
/// # Arguments
///
/// * `cmd` - The prepared [`Command`] ready to be configured and spawned.
/// * `timeout` - Maximum duration allowed before the process and its descendants are terminated.
///
/// # Returns
///
/// Returns an [`Output`] containing collected stdout, stderr, and the process exit code,
/// or an error message if the process cannot be spawned or waited on. On timeout, returns
/// exit code 124 with a notice appended to stderr.
fn exec_command(mut cmd: Command, timeout: Duration) -> Result<Output, String> {
    use std::process::Stdio;

    // The deadline is ENFORCED, not advisory.
    // stdin is null so commands that read input (e.g. bare python, git commit, prompts)
    // do not block forever on an unattended terminal.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Place the spawned command in its own process group on Unix.
    // Without this, background grandchildren (e.g., `(sleep 30) & wait`) stay alive
    // when the immediate child is killed, keeping pipe handles open and causing
    // the reader threads to hang indefinitely.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot run command: {e}"))?;

    // Drained on background threads: a command that fills the pipe buffer deadlocks against
    // a parent that only reads after waiting, which turns a noisy command into a hang.
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
                // On Unix, kill the entire process group with a negative PGID.
                // This ensures all child and grandchild processes are terminated simultaneously,
                // releasing standard stream pipe handles so reader threads unblock promptly.
                #[cfg(unix)]
                {
                    let pgid = child.id() as i32;
                    if pgid > 0 {
                        unsafe {
                            libc::kill(-pgid, libc::SIGKILL);
                        }
                    } else {
                        let _ = child.kill();
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
        // Reported as a failure the model can act on, with the output captured so far --
        // a killed command that returns nothing tells it only that something went wrong.
        None => Ok(Output {
            stdout,
            stderr: format!("{stderr}\n[killed by blyte after {}s]", timeout.as_secs()),
            exit_code: 124, // conventional timeout code, as GNU timeout(1) uses
        }),
    }
}

/// Commands run on this machine, in the workspace directory.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LocalShell {
    pub profile: Option<Profile>,
}

#[allow(non_upper_case_globals)]
pub const LocalShell: LocalShell = LocalShell { profile: None };

impl LocalShell {
    /// Creates an unsandboxed local shell.
    pub fn new() -> Self {
        Self { profile: None }
    }

    /// Creates a local shell sandboxed with the given profile.
    pub fn sandboxed(profile: impl Into<Profile>) -> Self {
        Self {
            profile: Some(profile.into()),
        }
    }
}

impl Shell for LocalShell {
    fn exec(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        let mut cmd = match &self.profile {
            Some(Profile::Seatbelt(profile)) => {
                let mut c = Command::new("/usr/bin/sandbox-exec");
                c.args(["-p", profile, "bash", "-lc", command]);
                c
            }
            Some(Profile::Landlock(paths)) => {
                #[cfg(target_os = "linux")]
                {
                    let mut c = Command::new("bash");
                    c.arg("-lc").arg(command);
                    crate::sandbox::landlock_pre_exec(&mut c, paths, false)
                        .map_err(|e| e.to_string())?;
                    c
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = paths;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "Landlock is only supported on Linux",
                    )
                    .to_string());
                }
            }
            Some(Profile::LandlockOffline(paths)) => {
                #[cfg(target_os = "linux")]
                {
                    let mut c = Command::new("bash");
                    c.arg("-lc").arg(command);
                    crate::sandbox::landlock_pre_exec(&mut c, paths, true)
                        .map_err(|e| e.to_string())?;
                    c
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = paths;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "Landlock is only supported on Linux",
                    )
                    .to_string());
                }
            }
            None => {
                let mut c = Command::new("bash");
                c.arg("-lc").arg(command);
                c
            }
        };
        cmd.current_dir(cwd);
        scrub(&mut cmd);
        exec_command(cmd, timeout)
    }

    fn describe(&self) -> String {
        "Commands run directly on this machine, in the workspace directory.".into()
    }
}

/// Commands run inside an already-running container via `docker exec`.
///
/// The case the previous harness handled with a prose instruction. Here the model still calls
/// `bash` and cannot route around it, because there is no host shell in its tool surface.
#[derive(Debug, Clone)]
pub struct ContainerShell {
    pub container_id: String,
    /// Where the workspace is mounted inside the container.
    pub workdir: PathBuf,
}

impl Shell for ContainerShell {
    fn exec(&self, command: &str, _cwd: &Path, timeout: Duration) -> Result<Output, String> {
        // Construct docker exec invocation targeting the running container and its workdir.
        let mut cmd = Command::new("docker");
        cmd.args(["exec", "-i", "-w"])
            .arg(&self.workdir)
            .arg(&self.container_id)
            .args(["bash", "-lc", command]);
        // Delegate to exec_command to ensure process group isolation and timeout parity.
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

/// Single-quote for a POSIX shell.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// How to start a session's shell: a login shell, as `bash -lc` gives today.
#[derive(Debug, Clone)]
pub enum Launcher {
    /// `bash -l` on this machine, starting in `root`, optionally sandboxed under a profile.
    Local {
        root: PathBuf,
        profile: Option<Profile>,
    },
    /// `bash -l` inside a running container, starting in `workdir`.
    Container {
        container_id: String,
        workdir: PathBuf,
    },
}

impl Launcher {
    fn command(&self) -> std::io::Result<Command> {
        match self {
            Launcher::Local { root, profile } => {
                let mut c = match profile {
                    Some(Profile::Seatbelt(p)) => {
                        let mut cmd = Command::new("/usr/bin/sandbox-exec");
                        cmd.args(["-p", p, "bash", "-l"]);
                        cmd
                    }
                    Some(Profile::Landlock(paths)) => {
                        #[cfg(target_os = "linux")]
                        {
                            let mut cmd = Command::new("bash");
                            cmd.arg("-l");
                            crate::sandbox::landlock_pre_exec(&mut cmd, paths, false)?;
                            cmd
                        }
                        #[cfg(not(target_os = "linux"))]
                        {
                            let _ = paths;
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::Unsupported,
                                "Landlock is only supported on Linux",
                            ));
                        }
                    }
                    Some(Profile::LandlockOffline(paths)) => {
                        #[cfg(target_os = "linux")]
                        {
                            let mut cmd = Command::new("bash");
                            cmd.arg("-l");
                            crate::sandbox::landlock_pre_exec(&mut cmd, paths, true)?;
                            cmd
                        }
                        #[cfg(not(target_os = "linux"))]
                        {
                            let _ = paths;
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::Unsupported,
                                "Landlock is only supported on Linux",
                            ));
                        }
                    }
                    None => {
                        let mut cmd = Command::new("bash");
                        cmd.arg("-l");
                        cmd
                    }
                };
                c.current_dir(root);
                scrub(&mut c);
                Ok(c)
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
                Ok(c)
            }
        }
    }

    /// The process-per-command executor this launcher replaces, used whenever a session
    /// cannot start.
    fn fallback(&self) -> Box<dyn Shell> {
        match self {
            Launcher::Local { profile, .. } => match profile {
                Some(p) => Box::new(LocalShell::sandboxed(p.clone())),
                None => Box::new(LocalShell),
            },
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
    TimedOut {
        out: String,
        err: String,
    },
    /// The shell ended before reporting (e.g. the command ran `exit`).
    Died {
        out: String,
        err: String,
    },
}

/// One long-lived `bash -l`. A command arrives through a heredoc read by the `read` builtin; its
/// stdout and stderr go to their own files and come back with one `printf` behind a header of
/// byte lengths. Only the command itself runs as a process: the previous protocol spawned about
/// six helpers per command (`base64`, `tr`, `rm`), about 25 ms on macOS.
pub struct Session {
    child: Child,
    stdin: ChildStdin,
    chunks: Receiver<Vec<u8>>,
    buf: Vec<u8>,
    tmp: String,
    next: u64,
    nonce: String,
}

const READY: &str = "__BLYTE_READY__ ";

/// A per-process token for heredoc delimiters, so no command or file content can end one early.
fn nonce() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{t:x}{:x}", std::process::id())
}

enum ReadErr {
    Timeout,
    Closed,
}

impl Session {
    pub fn start(launcher: &Launcher) -> Result<Session, String> {
        let mut cmd = launcher
            .command()
            .map_err(|e| format!("cannot start a shell session: {e}"))?;
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
        let mut stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut chunk = vec![0u8; 64 * 1024];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(chunk[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let mut s = Session {
            child,
            stdin,
            chunks: rx,
            buf: Vec::new(),
            tmp: String::new(),
            next: 0,
            nonce: nonce(),
        };
        s.send(
            "__blyte_t=$(mktemp -d 2>/dev/null || { mkdir -p /tmp/blyte.$$ && echo /tmp/blyte.$$; }); \
             echo \"__BLYTE_READY__ $__blyte_t\"\n",
        )?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match s.read_line(deadline) {
                Ok(l) if l.starts_with(READY) => {
                    s.tmp = l[READY.len()..].trim().to_string();
                    return Ok(s);
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

    fn fill(&mut self, deadline: Instant) -> Result<(), ReadErr> {
        match self
            .chunks
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(c) => {
                self.buf.extend_from_slice(&c);
                Ok(())
            }
            Err(RecvTimeoutError::Timeout) => Err(ReadErr::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(ReadErr::Closed),
        }
    }

    /// The next line of session output, without its newline.
    fn read_line(&mut self, deadline: Instant) -> Result<String, ReadErr> {
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                return Ok(String::from_utf8_lossy(&line[..i]).into_owned());
            }
            self.fill(deadline)?;
        }
    }

    /// Exactly `n` bytes of session output.
    fn read_exact(&mut self, n: usize, deadline: Instant) -> Result<Vec<u8>, ReadErr> {
        while self.buf.len() < n {
            self.fill(deadline)?;
        }
        Ok(self.buf.drain(..n).collect())
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
        let (out, err) = (
            format!("{}/{id}.o", self.tmp),
            format!("{}/{id}.e", self.tmp),
        );
        let (qo, qe, qt) = (quote(&out), quote(&err), quote(&self.tmp));
        let delim = format!("__BLYTE_CMD_{id}_{}__", self.nonce);
        let body = match cwd {
            Some(d) => format!(
                "( cd {} && eval \"$__blyte_c\" )",
                quote(&d.to_string_lossy())
            ),
            None => "eval \"$__blyte_c\"".to_string(),
        };
        // `|| true` on the protocol's own reads: a model's `set -e` must not end the session
        // because `read` reports end of input. LC_ALL=C makes `${#var}` count bytes; the
        // model's LC_ALL is put back afterwards.
        // Recreate tmp before each command so output redirects succeed even if a command deleted it.
        let script = format!(
            "IFS= read -r -d '' __blyte_c <<'{delim}' || true\n{command}\n{delim}\n\
             mkdir -p {qt} 2>/dev/null; {body} </dev/null >{qo} 2>{qe}; __blyte_r=$?; \
             IFS= read -r -d '' __blyte_o <{qo} || true; \
             IFS= read -r -d '' __blyte_e <{qe} || true; \
             __blyte_l=${{LC_ALL-__blyte_unset__}}; LC_ALL=C; \
             printf '__BLYTE_{id}__ %s %s %s\\n%s%s' \"$__blyte_r\" \"${{#__blyte_o}}\" \
             \"${{#__blyte_e}}\" \"$__blyte_o\" \"$__blyte_e\"; \
             if [ \"$__blyte_l\" = __blyte_unset__ ]; then unset LC_ALL; else LC_ALL=$__blyte_l; fi; \
             unset __blyte_c __blyte_o __blyte_e __blyte_l\n"
        );
        if self.send(&script).is_err() {
            return RunResult::Died { out, err };
        }
        let marker = format!("__BLYTE_{id}__ ");
        let deadline = Instant::now() + timeout;
        let fail = |e: ReadErr, out: String, err: String| match e {
            ReadErr::Timeout => RunResult::TimedOut { out, err },
            ReadErr::Closed => RunResult::Died { out, err },
        };
        let header = loop {
            match self.read_line(deadline) {
                Ok(l) => {
                    if let Some(rest) = l.strip_prefix(&marker) {
                        break rest.to_string();
                    }
                }
                Err(e) => return fail(e, out, err),
            }
        };
        let mut f = header.split(' ');
        let exit_code = f.next().and_then(|x| x.parse().ok()).unwrap_or(-1);
        let no: usize = f.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        let ne: usize = f.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        match self.read_exact(no + ne, deadline) {
            Ok(bytes) => RunResult::Done(Output {
                stdout: String::from_utf8_lossy(&bytes[..no]).into_owned(),
                stderr: String::from_utf8_lossy(&bytes[no..]).into_owned(),
                exit_code,
            }),
            Err(e) => fail(e, out, err),
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
        Self::new(Launcher::Local {
            root,
            profile: None,
        })
    }

    pub fn local_sandboxed(root: PathBuf, profile: impl Into<Profile>) -> Self {
        Self::new(Launcher::Local {
            root,
            profile: Some(profile.into()),
        })
    }

    pub fn sandboxed(root: PathBuf, profile: impl Into<Profile>) -> Self {
        Self::local_sandboxed(root, profile)
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
