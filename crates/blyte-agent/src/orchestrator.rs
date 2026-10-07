//! The instructions for `blyte --orchestrate`: blyte as the planner and reviewer that dispatches
//! implementers, as Claude Code does with the blyte-implementer skill.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::rules::MAX_RULES_CHARS;

/// What the orchestrator is. Stands in for the implementer's opening line.
pub const PREAMBLE: &str = "\
You are the orchestrator in a repository. You plan the work, dispatch implementers with \
`blyte dispatch`, review what they did with `blyte review`, record a verdict, and merge. You do not \
write the implementation yourself, except for a one-line fix; every other change goes to a dispatch.

You work with a person. When you need a decision that is theirs to make, or you have a plan for them \
to approve, end your reply with the question and stop: their answer arrives as the next message. Do not \
ask about choices with a conventional default; pick it, say so, and carry on.

Keep tool output small, since everything you run stays in the conversation: send dispatch output to \
`/dev/null` (each run's summary is in `.wt/<id>.json`), wait with `blyte wait`, read `blyte review <id>` \
before any `--diff` and open the diff only for what it flags, and trim long output with `| tail -n 40` or \
`grep`. When you need a detail from earlier that is no longer in view, re-read it from the plan file or \
`.wt/<id>.task.md` rather than guessing.";

/// The user's rules file: `$XDG_CONFIG_HOME/blyte/orchestrator.md` when `XDG_CONFIG_HOME` is set and not
/// empty, else `$HOME/.config/blyte/orchestrator.md`; `None` when neither variable is set.
pub fn user_rules_path() -> Option<PathBuf> {
    user_rules_path_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

fn user_rules_path_from(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(xdg) = xdg.filter(|s| !s.is_empty()) {
        Some(PathBuf::from(xdg).join("blyte").join("orchestrator.md"))
    } else {
        home.map(|home| {
            PathBuf::from(home)
                .join(".config")
                .join("blyte")
                .join("orchestrator.md")
        })
    }
}

/// The file's text, trimmed; `None` if it is missing, unreadable or empty. Longer than
/// `crate::rules::MAX_RULES_CHARS` characters: cut there, followed by
/// `"\n\n[cut at {MAX_RULES_CHARS} characters; the rest is in {path}]"` (path displayed in full).
pub fn load_user_rules(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.chars().count() > MAX_RULES_CHARS {
        let cut: String = text.chars().take(MAX_RULES_CHARS).collect();
        Some(format!(
            "{cut}\n\n[cut at {MAX_RULES_CHARS} characters; the rest is in {}]",
            path.display()
        ))
    } else {
        Some(text.to_string())
    }
}

/// The orchestrator sections of the system prompt, in this order:
/// "## How you work\n{PREAMBLE}\n\n## The blyte-implementer skill\n{skill}\n\n", then, if `user_rules`
/// is `Some((path, text))`, "## Your rules (from {path})\n{text}\n\n". `skill` is inserted trimmed.
pub fn prompt_sections(skill: &str, user_rules: Option<(&Path, &str)>) -> String {
    let skill = skill.trim();
    if let Some((path, text)) = user_rules {
        format!(
            "## How you work\n{PREAMBLE}\n\n## The blyte-implementer skill\n{skill}\n\n## Your rules (from {})\n{text}\n\n",
            path.display()
        )
    } else {
        format!("## How you work\n{PREAMBLE}\n\n## The blyte-implementer skill\n{skill}\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_rules_path_prefers_xdg() {
        let path = user_rules_path_from(
            Some(OsStr::new("/custom/config")),
            Some(OsStr::new("/home/user")),
        );
        assert_eq!(
            path,
            Some(PathBuf::from("/custom/config/blyte/orchestrator.md"))
        );
    }

    #[test]
    fn user_rules_path_falls_back_to_home() {
        let path = user_rules_path_from(None, Some(OsStr::new("/home/user")));
        assert_eq!(
            path,
            Some(PathBuf::from("/home/user/.config/blyte/orchestrator.md"))
        );
    }

    #[test]
    fn user_rules_path_ignores_empty_xdg() {
        let path = user_rules_path_from(Some(OsStr::new("")), Some(OsStr::new("/home/user")));
        assert_eq!(
            path,
            Some(PathBuf::from("/home/user/.config/blyte/orchestrator.md"))
        );
    }

    #[test]
    fn user_rules_path_none_without_home() {
        assert_eq!(user_rules_path_from(None, None), None);
        assert_eq!(user_rules_path_from(Some(OsStr::new("")), None), None);
    }

    #[test]
    fn load_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.md");
        assert_eq!(load_user_rules(&path), None);
    }

    #[test]
    fn load_blank_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blank.md");
        std::fs::write(&path, "   \n\t  \r\n  ").unwrap();
        assert_eq!(load_user_rules(&path), None);
    }

    #[test]
    fn load_trims() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.md");
        std::fs::write(&path, "\n  be brief \n").unwrap();
        assert_eq!(load_user_rules(&path).as_deref(), Some("be brief"));
    }

    #[test]
    fn load_caps_long_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orchestrator.md");
        let long_text = "é".repeat(MAX_RULES_CHARS + 10);
        std::fs::write(&path, long_text).unwrap();
        let result = load_user_rules(&path).unwrap();
        let expected_prefix = format!("{}\n\n[cut at ", "é".repeat(MAX_RULES_CHARS));
        assert!(result.starts_with(&expected_prefix));
        let expected_suffix = format!("{}]", path.display());
        assert!(result.ends_with(&expected_suffix));
    }

    #[test]
    fn sections_in_order() {
        let path = Path::new("/custom/orchestrator.md");
        let s = prompt_sections("  SKILL BODY \n", Some((path, "RULES BODY")));
        assert!(s.contains(PREAMBLE));
        let pos1 = s.find("## How you work").unwrap();
        let pos2 = s
            .find("## The blyte-implementer skill\nSKILL BODY\n")
            .unwrap();
        let pos3 = s.find("## Your rules (from ").unwrap();
        assert!(pos1 < pos2);
        assert!(pos2 < pos3);
    }

    #[test]
    fn sections_without_rules() {
        let s = prompt_sections("S", None);
        assert!(!s.contains("## Your rules"));
    }

    #[test]
    fn preamble_names_the_commands() {
        assert!(PREAMBLE.contains("blyte dispatch"));
        assert!(PREAMBLE.contains("blyte review"));
        assert!(PREAMBLE.contains("blyte wait"));
        assert!(PREAMBLE.contains("/dev/null"));
    }
}
