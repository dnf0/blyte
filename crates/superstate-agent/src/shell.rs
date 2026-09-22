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

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

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
    use std::io::Read;
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
            stderr: format!(
                "{stderr}\n[killed by superstate after {}s]",
                timeout.as_secs()
            ),
            exit_code: 124, // conventional timeout code, as GNU timeout(1) uses
        }),
    }
}

/// Commands run on this machine, in the workspace directory.
#[derive(Debug, Default, Clone)]
pub struct LocalShell;

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
