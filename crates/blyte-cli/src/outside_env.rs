//! Detects shell commands in a run log that borrow a dependency environment from outside the worktree.

use std::path::{Component, Path, PathBuf};

pub const ENV_DIRS: &[&str] = &[
    ".venv",
    "venv",
    "node_modules",
    "site-packages",
    "archive-v0",
];

/// Extracts the `worktree` path from a parsed run summary JSON value.
/// Returns an `Err` naming the missing field if `worktree` is absent or empty.
pub fn summary_worktree(summary: &serde_json::Value) -> Result<PathBuf, String> {
    let wt = summary
        .get("worktree")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing `worktree` field in summary".to_string())?;
    Ok(PathBuf::from(wt))
}

/// Returns the logged shell commands that take a dependency environment from outside `worktree`.
/// Each command is returned once, in log order, trimmed to 200 characters.
pub fn borrowed_env_commands(log: &str, worktree: &Path) -> Vec<String> {
    let norm_wt = normalize_path(worktree);
    let mut out = Vec::new();
    for cmd in crate::dispatch::bash_commands(log) {
        if borrows_env(&cmd, &norm_wt) {
            let trimmed: String = cmd.chars().take(200).collect();
            if !out.contains(&trimmed) {
                out.push(trimmed);
            }
        }
    }
    out
}

/// Formats the `== checks` flag for borrowed environment commands, or `None` when empty.
pub fn flag(commands: &[String]) -> Option<String> {
    let first = commands.first()?;
    Some(format!(
        "the gate may have run in an environment from outside the worktree: {} command(s), first: {}",
        commands.len(),
        first
    ))
}

fn borrows_env(cmd: &str, norm_wt: &Path) -> bool {
    cmd.split("&&")
        .flat_map(|s| s.split(';'))
        .flat_map(|s| s.split('|'))
        .any(|part| part_borrows_env(part, norm_wt))
}

fn part_borrows_env(part: &str, norm_wt: &Path) -> bool {
    let tokens: Vec<&str> = part.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i].trim_matches(|c: char| matches!(c, '"' | '\''));
        if t == "sudo" || is_env_assignment(t) {
            i += 1;
        } else {
            break;
        }
    }
    let Some(&raw_prog) = tokens.get(i) else {
        return false;
    };
    let prog = raw_prog.trim_matches(|c: char| matches!(c, '"' | '\''));
    let prog_name = Path::new(prog)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty());
    if !matches!(prog_name, Some("cp" | "ln" | "rsync" | "mv")) {
        return false;
    }

    for &raw_arg in &tokens[i + 1..] {
        let trimmed = raw_arg.trim_matches(|c: char| matches!(c, '"' | '\''));
        if trimmed.is_empty() {
            continue;
        }
        let arg = if trimmed.starts_with('-') {
            match trimmed.split_once('=') {
                Some((_, val)) => val.trim_matches(|c: char| matches!(c, '"' | '\'')),
                None => continue,
            }
        } else {
            trimmed
        };
        if has_env_dir_component(arg) && is_outside_worktree(arg, norm_wt) {
            return true;
        }
    }
    false
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn has_env_dir_component(arg: &str) -> bool {
    Path::new(arg).components().any(|c| match c {
        Component::Normal(os) => os.to_str().is_some_and(|s| ENV_DIRS.contains(&s)),
        _ => false,
    })
}

fn is_outside_worktree(arg: &str, norm_wt: &Path) -> bool {
    if arg == "~"
        || arg.starts_with("~/")
        || arg == "$HOME"
        || arg.starts_with("$HOME/")
        || arg.starts_with("${HOME}/")
    {
        return true;
    }
    let p = Path::new(arg);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        norm_wt.join(p)
    };
    let resolved = normalize_path(&joined);
    !resolved.starts_with(norm_wt)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !out.has_root() {
                    out.push("..");
                }
            }
            Component::Normal(c) => out.push(c),
        }
    }
    out
}
