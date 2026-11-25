//! Prepares a dispatch worktree's environment before the implementer starts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use blyte_agent::shell::Shell;

pub const SETUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn absolute_log_path(
    log_path: &Path,
    get_cwd: impl FnOnce() -> std::io::Result<PathBuf>,
) -> Result<PathBuf, String> {
    if log_path.is_absolute() {
        Ok(log_path.to_path_buf())
    } else {
        get_cwd().map(|cwd| cwd.join(log_path)).map_err(|e| {
            format!(
                "failed to resolve absolute setup log path for {} (current working directory unavailable): {e}",
                log_path.display()
            )
        })
    }
}

fn write_setup_log(log_path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = log_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "failed to write setup log {} (creating directory {}): {e}",
                    log_path.display(),
                    parent.display()
                )
            })?;
        }
    }
    std::fs::write(log_path, content)
        .map_err(|e| format!("failed to write setup log {}: {e}", log_path.display()))
}

fn tail_lines(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > max_lines {
        lines[lines.len() - max_lines..].join("\n")
    } else {
        lines.join("\n")
    }
}

fn format_stream_tails(stdout: &str, stderr: &str) -> String {
    let mut sections = Vec::new();
    if !stdout.is_empty() {
        sections.push(format!("stdout:\n{}", tail_lines(stdout, 20)));
    }
    if !stderr.is_empty() {
        sections.push(format!("stderr:\n{}", tail_lines(stderr, 20)));
    }
    sections.join("\n")
}

/// Runs `setup` with bash in `worktree`, outside the sandbox, writing its combined output to
/// `log_path`. Every `env` entry and `BLYTE_MAIN_CHECKOUT=<main_checkout>` are exported first.
/// Returns `Ok(())` when exit code is 0 and the log is written; otherwise returns `Err(message)`
/// including the failure header, the last 20 lines of each non-empty stream under `stdout:` /
/// `stderr:` labels, and a trailing `full log: <absolute path>` line (or the log write error
/// naming the path if writing failed).
pub fn run_setup(
    setup: &str,
    worktree: &Path,
    main_checkout: &Path,
    env: &BTreeMap<String, String>,
    log_path: &Path,
    timeout: Duration,
) -> Result<(), String> {
    run_setup_with_cwd(
        setup,
        worktree,
        main_checkout,
        env,
        log_path,
        timeout,
        std::env::current_dir,
    )
}

fn run_setup_with_cwd(
    setup: &str,
    worktree: &Path,
    main_checkout: &Path,
    env: &BTreeMap<String, String>,
    log_path: &Path,
    timeout: Duration,
    get_cwd: impl FnOnce() -> std::io::Result<PathBuf>,
) -> Result<(), String> {
    let abs_log_path = absolute_log_path(log_path, get_cwd)?;

    let mut script = String::new();
    script.push_str(&format!(
        "export BLYTE_MAIN_CHECKOUT={}\n",
        quote(&main_checkout.to_string_lossy())
    ));
    for (k, v) in env {
        script.push_str(&format!("export {k}={}\n", quote(v)));
    }
    script.push_str(setup);

    let shell = blyte_agent::shell::LocalShell::new();
    let res = shell.exec(&script, worktree, timeout);

    let (header, tail, log_result) = match res {
        Ok(output) => {
            let combined = output.merged();
            let log_result = write_setup_log(&abs_log_path, &combined);
            if output.exit_code == 0 {
                return log_result;
            }
            let is_timeout = output.exit_code == 124 && output.stderr.contains("killed by blyte");
            let header = if is_timeout {
                if timeout.as_secs() > 0 {
                    format!("setup failed: timed out after {}s", timeout.as_secs())
                } else {
                    "setup failed: timed out".to_string()
                }
            } else {
                format!("setup failed (exit {})", output.exit_code)
            };
            let tail = format_stream_tails(&output.stdout, &output.stderr);
            (header, tail, log_result)
        }
        Err(e) => {
            let log_result = write_setup_log(&abs_log_path, &format!("{e}\n"));
            let header = format!("setup failed: {e}");
            (header, String::new(), log_result)
        }
    };

    let footer = match log_result {
        Ok(()) => format!("full log: {}", abs_log_path.display()),
        Err(log_err) => log_err,
    };

    let mut parts = vec![header];
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts.push(footer);

    Err(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_log_path_errors_when_current_dir_fails_for_relative_path() {
        let rel = Path::new("relative/setup.log");
        let err = absolute_log_path(rel, || {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "current directory removed",
            ))
        })
        .expect_err("relative log path with failing current_dir must error");

        assert!(
            err.contains("relative/setup.log"),
            "error must name the relative log path: {err}"
        );
        assert!(
            err.contains("current working directory"),
            "error must name the missing current working directory: {err}"
        );
        assert!(
            err.contains("current directory removed"),
            "error must include the cause: {err}"
        );
    }

    #[test]
    fn absolute_log_path_keeps_already_absolute_path_without_calling_cwd() {
        let abs = Path::new("/tmp/already-abs/setup.log");
        let resolved = absolute_log_path(abs, || {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "should not be called",
            ))
        })
        .expect("absolute path should not need current_dir");
        assert_eq!(resolved, abs);
    }

    #[test]
    fn run_setup_errors_when_current_dir_fails_for_relative_log_path() {
        let wt = tempfile::tempdir().unwrap();
        let main = tempfile::tempdir().unwrap();
        let env = BTreeMap::new();
        let marker = wt.path().join("ran.txt");

        let err = run_setup_with_cwd(
            "echo ran > ran.txt",
            wt.path(),
            main.path(),
            &env,
            Path::new("nested/setup.log"),
            SETUP_TIMEOUT,
            || {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "cwd gone",
                ))
            },
        )
        .expect_err("run_setup with unresolvable relative log path must fail");

        assert!(
            err.contains("nested/setup.log"),
            "error must name log path: {err}"
        );
        assert!(
            err.contains("current working directory"),
            "error must name missing current working directory: {err}"
        );
        assert!(err.contains("cwd gone"), "error must include cause: {err}");
        assert!(
            !marker.exists(),
            "setup command must not run when log path cannot be resolved"
        );
    }
}
