//! The repository's own rules for agents, loaded on every run so that blyte gets them however it
//! was started (2026-09-30: runs started with `blyte dispatch` directly saw only the task text,
//! while the dispatch scripts appended `.claude/agy-rules.md`).
//!
//! The first file found wins: `.blyte/rules.md` (written for blyte), `.claude/agy-rules.md` (the
//! implementer rules repositories already keep for headless agents), then `AGENTS.md` (the
//! cross-tool convention). `CLAUDE.md` is not read: it is written for the orchestrator. Each is
//! looked for in the working tree, then in the main checkout, since the file may be untracked.

use std::path::{Path, PathBuf};

/// The files blyte reads rules from, in order of precedence (relative to the repository root).
pub const RULE_FILES: [&str; 3] = [".blyte/rules.md", ".claude/agy-rules.md", "AGENTS.md"];

/// Rules longer than this are cut, with a note, so one large file cannot crowd out the task.
pub const MAX_RULES_CHARS: usize = 12_000;

/// Repository rules: where they came from and their text (already capped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRules {
    pub source: PathBuf,
    pub text: String,
}

/// The main checkout's root when `root` is a linked git worktree, else `None`.
fn main_checkout(root: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let common = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let main = common.parent()?.to_path_buf();
    let same = main.canonicalize().ok()? == root.canonicalize().ok()?;
    (!same).then_some(main)
}

/// Load the repository's rules for `root`, or `None` when it has none.
pub fn load(root: &Path) -> Option<RepoRules> {
    let mut roots = vec![root.to_path_buf()];
    roots.extend(main_checkout(root));
    for name in RULE_FILES {
        for base in &roots {
            let path = base.join(name);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let text = if text.chars().count() > MAX_RULES_CHARS {
                let cut: String = text.chars().take(MAX_RULES_CHARS).collect();
                format!("{cut}\n\n[cut at {MAX_RULES_CHARS} characters; the rest is in {name}]")
            } else {
                text.to_string()
            };
            return Some(RepoRules { source: path, text });
        }
    }
    None
}

/// The system prompt section for the repository's rules; empty when it has none.
pub fn prompt_section(root: &Path) -> String {
    match load(root) {
        Some(r) => format!(
            "## Repository rules\nThe repository's own rules for agents, from `{}`. Follow them \
             unless the task says otherwise; where they name a check command, it is the same \
             check that decides completion.\n\n{}\n\n",
            r.source.display(),
            r.text
        ),
        None => String::new(),
    }
}
