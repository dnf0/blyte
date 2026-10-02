//! Tests for warning about a superseded `blyte-research` user skill.
//!
//! An older, separately installed skill told sessions to "keep --max-turns tight (80)",
//! contradicting the bundled `blyte-implementer` skill. `blyte skill install` and
//! `blyte update` warn the user to remove it, without removing it automatically.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use blyte_cli::skill::{superseded_skills, superseded_warning, SKILL_MD};

fn blyte(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn none_present_returns_empty_list_and_prints_nothing() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let found = superseded_skills(home.path());
    assert!(found.is_empty(), "expected empty, got {found:?}");

    let out = blyte(cwd.path(), home.path(), &["skill", "install"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.is_empty(),
        "expected empty stderr when no superseded skills exist, got: {stderr}"
    );
}

#[test]
fn claude_copy_present_is_listed_and_warned() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let research_dir = home.path().join(".claude/skills/blyte-research");
    std::fs::create_dir_all(&research_dir).unwrap();
    std::fs::write(research_dir.join("SKILL.md"), "# blyte-research\n").unwrap();

    let found = superseded_skills(home.path());
    assert_eq!(found, vec![research_dir.clone()]);

    let warning = superseded_warning(&research_dir);
    let expected = format!(
        "blyte: {} is superseded by the blyte-implementer skill's research section (default 300 turns); remove it: rm -r {}",
        research_dir.display(),
        research_dir.display()
    );
    assert_eq!(warning, expected);

    let out = blyte(cwd.path(), home.path(), &["skill", "install"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&expected),
        "expected stderr to contain:\n{expected}\nbut got:\n{stderr}"
    );

    // Verify it is never removed automatically
    assert!(
        research_dir.join("SKILL.md").exists(),
        "superseded skill should not be deleted automatically"
    );
}

#[test]
fn both_claude_and_codex_copies_are_listed_and_warned() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let claude_research = home.path().join(".claude/skills/blyte-research");
    let codex_research = home.path().join(".codex/skills/blyte-research");

    std::fs::create_dir_all(&claude_research).unwrap();
    std::fs::write(claude_research.join("SKILL.md"), "# claude research\n").unwrap();

    std::fs::create_dir_all(&codex_research).unwrap();
    std::fs::write(codex_research.join("SKILL.md"), "# codex research\n").unwrap();

    let found = superseded_skills(home.path());
    assert_eq!(found.len(), 2);
    assert!(found.contains(&claude_research));
    assert!(found.contains(&codex_research));

    let out = blyte(
        cwd.path(),
        home.path(),
        &["skill", "install", "--harness", "all"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let claude_expected = superseded_warning(&claude_research);
    let codex_expected = superseded_warning(&codex_research);
    assert!(
        stderr.contains(&claude_expected),
        "expected stderr to contain:\n{claude_expected}\nbut got:\n{stderr}"
    );
    assert!(
        stderr.contains(&codex_expected),
        "expected stderr to contain:\n{codex_expected}\nbut got:\n{stderr}"
    );

    // Both remain intact
    assert!(claude_research.exists());
    assert!(codex_research.exists());
}

#[test]
fn codex_agents_copy_is_listed_and_warned() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let agents_research = home.path().join(".agents/skills/blyte-research");
    std::fs::create_dir_all(&agents_research).unwrap();
    std::fs::write(agents_research.join("SKILL.md"), "# agents research\n").unwrap();

    let found = superseded_skills(home.path());
    assert_eq!(found, vec![agents_research.clone()]);

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
    let stderr = String::from_utf8_lossy(&out.stderr);
    let expected = superseded_warning(&agents_research);
    assert!(
        stderr.contains(&expected),
        "expected stderr to contain:\n{expected}\nbut got:\n{stderr}"
    );
    assert!(agents_research.exists());
}

#[test]
fn update_warns_about_superseded_skill_after_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin/blyte");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_blyte"), &bin).unwrap();

    let log = dir.path().join("calls.log");
    let release = dir.path().join("release-blyte");
    std::fs::write(
        &release,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'blyte 9.9.9 (test)'; exit 0; fi\necho \"$@\" >> {}\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&release, std::fs::Permissions::from_mode(0o755)).unwrap();

    let gh = dir.path().join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\ncase \"$2\" in\n  view) echo 'v9.9.9' ;;\n  download) while [ $# -gt 0 ]; do [ \"$1\" = -O ] && out=$2; shift; done; cp {} \"$out\" ;;\nesac\n",
            release.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    // Set up an installed skill so refresh runs
    let skill = home.join(".claude/skills/blyte-implementer/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, SKILL_MD).unwrap();

    // Set up superseded skill
    let research_dir = home.join(".claude/skills/blyte-research");
    std::fs::create_dir_all(&research_dir).unwrap();
    std::fs::write(research_dir.join("SKILL.md"), "# old research\n").unwrap();

    let out = Command::new(&bin)
        .arg("update")
        .env("BLYTE_GH", &gh)
        .env("HOME", &home)
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let expected = superseded_warning(&research_dir);
    assert!(
        stderr.contains(&expected),
        "expected stderr to contain:\n{expected}\nbut got:\n{stderr}"
    );
    assert!(research_dir.exists(), "must not be deleted automatically");
}
