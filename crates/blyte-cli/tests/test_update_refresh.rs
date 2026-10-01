//! `blyte update` refreshes every skill copy it installed and nobody edited, for every harness
//! (2026-10-01).

use std::path::Path;
use std::process::Command;

use blyte_cli::update::refresh_skills;

fn install(home: &Path, args: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .env("HOME", home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn unedited_copies_of_every_harness_are_refreshed_and_edited_ones_are_left() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    install(home, &["skill", "install"]);
    install(home, &["skill", "install", "--harness", "codex"]);
    let claude = home.join(".claude/skills/blyte-implementer");
    let codex = [
        home.join(".agents/skills/blyte-implementer"),
        home.join(".codex/skills/blyte-implementer"),
    ];
    for d in std::iter::once(&claude).chain(codex.iter()) {
        assert!(d.join(".blyte-installed").is_file(), "{d:?}");
        // As if an older release had installed it: both files hold the old text.
        std::fs::write(d.join("SKILL.md"), "old skill\n").unwrap();
        std::fs::write(d.join(".blyte-installed"), "old skill\n").unwrap();
    }
    let lines = refresh_skills(Path::new(env!("CARGO_BIN_EXE_blyte")), home);
    assert!(
        lines.iter().any(|l| l == "skill refreshed for Claude Code"),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l == "skill refreshed for Codex"),
        "{lines:?}"
    );
    for d in std::iter::once(&claude).chain(codex.iter()) {
        assert_ne!(
            std::fs::read_to_string(d.join("SKILL.md")).unwrap(),
            "old skill\n",
            "{d:?}"
        );
    }

    // An edited copy is left alone, and says so.
    std::fs::write(claude.join("SKILL.md"), "my own notes\n").unwrap();
    let lines = refresh_skills(Path::new(env!("CARGO_BIN_EXE_blyte")), home);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("skill not refreshed for Claude Code") && l.contains("edited")),
        "{lines:?}"
    );
    assert_eq!(
        std::fs::read_to_string(claude.join("SKILL.md")).unwrap(),
        "my own notes\n"
    );
}
