//! Prepares a dispatch worktree's environment before the implementer starts.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use blyte_agent::shell::Shell;

pub const SETUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Runs `setup` with bash in `worktree`, outside the sandbox, writing its combined output to
/// `log_path`. Every `env` entry and `BLYTE_MAIN_CHECKOUT=<main_checkout>` are exported first.
/// Ok(()) on exit 0; Err(message) otherwise, where message is
/// "setup failed (exit N)" or "setup failed: <error>" (a timeout or spawn failure), followed by
/// a newline and the last 20 lines of the output.
pub fn run_setup(
    setup: &str,
    worktree: &Path,
    main_checkout: &Path,
    env: &BTreeMap<String, String>,
    log_path: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let mut script = String::new();
    script.push_str(&format!(
        "export BLYTE_MAIN_CHECKOUT={}\n",
        quote(&main_checkout.to_string_lossy())
    ));
    for (k, v) in env {
        script.push_str(&format!("export {k}={}\n", quote(v)));
    }
    script.push_str(setup);

    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let shell = blyte_agent::shell::LocalShell::new();
    let res = shell.exec(&script, worktree, timeout);

    let (header, combined) = match res {
        Ok(output) => {
            let combined = output.merged();
            let _ = std::fs::write(log_path, &combined);
            if output.exit_code == 0 {
                return Ok(());
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
            (header, combined)
        }
        Err(e) => {
            let _ = std::fs::write(log_path, format!("{e}\n"));
            let header = format!("setup failed: {e}");
            (header, String::new())
        }
    };

    let lines: Vec<&str> = combined.lines().collect();
    let tail = if lines.len() > 20 {
        lines[lines.len() - 20..].join("\n")
    } else {
        lines.join("\n")
    };

    let msg = if tail.is_empty() {
        header
    } else {
        format!("{header}\n{tail}")
    };

    Err(msg)
}
