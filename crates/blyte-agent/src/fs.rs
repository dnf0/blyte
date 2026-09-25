//! Where files actually live. The companion seam to [`crate::shell`].
//!
//! Routing commands through an executor while reading and writing files on the orchestrator's
//! own disk is coherent for exactly one deployment -- a bind-mounted workspace, where both
//! views address the same bytes. For a shell that executes on another machine, or in a
//! container without a mount, it is silently wrong: the model edits one filesystem and verifies
//! another. That is the same defect the previous harness had in reverse, and it cost four runs
//! before anyone noticed the worker was editing the host while its gate ran in the sandbox.
//!
//! DeepSeek's harness keeps `ctx.fs` separate from `ctx.shell` for this reason. So does this.
//!
//! Canonicalisation belongs here too, not in the tools. The workspace-escape check is only
//! meaningful against the filesystem the write will actually land on -- resolving symlinks on
//! the orchestrator's disk says nothing about where a path leads inside a container.

use std::path::{Path, PathBuf};
use std::process::Command;

pub trait Fs: Send + Sync {
    fn read(&self, path: &Path) -> Result<String, String>;
    fn write(&self, path: &Path, content: &str) -> Result<(), String>;
    /// Resolve symlinks and `..`. Used for the workspace-escape check, so it must answer for
    /// the filesystem the operation will really touch.
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, String>;
    /// Whether paths name files on this machine, so tools may read them in-process.
    fn is_local(&self) -> bool {
        false
    }
}

#[derive(Debug, Default, Clone)]
pub struct LocalFs;

impl Fs for LocalFs {
    fn is_local(&self) -> bool {
        true
    }
    fn read(&self, path: &Path) -> Result<String, String> {
        std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
    }
    fn write(&self, path: &Path, content: &str) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        std::fs::write(path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, String> {
        path.canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Files inside a running container, reached the same way its shell is.
#[derive(Debug, Clone)]
pub struct ContainerFs {
    pub container_id: String,
}

impl ContainerFs {
    fn exec(&self, args: &[&str], stdin: Option<&str>) -> Result<(String, i32), String> {
        use std::io::Write;
        use std::process::Stdio;
        let mut cmd = Command::new("docker");
        cmd.args(["exec", "-i", &self.container_id])
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("docker exec failed: {e}"))?;
        if let Some(body) = stdin {
            child
                .stdin
                .take()
                .ok_or("no stdin")?
                .write_all(body.as_bytes())
                .map_err(|e| format!("cannot send content: {e}"))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| format!("docker exec failed: {e}"))?;
        Ok((
            String::from_utf8_lossy(&out.stdout).to_string(),
            out.status.code().unwrap_or(-1),
        ))
    }
}

impl Fs for ContainerFs {
    fn read(&self, path: &Path) -> Result<String, String> {
        let p = path.to_string_lossy().to_string();
        match self.exec(&["cat", "--", &p], None)? {
            (body, 0) => Ok(body),
            _ => Err(format!("cannot read {p} in container")),
        }
    }
    fn write(&self, path: &Path, content: &str) -> Result<(), String> {
        let p = path.to_string_lossy().to_string();
        let dir = path
            .parent()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();
        if !dir.is_empty() {
            self.exec(&["mkdir", "-p", "--", &dir], None)?;
        }
        // `tee` rather than a redirect: the content never passes through a shell, so quotes,
        // backticks and `$` in source code cannot be interpreted on the way in.
        match self.exec(&["tee", "--", &p], Some(content))? {
            (_, 0) => Ok(()),
            _ => Err(format!("cannot write {p} in container")),
        }
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, String> {
        let p = path.to_string_lossy().to_string();
        match self.exec(&["readlink", "-f", "--", &p], None)? {
            (out, 0) if !out.trim().is_empty() => Ok(PathBuf::from(out.trim())),
            _ => Err(format!("cannot resolve {p} in container")),
        }
    }
}

/// Files reached through a persistent session: the container counterpart of [`LocalFs`] at a
/// round trip of a few milliseconds, instead of 2-5 `docker exec` per operation (a container
/// `edit` cost about 300 ms). Content crosses through a heredoc read by the `read` builtin, so no byte is interpreted on the
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
        // Content through a heredoc read by the `read` builtin: no process, no encoding. The
        // heredoc adds one final newline, which is stripped, so the bytes are exact.
        let delim = format!(
            "__BLYTE_CONTENT_{}_{}__",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let cmd = format!(
            "IFS= read -r -d '' __blyte_w <<'{delim}' || true\n{content}\n{delim}\n\
             __blyte_w=${{__blyte_w%$'\\n'}}; {mkdir}printf %s \"$__blyte_w\" > {}",
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
