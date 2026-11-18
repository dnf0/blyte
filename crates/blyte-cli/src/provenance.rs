//! Records the git state of each directory a research run may read, so the
//! report can say what was read rather than leave the reader to guess.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceState {
    /// The path is not inside a git work tree.
    NotGit,
    Git {
        /// Full commit id of HEAD.
        head: String,
        /// None when HEAD is detached.
        branch: Option<String>,
        /// Lines of `git status --porcelain`.
        uncommitted: usize,
        /// e.g. "origin/main"; None when origin/HEAD is not set.
        default_ref: Option<String>,
        /// (behind, ahead) of HEAD against `default_ref`; None when `default_ref` is None.
        behind_ahead: Option<(usize, usize)>,
    },
    /// A git command failed unexpectedly: the command and its stderr.
    Unknown(String),
}

/// Runs one git command in `path`. A spawn failure is an error naming the command.
fn run_git(path: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))
}

fn stdout_trimmed(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn failure(args: &[&str], out: &Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    format!("git {}: {stderr}", args.join(" "))
}

/// Runs a `git symbolic-ref -q` style command where exit 1 with empty output
/// means "not set". Returns Ok(None) for that case.
fn optional_ref(path: &Path, args: &[&str]) -> Result<Option<String>, String> {
    let out = run_git(path, args)?;
    if out.status.success() {
        let value = stdout_trimmed(&out);
        if value.is_empty() {
            return Err(failure(args, &out));
        }
        return Ok(Some(value));
    }
    if out.status.code() == Some(1) && stdout_trimmed(&out).is_empty() {
        return Ok(None);
    }
    Err(failure(args, &out))
}

fn git_state(path: &Path) -> Result<SourceState, String> {
    let inside = run_git(path, &["rev-parse", "--is-inside-work-tree"])?;
    if !inside.status.success() {
        let stderr = String::from_utf8_lossy(&inside.stderr);
        if stderr.contains("not a git repository") {
            return Ok(SourceState::NotGit);
        }
        return Err(failure(&["rev-parse", "--is-inside-work-tree"], &inside));
    }
    if stdout_trimmed(&inside) != "true" {
        return Ok(SourceState::NotGit);
    }

    let head_args = ["rev-parse", "HEAD"];
    let head_out = run_git(path, &head_args)?;
    if !head_out.status.success() {
        return Err(failure(&head_args, &head_out));
    }
    let head = stdout_trimmed(&head_out);

    let branch = optional_ref(path, &["symbolic-ref", "--short", "-q", "HEAD"])?;

    let status_args = ["status", "--porcelain"];
    let status_out = run_git(path, &status_args)?;
    if !status_out.status.success() {
        return Err(failure(&status_args, &status_out));
    }
    let uncommitted = String::from_utf8_lossy(&status_out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();

    let default_ref = optional_ref(
        path,
        &["symbolic-ref", "--short", "-q", "refs/remotes/origin/HEAD"],
    )?;

    let behind_ahead = match &default_ref {
        None => None,
        Some(default) => {
            let range = format!("HEAD...{default}");
            let rev_args = ["rev-list", "--left-right", "--count", range.as_str()];
            let out = run_git(path, &rev_args)?;
            if !out.status.success() {
                return Err(failure(&rev_args, &out));
            }
            let text = stdout_trimmed(&out);
            let parsed: Vec<&str> = text.split_whitespace().collect();
            let numbers: Result<Vec<usize>, _> =
                parsed.iter().map(|s| s.parse::<usize>()).collect();
            match numbers {
                // rev-list prints "<only in HEAD> <only in default>": ahead, behind.
                Ok(v) if v.len() == 2 => Some((v[1], v[0])),
                _ => {
                    return Err(format!(
                        "git rev-list --left-right --count: unexpected output `{text}`"
                    ))
                }
            }
        }
    };

    Ok(SourceState::Git {
        head,
        branch,
        uncommitted,
        default_ref,
        behind_ahead,
    })
}

/// The git state of `path`. Never guesses: a failing command yields `Unknown`.
pub fn source_state(path: &Path) -> SourceState {
    match git_state(path) {
        Ok(state) => state,
        Err(msg) => SourceState::Unknown(msg),
    }
}

fn describe(path: &Path, state: &SourceState) -> String {
    let p = path.display();
    match state {
        SourceState::NotGit => format!("- `{p}`: not a git repository"),
        SourceState::Unknown(msg) => format!("- `{p}`: unknown ({msg})"),
        SourceState::Git {
            head,
            branch,
            uncommitted,
            default_ref,
            behind_ahead,
        } => {
            let position = match branch {
                Some(b) => format!("branch `{b}` at `{head}`"),
                None => format!("detached HEAD at `{head}`"),
            };
            let distance = match (default_ref, behind_ahead) {
                (Some(default), Some((behind, ahead))) => {
                    if *behind == 0 && *ahead == 0 {
                        format!("level with `{default}` (as last fetched)")
                    } else {
                        format!(
                            "{behind} behind and {ahead} ahead of `{default}` (as last fetched)"
                        )
                    }
                }
                _ => "no `origin/HEAD` to compare with".to_string(),
            };
            let work = match uncommitted {
                0 => "clean".to_string(),
                1 => "1 uncommitted change".to_string(),
                n => format!("{n} uncommitted changes"),
            };
            format!("- `{p}`: {position}, {distance}, {work}")
        }
    }
}

/// The "Source state" block that research prompts and reports carry.
pub fn render(sources: &[(PathBuf, SourceState)]) -> String {
    let mut out = String::from("## Source state (written by the harness)\n\n");
    for (path, state) in sources {
        out.push_str(&describe(path, state));
        out.push('\n');
    }
    out.push_str(
        "\nThis is what the files on disk were when the run started. It is not necessarily the default branch.\n\
         Commit ids not listed here or shown in tool output must not be cited.",
    );
    out
}

/// The first lines of a research report whose run did not end `verified`.
pub fn unverified_banner(outcome: &str, turns: Option<u64>) -> String {
    let after = match turns {
        Some(n) => format!(" after {n} turns"),
        None => String::new(),
    };
    format!(
        "> **Not verified.** This research run ended as `{outcome}`{after}. The report below did not pass blyte's checks and may be an unfinished draft: check it before relying on it.\n\n"
    )
}

/// Put `unverified_banner` at the top of the report at `path`, unless `outcome` is "verified".
/// The rest of the file is kept byte for byte. A file that cannot be read or written is an error.
pub fn mark_if_unverified(path: &Path, outcome: &str, turns: Option<u64>) -> std::io::Result<()> {
    if outcome == "verified" {
        return Ok(());
    }
    let body = std::fs::read(path)?;
    let banner = unverified_banner(outcome, turns);
    if body.starts_with(banner.as_bytes()) {
        return Ok(());
    }
    let mut combined = Vec::with_capacity(banner.len() + body.len());
    combined.extend_from_slice(banner.as_bytes());
    combined.extend_from_slice(&body);
    std::fs::write(path, combined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_starts_with_the_prefix_the_brief_check_looks_for() {
        let banner = unverified_banner("budget_exhausted", Some(300));
        assert!(banner.starts_with(crate::research_brief::NOT_VERIFIED_PREFIX));
    }
}
