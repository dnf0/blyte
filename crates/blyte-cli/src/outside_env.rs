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
    let tokens = shell_words(part);
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i].as_str();
        if t == "sudo" || is_env_assignment(t) {
            i += 1;
        } else {
            break;
        }
    }
    let Some(prog) = tokens.get(i).map(String::as_str) else {
        return false;
    };
    let prog_name = Path::new(prog)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty());
    if !matches!(prog_name, Some("cp" | "ln" | "rsync" | "mv")) {
        return false;
    }

    for raw_arg in &tokens[i + 1..] {
        if raw_arg.is_empty() {
            continue;
        }
        let arg = if raw_arg.starts_with('-') {
            match raw_arg.split_once('=') {
                Some((_, val)) => val,
                None => continue,
            }
        } else {
            raw_arg.as_str()
        };
        if has_env_dir_component(arg) && is_outside_worktree(arg, norm_wt) {
            return true;
        }
    }
    false
}

/// Splits a shell command segment into words, honouring single quotes, double quotes, and backslash escapes.
pub fn shell_words(part: &str) -> Vec<String> {
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote = Quote::None;
    let mut chars = part.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Quote::None => match c {
                c if c.is_whitespace() => {
                    if in_word {
                        words.push(std::mem::take(&mut current));
                        in_word = false;
                    }
                }
                '\'' => {
                    quote = Quote::Single;
                    in_word = true;
                }
                '"' => {
                    quote = Quote::Double;
                    in_word = true;
                }
                '\\' => {
                    in_word = true;
                    match chars.next() {
                        Some(next) => current.push(next),
                        None => current.push('\\'),
                    }
                }
                _ => {
                    in_word = true;
                    current.push(c);
                }
            },
            Quote::Single => {
                if c == '\'' {
                    quote = Quote::None;
                } else {
                    current.push(c);
                }
            }
            Quote::Double => match c {
                '"' => quote = Quote::None,
                '\\' => {
                    if matches!(chars.peek(), Some('"' | '\\')) {
                        if let Some(next) = chars.next() {
                            current.push(next);
                        }
                    } else {
                        current.push('\\');
                    }
                }
                _ => current.push(c),
            },
        }
    }

    if in_word {
        words.push(current);
    }
    words
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

/// Lexically normalises a path without filesystem access, keeping leading `..` components on relative paths.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut stack: Vec<Component<'_>> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => stack.push(comp),
            Component::CurDir => {}
            Component::ParentDir => match stack.last() {
                Some(Component::Normal(_)) => {
                    stack.pop();
                }
                Some(Component::RootDir) => {}
                _ => stack.push(comp),
            },
        }
    }
    stack.into_iter().collect()
}
