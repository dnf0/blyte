//! `blyte skill install` puts the Claude Code skill for the plan → delegate → review workflow
//! where Claude Code finds it; the skill is compiled into the binary so it matches it (2026-09-27).

use std::path::Path;
use std::process::Command;

use blyte_cli::skill::SKILL_MD;

fn blyte(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn the_embedded_skill_is_the_dispatch_workflow() {
    assert!(
        SKILL_MD.starts_with("---\nname: blyte-implementer\n"),
        "{}",
        &SKILL_MD[..60]
    );
    for s in [
        "blyte dispatch",
        "blyte review",
        "no_changes",
        "final_message",
    ] {
        assert!(SKILL_MD.contains(s), "{s} missing");
    }
}

#[test]
fn install_writes_the_personal_skill_and_refuses_to_clobber_an_edited_one() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let out = blyte(cwd.path(), home.path(), &["skill", "install"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = home
        .path()
        .join(".claude/skills/blyte-implementer/SKILL.md");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SKILL_MD);
    assert!(String::from_utf8_lossy(&out.stdout).contains("installed"));
    // Same content again: fine.
    assert_eq!(
        blyte(cwd.path(), home.path(), &["skill", "install"])
            .status
            .code(),
        Some(0)
    );
    // Edited by the user: refused without --force, replaced with it.
    std::fs::write(&path, "my edits").unwrap();
    let refused = blyte(cwd.path(), home.path(), &["skill", "install"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--force"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "my edits");
    assert_eq!(
        blyte(cwd.path(), home.path(), &["skill", "install", "--force"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SKILL_MD);
}

#[test]
fn project_install_goes_into_the_repository() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    let out = blyte(repo.path(), home.path(), &["skill", "install", "--project"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(repo
        .path()
        .join(".claude/skills/blyte-implementer/SKILL.md")
        .exists());
    assert!(!home.path().join(".claude").exists());
}

#[test]
fn print_writes_the_skill_to_stdout() {
    let home = tempfile::tempdir().unwrap();
    let out = blyte(home.path(), home.path(), &["skill", "print"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), SKILL_MD);
}
