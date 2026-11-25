use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::rules::MAX_RULES_CHARS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answers {
    Person,
    Nobody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub source: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    pub process: Option<Part>,
    pub repository: Option<Part>,
}

pub const DEFAULT_PROCESS_SOURCE: &str = "default (built in)";
pub const ANSWERS_PERSON: &str = "A person is reading your replies. A reply that ends with a question and calls no tool waits for their answer, which arrives as the next message.";
pub const ANSWERS_NOBODY: &str = "Nobody will answer during this run. It ends when you reply without calling a tool, so a question ends the run unanswered.";

pub fn user_rules_path() -> Option<PathBuf> {
    user_rules_path_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

pub fn user_rules_path_from(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(dir) = xdg.filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir).join("blyte").join("process.md"));
    }
    home.filter(|v| !v.is_empty())
        .map(|h| PathBuf::from(h).join(".config/blyte/process.md"))
}

pub fn repository_rules_path(root: &Path) -> PathBuf {
    root.join(".blyte").join("process.md")
}

pub fn load_rules_file(path: &Path) -> Result<Option<String>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let text = raw.trim();
    if text.is_empty() {
        return Err(format!("{} is empty", path.display()));
    }
    let count = text.chars().count();
    if count > MAX_RULES_CHARS {
        return Err(format!(
            "{} has {count} characters; the limit is {MAX_RULES_CHARS}",
            path.display()
        ));
    }
    Ok(Some(text.to_string()))
}

fn reject_old_rules_file(new_path: &Path) -> Result<(), String> {
    if let Some(dir) = new_path.parent() {
        let old_path = dir.join("orchestrator.md");
        if std::fs::symlink_metadata(&old_path).is_ok() {
            return Err(format!(
                "{} is no longer read: rename it to {}",
                old_path.display(),
                new_path.display()
            ));
        }
    }
    Ok(())
}

pub fn resolve_sources(
    user_path: Option<&Path>,
    root: &Path,
    default_process: Option<&str>,
) -> Result<Sources, String> {
    let repo_path = repository_rules_path(root);
    if let Some(path) = user_path {
        reject_old_rules_file(path)?;
    }
    reject_old_rules_file(&repo_path)?;
    let user_part = match user_path {
        Some(path) => load_rules_file(path)?.map(|text| Part {
            source: path.display().to_string(),
            text,
        }),
        None => None,
    };
    let process = user_part.or_else(|| {
        default_process.map(|text| Part {
            source: DEFAULT_PROCESS_SOURCE.to_string(),
            text: text.trim().to_string(),
        })
    });
    let repository = load_rules_file(&repo_path)?.map(|text| Part {
        source: repo_path.display().to_string(),
        text,
    });
    Ok(Sources {
        process,
        repository,
    })
}

pub fn assemble(reference: &str, answers: Answers, sources: &Sources) -> String {
    let answers_line = match answers {
        Answers::Person => ANSWERS_PERSON,
        Answers::Nobody => ANSWERS_NOBODY,
    };
    let mut out = format!("## blyte reference\n{}\n\n", reference.trim());
    out.push_str(answers_line);
    out.push_str("\n\n");
    if let Some(part) = &sources.process {
        out.push_str(&format!(
            "## Process (from {})\n{}\n\n",
            part.source,
            part.text.trim()
        ));
    }
    if let Some(part) = &sources.repository {
        out.push_str(&format!(
            "## This repository's rules (from {})\n{}\n\n",
            part.source,
            part.text.trim()
        ));
    }
    out
}

