use std::path::PathBuf;
use std::process::Command;

pub const SKILL_MD: &str = include_str!("../assets/blyte-implementer/SKILL.md");

pub const DEFAULT_BLOCK: &str = "\
<!-- blyte:default -->
## Implementation: delegate to blyte, in parallel

For code changes in a git repository that has a check command (tests, lint), plan and review
the work yourself and delegate the implementation to blyte with the `blyte-implementer` skill.
Split the work into independent tasks and dispatch them in parallel (`blyte dispatch`, one
worktree each); run tasks one after another only when one depends on another. Keep for
yourself: design decisions, ambiguous requirements, one-line edits, and anything with no check.
<!-- /blyte:default -->";

const START_MARKER: &str = "<!-- blyte:default -->";
const END_MARKER: &str = "<!-- /blyte:default -->";

fn repo_root() -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("failed to execute git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "not a git repository: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

fn home_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME")
        .map_err(|_| "HOME environment variable is unset or empty".to_string())?;
    if home.trim().is_empty() {
        return Err("HOME environment variable is unset or empty".to_string());
    }
    Ok(PathBuf::from(home))
}

pub fn skill_dir(project: bool) -> Result<PathBuf, String> {
    if project {
        Ok(repo_root()?.join(".claude/skills/blyte-implementer"))
    } else {
        Ok(home_dir()?.join(".claude/skills/blyte-implementer"))
    }
}

pub fn claude_md_path(project: bool) -> Result<PathBuf, String> {
    if project {
        Ok(repo_root()?.join("CLAUDE.md"))
    } else {
        Ok(home_dir()?.join(".claude/CLAUDE.md"))
    }
}

pub fn set_default_block(text: &str) -> String {
    if let Some(start) = text.find(START_MARKER) {
        if let Some(end_rel) = text[start..].find(END_MARKER) {
            let end = start + end_rel + END_MARKER.len();
            let mut result = format!("{}{DEFAULT_BLOCK}{}", &text[..start], &text[end..]);
            while result.ends_with("\n\n") {
                result.pop();
            }
            if !result.ends_with('\n') {
                result.push('\n');
            }
            return result;
        }
    }

    if text.is_empty() {
        format!("{DEFAULT_BLOCK}\n")
    } else if text.ends_with('\n') {
        format!("{text}\n{DEFAULT_BLOCK}\n")
    } else {
        format!("{text}\n\n{DEFAULT_BLOCK}\n")
    }
}

pub fn remove_default_block(text: &str) -> String {
    let Some(start) = text.find(START_MARKER) else {
        return text.to_string();
    };
    let Some(end_rel) = text[start..].find(END_MARKER) else {
        return text.to_string();
    };
    let end = start + end_rel + END_MARKER.len();

    let prefix = &text[..start];
    let prefix_clean = if prefix.ends_with("\n\n") {
        &prefix[..prefix.len() - 1]
    } else if prefix == "\n" {
        ""
    } else {
        prefix
    };

    let suffix = if text[end..].starts_with('\n') {
        &text[end + 1..]
    } else {
        &text[end..]
    };

    format!("{prefix_clean}{suffix}")
}

pub fn install(project: bool, force: bool, no_default: bool) -> Result<PathBuf, String> {
    let dir = skill_dir(project)?;
    let skill_path = dir.join("SKILL.md");

    if skill_path.exists() {
        let existing = std::fs::read_to_string(&skill_path)
            .map_err(|e| format!("failed to read {}: {e}", skill_path.display()))?;
        if existing != SKILL_MD && !force {
            return Err(format!(
                "{} already exists with different content; pass --force to overwrite",
                skill_path.display()
            ));
        }
    }

    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create directory {}: {e}", dir.display()))?;
    std::fs::write(&skill_path, SKILL_MD)
        .map_err(|e| format!("failed to write {}: {e}", skill_path.display()))?;

    if !no_default {
        let md_path = claude_md_path(project)?;
        if let Some(parent) = md_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
        }
        let existing = if md_path.exists() {
            std::fs::read_to_string(&md_path)
                .map_err(|e| format!("failed to read {}: {e}", md_path.display()))?
        } else {
            String::new()
        };
        let updated = set_default_block(&existing);
        std::fs::write(&md_path, updated)
            .map_err(|e| format!("failed to write {}: {e}", md_path.display()))?;
    }

    Ok(skill_path)
}

pub fn uninstall(project: bool) -> Result<Vec<PathBuf>, String> {
    let dir = skill_dir(project)?;
    let skill_path = dir.join("SKILL.md");
    let mut changed = Vec::new();

    if skill_path.exists() {
        std::fs::remove_file(&skill_path)
            .map_err(|e| format!("failed to remove {}: {e}", skill_path.display()))?;
        changed.push(skill_path);
    }

    if dir.exists() {
        if let Ok(mut entries) = std::fs::read_dir(&dir) {
            if entries.next().is_none() {
                let _ = std::fs::remove_dir(&dir);
            }
        }
    }

    let md_path = claude_md_path(project)?;
    if md_path.exists() {
        let existing = std::fs::read_to_string(&md_path)
            .map_err(|e| format!("failed to read {}: {e}", md_path.display()))?;
        let updated = remove_default_block(&existing);
        if updated != existing {
            std::fs::write(&md_path, updated)
                .map_err(|e| format!("failed to write {}: {e}", md_path.display()))?;
            changed.push(md_path);
        }
    }

    Ok(changed)
}
