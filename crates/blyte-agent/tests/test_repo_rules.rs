//! blyte loads the repository's rules for agents itself, however it was started (2026-09-30).

use std::path::Path;
use std::process::Command;

use blyte_agent::rules::{load, prompt_section, MAX_RULES_CHARS};

fn write(root: &Path, name: &str, text: &str) {
    let p = root.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn the_first_rule_file_found_wins() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(load(d.path()), None);
    assert_eq!(prompt_section(d.path()), "");

    write(d.path(), "AGENTS.md", "agents rules");
    assert_eq!(load(d.path()).unwrap().text, "agents rules");

    write(d.path(), ".claude/agy-rules.md", "implementer rules");
    assert_eq!(load(d.path()).unwrap().text, "implementer rules");

    write(d.path(), ".blyte/rules.md", "  blyte rules\n");
    let r = load(d.path()).unwrap();
    assert_eq!(r.text, "blyte rules");
    assert!(r.source.ends_with(".blyte/rules.md"));

    let section = prompt_section(d.path());
    assert!(section.starts_with("## Repository rules\n"), "{section}");
    assert!(section.contains("blyte rules"), "{section}");
}

#[test]
fn claude_md_is_not_read_and_empty_files_are_skipped() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "CLAUDE.md", "orchestrator instructions");
    assert_eq!(load(d.path()), None);
    write(d.path(), ".claude/agy-rules.md", "   \n");
    write(d.path(), "AGENTS.md", "agents rules");
    assert_eq!(load(d.path()).unwrap().text, "agents rules");
}

#[test]
fn long_rules_are_cut_with_a_note() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "AGENTS.md", &"x".repeat(MAX_RULES_CHARS + 500));
    let r = load(d.path()).unwrap();
    assert!(r.text.starts_with(&"x".repeat(MAX_RULES_CHARS)));
    assert!(
        r.text
            .ends_with("[cut at 12000 characters; the rest is in AGENTS.md]"),
        "{}",
        &r.text[r.text.len() - 80..]
    );
}

#[test]
fn a_worktree_finds_rules_kept_untracked_in_the_main_checkout() {
    let d = tempfile::tempdir().unwrap();
    let main = d.path().join("repo");
    std::fs::create_dir_all(&main).unwrap();
    let git = |dir: &Path, args: &[&str]| {
        assert!(Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
    };
    git(&main, &["init", "-q"]);
    write(&main, "f.txt", "a\n");
    git(&main, &["add", "f.txt"]);
    git(
        &main,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "i",
            "--no-gpg-sign",
        ],
    );
    write(&main, ".claude/agy-rules.md", "untracked implementer rules");
    let wt = d.path().join("wt");
    git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]);

    let r = load(&wt).unwrap();
    assert_eq!(r.text, "untracked implementer rules");
    assert!(r.source.starts_with(main.canonicalize().unwrap()) || r.source.starts_with(&main));
}