pub fn start_lines(sources: &Sources) -> [String; 2] {
    let process = sources
        .process
        .as_ref()
        .map_or("none", |p| p.source.as_str());
    let repository = sources
        .repository
        .as_ref()
        .map_or("none", |p| p.source.as_str());
    [
        format!("process: {process}"),
        format!("repository rules: {repository}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn user_rules_path_prefers_xdg() {
        let p = user_rules_path_from(Some(OsStr::new("/xdg")), Some(OsStr::new("/home/u")));
        assert_eq!(p, Some(PathBuf::from("/xdg/blyte/process.md")));
    }

    #[test]
    fn user_rules_path_falls_back_to_home() {
        let p = user_rules_path_from(None, Some(OsStr::new("/home/u")));
        assert_eq!(p, Some(PathBuf::from("/home/u/.config/blyte/process.md")));
        let p2 = user_rules_path_from(Some(OsStr::new("")), Some(OsStr::new("/home/u")));
        assert_eq!(p2, p);
    }

    #[test]
    fn user_rules_path_none_without_either() {
        assert_eq!(user_rules_path_from(None, None), None);
    }

    #[test]
    fn load_missing_file_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("missing.md");
        assert_eq!(load_rules_file(&path), Ok(None));
    }

    #[test]
    fn load_trims_text() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("process.md");
        std::fs::write(&path, "\n  plan first\n\n").unwrap();
        assert_eq!(load_rules_file(&path), Ok(Some("plan first".to_string())));
    }

    #[test]
    fn load_empty_file_is_an_error_naming_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty.md");
        std::fs::write(&path, "   \n\t\n").unwrap();
        let err = load_rules_file(&path).unwrap_err();
        assert!(err.contains(&path.display().to_string()), "{err}");
        assert!(err.contains("is empty"), "{err}");
    }

    #[test]
    fn load_file_over_the_limit_is_an_error_with_both_numbers() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("oversized.md");
        let over = MAX_RULES_CHARS + 1;
        std::fs::write(&path, "x".repeat(over)).unwrap();
        let err = load_rules_file(&path).unwrap_err();
        assert!(err.contains(&path.display().to_string()), "{err}");
        assert!(err.contains(&over.to_string()), "{err}");
        assert!(err.contains(&MAX_RULES_CHARS.to_string()), "{err}");
    }

    #[test]
    fn load_file_at_the_limit_is_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("exact.md");
        let text = "a".repeat(MAX_RULES_CHARS);
        std::fs::write(&path, &text).unwrap();
        let loaded = load_rules_file(&path).unwrap().unwrap();
        assert_eq!(loaded.chars().count(), MAX_RULES_CHARS);
        assert_eq!(loaded, text);
    }

    #[test]
    fn user_file_replaces_the_default_process() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let user_path = tmp.path().join("user.md");
        std::fs::write(&user_path, "Custom user process.").unwrap();

        let sources =
            resolve_sources(Some(&user_path), &root, Some("Built-in default process.")).unwrap();
        let out = assemble("Ref facts.", Answers::Person, &sources);
        assert!(out.contains("Custom user process."), "{out}");
        assert!(
            out.contains(&format!("## Process (from {})", user_path.display())),
            "{out}"
        );
        assert!(!out.contains("Built-in default process."), "{out}");
    }

    #[test]
    fn no_user_file_uses_the_default_and_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.md");
        let sources = resolve_sources(Some(&missing), tmp.path(), Some("Default steps.")).unwrap();
        let out = assemble("Ref facts.", Answers::Nobody, &sources);
        assert!(
            out.contains("## Process (from default (built in))\nDefault steps.\n\n"),
            "{out}"
        );
    }

    #[test]
    fn no_default_and_no_user_file_has_no_process_section() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.md");
        let sources = resolve_sources(Some(&missing), tmp.path(), None).unwrap();
        assert!(sources.process.is_none());
        let out = assemble("Ref facts.", Answers::Nobody, &sources);
        assert!(!out.contains("## Process"), "{out}");
    }

    #[test]
    fn repository_file_is_added_after_the_process() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_file = repository_rules_path(tmp.path());
        std::fs::create_dir_all(repo_file.parent().unwrap()).unwrap();
        std::fs::write(&repo_file, "Repo specific conversation model rule.").unwrap();

        let sources = resolve_sources(None, tmp.path(), Some("Default process.")).unwrap();
        let out = assemble("Ref facts.", Answers::Person, &sources);
        let process_pos = out.find("## Process (from ").unwrap();
        let repo_pos = out.find("## This repository's rules (from ").unwrap();
        assert!(repo_pos > process_pos, "{out}");
        assert!(
            out.contains("Repo specific conversation model rule."),
            "{out}"
        );
        assert!(out.contains(&repo_file.display().to_string()), "{out}");
    }

    #[test]
    fn sections_come_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_file = repository_rules_path(tmp.path());
        std::fs::create_dir_all(repo_file.parent().unwrap()).unwrap();
        std::fs::write(&repo_file, "REPO_BODY").unwrap();

        let sources = resolve_sources(None, tmp.path(), Some("PROCESS_BODY")).unwrap();
        let out = assemble("  REF_BODY  \n", Answers::Person, &sources);
        let expected = format!(
            "## blyte reference\nREF_BODY\n\n{ANSWERS_PERSON}\n\n## Process (from {DEFAULT_PROCESS_SOURCE})\nPROCESS_BODY\n\n## This repository's rules (from {})\nREPO_BODY\n\n",
            repo_file.display()
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn answers_person_and_nobody_differ() {
        let sources = Sources {
            process: None,
            repository: None,
        };
        let person = assemble("Ref", Answers::Person, &sources);
        let nobody = assemble("Ref", Answers::Nobody, &sources);
        assert!(person.contains(ANSWERS_PERSON));
        assert!(!person.contains(ANSWERS_NOBODY));
        assert!(nobody.contains(ANSWERS_NOBODY));
        assert!(!nobody.contains(ANSWERS_PERSON));
    }

    #[test]
    fn start_lines_name_sources_or_none() {
        let empty = Sources {
            process: None,
            repository: None,
        };
        assert_eq!(
            start_lines(&empty),
            [
                "process: none".to_string(),
                "repository rules: none".to_string(),
            ]
        );

        let full = Sources {
            process: Some(Part {
                source: DEFAULT_PROCESS_SOURCE.to_string(),
                text: "p".to_string(),
            }),
            repository: Some(Part {
                source: "/repo/.blyte/process.md".to_string(),
                text: "r".to_string(),
            }),
        };
        assert_eq!(
            start_lines(&full),
            [
                "process: default (built in)".to_string(),
                "repository rules: /repo/.blyte/process.md".to_string(),
            ]
        );
    }

    #[test]
    fn repository_old_orchestrator_file_returns_rename_error() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join(".blyte").join("orchestrator.md");
        let new = tmp.path().join(".blyte").join("process.md");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "old rules").unwrap();

        let err = resolve_sources(None, tmp.path(), Some("Default process.")).unwrap_err();
        assert!(err.contains(&old.display().to_string()), "{err}");
        assert!(err.contains(&new.display().to_string()), "{err}");
        assert!(err.contains(".blyte/orchestrator.md"), "{err}");
        assert!(err.contains(".blyte/process.md"), "{err}");
    }

    #[test]
    fn user_old_orchestrator_file_returns_rename_error() {
        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join("config").join("blyte");
        let old = user_dir.join("orchestrator.md");
        let new = user_dir.join("process.md");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::write(&old, "old user rules").unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();

        let err = resolve_sources(Some(&new), &repo, Some("Default process.")).unwrap_err();
        assert!(err.contains(&old.display().to_string()), "{err}");
        assert!(err.contains(&new.display().to_string()), "{err}");
    }

    #[test]
    fn old_orchestrator_file_still_errors_when_process_md_also_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_old = tmp.path().join(".blyte").join("orchestrator.md");
        let repo_new = tmp.path().join(".blyte").join("process.md");
        std::fs::create_dir_all(repo_old.parent().unwrap()).unwrap();
        std::fs::write(&repo_old, "old repo rules").unwrap();
        std::fs::write(&repo_new, "new repo rules").unwrap();

        let repo_err = resolve_sources(None, tmp.path(), Some("Default process.")).unwrap_err();
        assert!(
            repo_err.contains(&repo_old.display().to_string()),
            "{repo_err}"
        );
        assert!(
            repo_err.contains(&repo_new.display().to_string()),
            "{repo_err}"
        );

        let user_tmp = tempfile::tempdir().unwrap();
        let user_dir = user_tmp.path().join("blyte");
        let user_old = user_dir.join("orchestrator.md");
        let user_new = user_dir.join("process.md");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::write(&user_old, "old user rules").unwrap();
        std::fs::write(&user_new, "new user rules").unwrap();
        let clean_repo = user_tmp.path().join("repo");
        std::fs::create_dir_all(&clean_repo).unwrap();

        let user_err =
            resolve_sources(Some(&user_new), &clean_repo, Some("Default process.")).unwrap_err();
        assert!(
            user_err.contains(&user_old.display().to_string()),
            "{user_err}"
        );
        assert!(
            user_err.contains(&user_new.display().to_string()),
            "{user_err}"
        );
    }

    #[test]
    fn repository_process_md_alone_is_loaded_as_repository_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_new = tmp.path().join(".blyte").join("process.md");
        std::fs::create_dir_all(repo_new.parent().unwrap()).unwrap();
        std::fs::write(&repo_new, "Repository rules only.").unwrap();

        let sources = resolve_sources(None, tmp.path(), Some("Default process.")).unwrap();
        let repo = sources.repository.expect("repository rules loaded");
        assert_eq!(repo.source, repo_new.display().to_string());
        assert_eq!(repo.text, "Repository rules only.");
    }
}
