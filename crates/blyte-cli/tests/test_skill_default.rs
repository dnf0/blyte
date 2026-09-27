//! `blyte skill install --default` makes delegating to blyte Claude Code's default through a
//! marked CLAUDE.md block; `uninstall` removes it. The skill teaches parallel dispatch
//! (2026-09-27).

use std::path::Path;
use std::process::Command;

use blyte_cli::skill::{remove_default_block, set_default_block, DEFAULT_BLOCK, SKILL_MD};

fn blyte(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn the_block_is_appended_once_and_replaced_in_place() {
    assert_eq!(set_default_block(""), format!("{DEFAULT_BLOCK}\n"));
    let once = set_default_block("# Me\n\nMy rules.\n");
    assert_eq!(once, format!("# Me\n\nMy rules.\n\n{DEFAULT_BLOCK}\n"));
    assert_eq!(set_default_block(&once), once, "idempotent");
    let stale = "# Me\n\n<!-- blyte:default -->\nold text\n<!-- /blyte:default -->\n\n## After\n";
    let fixed = set_default_block(stale);
    assert!(
        fixed.contains(DEFAULT_BLOCK) && !fixed.contains("old text"),
        "{fixed}"
    );
    assert!(fixed.contains("## After"), "{fixed}");
    assert_eq!(remove_default_block(&once), "# Me\n\nMy rules.\n");
    assert_eq!(remove_default_block("# Me\n"), "# Me\n");
}

#[test]
fn install_default_writes_claude_md_and_uninstall_removes_both() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let md = home.path().join(".claude/CLAUDE.md");
    std::fs::create_dir_all(md.parent().unwrap()).unwrap();
    std::fs::write(&md, "# My rules\n").unwrap();

    let out = blyte(cwd.path(), home.path(), &["skill", "install", "--default"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("updated"));
    let text = std::fs::read_to_string(&md).unwrap();
    assert!(
        text.starts_with("# My rules\n") && text.contains(DEFAULT_BLOCK),
        "{text}"
    );

    let out = blyte(cwd.path(), home.path(), &["skill", "uninstall"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read_to_string(&md).unwrap(), "# My rules\n");
    assert!(!home
        .path()
        .join(".claude/skills/blyte-implementer/SKILL.md")
        .exists());
    let again = blyte(cwd.path(), home.path(), &["skill", "uninstall"]);
    assert_eq!(again.status.code(), Some(0));
}

#[test]
fn install_with_no_default_leaves_claude_md_alone() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    assert_eq!(
        blyte(
            cwd.path(),
            home.path(),
            &["skill", "install", "--no-default"]
        )
        .status
        .code(),
        Some(0)
    );
    assert!(!home.path().join(".claude/CLAUDE.md").exists());
}

#[test]
fn the_skill_is_a_default_and_teaches_parallel_dispatch() {
    assert!(
        SKILL_MD.contains("description: Use by default"),
        "description must trigger by default"
    );
    for s in ["in parallel", "Siblings", "wait", "rebase", "--base wip/T1"] {
        assert!(SKILL_MD.contains(s), "{s} missing");
    }
}
