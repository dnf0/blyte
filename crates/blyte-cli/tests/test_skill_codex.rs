//! `blyte skill install --harness codex` sets up Codex CLI as the orchestrator with blyte
//! underneath: the skill where Codex finds it, the default block in AGENTS.md, and rules that let
//! `blyte dispatch` run outside Codex's sandbox (2026-09-27). Run end to end against a live Codex on 2026-09-30.

use std::path::Path;
use std::process::Command;

use blyte_cli::skill::{CODEX_RULES, DEFAULT_BLOCK, SKILL_MD};

fn blyte(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn a_personal_codex_install_writes_skills_agents_md_and_rules() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let out = blyte(
        cwd.path(),
        home.path(),
        &["skill", "install", "--harness", "codex"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let h = home.path();
    for p in [
        ".agents/skills/blyte-implementer/SKILL.md",
        ".codex/skills/blyte-implementer/SKILL.md",
    ] {
        assert_eq!(std::fs::read_to_string(h.join(p)).unwrap(), SKILL_MD, "{p}");
    }
    assert!(std::fs::read_to_string(h.join(".codex/AGENTS.md"))
        .unwrap()
        .contains(DEFAULT_BLOCK));
    assert_eq!(
        std::fs::read_to_string(h.join(".codex/rules/blyte.rules")).unwrap(),
        CODEX_RULES
    );
    assert!(!h.join(".claude").exists(), "codex only");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("(untested)"));

    let out = blyte(
        cwd.path(),
        home.path(),
        &["skill", "uninstall", "--harness", "codex"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!h.join(".agents/skills/blyte-implementer/SKILL.md").exists());
    assert!(!h.join(".codex/rules/blyte.rules").exists());
    assert!(!std::fs::read_to_string(h.join(".codex/AGENTS.md"))
        .unwrap()
        .contains("blyte:default"));
}

#[test]
fn all_installs_for_both_and_the_default_is_still_claude() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    assert_eq!(
        blyte(
            cwd.path(),
            home.path(),
            &["skill", "install", "--harness", "all"]
        )
        .status
        .code(),
        Some(0)
    );
    assert!(home
        .path()
        .join(".claude/skills/blyte-implementer/SKILL.md")
        .exists());
    assert!(home
        .path()
        .join(".agents/skills/blyte-implementer/SKILL.md")
        .exists());

    let home2 = tempfile::tempdir().unwrap();
    assert_eq!(
        blyte(cwd.path(), home2.path(), &["skill", "install"])
            .status
            .code(),
        Some(0)
    );
    assert!(home2
        .path()
        .join(".claude/skills/blyte-implementer/SKILL.md")
        .exists());
    assert!(!home2.path().join(".agents").exists());
    assert!(!home2.path().join(".codex").exists());
}

#[test]
fn a_project_codex_install_goes_into_the_repository_without_rules() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    let out = blyte(
        repo.path(),
        home.path(),
        &["skill", "install", "--harness", "codex", "--project"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(repo
        .path()
        .join(".agents/skills/blyte-implementer/SKILL.md")
        .exists());
    assert!(std::fs::read_to_string(repo.path().join("AGENTS.md"))
        .unwrap()
        .contains(DEFAULT_BLOCK));
    assert!(!home.path().join(".codex").exists());
}

#[test]
fn the_skill_covers_both_harnesses() {
    for s in [
        "**Starting parallel tasks.**",
        "**Under Codex:**",
        "escalated permissions",
        "claude -p",
    ] {
        assert!(SKILL_MD.contains(s), "{s} missing");
    }
    assert!(CODEX_RULES.contains(r#"prefix_rule(pattern=["blyte", "dispatch"], decision="allow")"#));
}
