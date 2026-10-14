//! `blyte skill install` puts the Claude Code skill for the plan → delegate → review workflow
//! where Claude Code finds it; the skill is compiled into the binary so it matches it (2026-09-27).

use std::io::Cursor;
use std::path::Path;
use std::process::Command;

use blyte_cli::env_file::parse;
use blyte_cli::skill::{configure_model, SKILL_MD};

fn blyte(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("BLYTE_ENV_FILE", home.join(".config/blyte/env"))
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

#[test]
fn prompt_logic_chooses_default_flash_typed_id_reasks_once_and_sets_argon_timeout() {
    let dir = tempfile::tempdir().unwrap();

    // Empty answer -> gemini-4-argon + timeout 600.
    let env1 = dir.path().join("env1");
    let mut reader = Cursor::new(b"\n".as_slice());
    let mut writer = Vec::new();
    let chosen = configure_model(&env1, None, true, &mut reader, &mut writer).unwrap();
    assert_eq!(chosen.as_deref(), Some("gemini-4-argon"));
    assert_eq!(
        parse(&std::fs::read_to_string(&env1).unwrap()),
        vec![
            ("BLYTE_MODEL".to_string(), "gemini-4-argon".to_string()),
            ("BLYTE_MODEL_TIMEOUT_SECS".to_string(), "600".to_string()),
        ]
    );
    let out1 = String::from_utf8(writer).unwrap();
    assert!(out1.contains("BLYTE_MODEL=gemini-4-argon"), "{out1}");
    assert!(out1.contains("BLYTE_MODEL_TIMEOUT_SECS=600"), "{out1}");
    assert!(out1.contains(&env1.display().to_string()), "{out1}");

    // "2" -> gemini-3.8-flash-high (no timeout).
    let env2 = dir.path().join("env2");
    let mut reader = Cursor::new(b"2\n".as_slice());
    let mut writer = Vec::new();
    let chosen = configure_model(&env2, None, true, &mut reader, &mut writer).unwrap();
    assert_eq!(chosen.as_deref(), Some("gemini-3.8-flash-high"));
    assert_eq!(
        parse(&std::fs::read_to_string(&env2).unwrap()),
        vec![(
            "BLYTE_MODEL".to_string(),
            "gemini-3.8-flash-high".to_string()
        )]
    );

    // Typed model id -> used as given.
    let env3 = dir.path().join("env3");
    let mut reader = Cursor::new(b"openai/gpt-5.5\n".as_slice());
    let mut writer = Vec::new();
    let chosen = configure_model(&env3, None, true, &mut reader, &mut writer).unwrap();
    assert_eq!(chosen.as_deref(), Some("openai/gpt-5.5"));
    assert_eq!(
        parse(&std::fs::read_to_string(&env3).unwrap()),
        vec![("BLYTE_MODEL".to_string(), "openai/gpt-5.5".to_string())]
    );

    // Invalid answer ("3") re-asks once and accepts the second answer.
    let env4 = dir.path().join("env4");
    let mut reader = Cursor::new(b"3\nclaude-sonnet-4-5\n".as_slice());
    let mut writer = Vec::new();
    let chosen = configure_model(&env4, None, true, &mut reader, &mut writer).unwrap();
    assert_eq!(chosen.as_deref(), Some("claude-sonnet-4-5"));
    assert_eq!(
        parse(&std::fs::read_to_string(&env4).unwrap()),
        vec![("BLYTE_MODEL".to_string(), "claude-sonnet-4-5".to_string())]
    );
}

#[test]
fn model_flag_writes_without_asking_and_existing_blyte_model_is_left_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let env_path = home.path().join(".config/blyte/env");

    // No terminal and no --model: nothing written.
    let out = blyte(cwd.path(), home.path(), &["skill", "install"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(!env_path.exists());

    // --model writes without asking (and sets timeout for argon).
    let out = blyte(
        cwd.path(),
        home.path(),
        &["skill", "install", "--model", "gemini-4-argon"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&env_path).unwrap(),
        "BLYTE_MODEL=gemini-4-argon\nBLYTE_MODEL_TIMEOUT_SECS=600\n"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("BLYTE_MODEL=gemini-4-argon"), "{stdout}");

    // Existing BLYTE_MODEL without --model is printed and left unchanged, even with a terminal.
    let mut reader = Cursor::new(b"2\n".as_slice());
    let mut writer = Vec::new();
    let res = configure_model(&env_path, None, true, &mut reader, &mut writer).unwrap();
    assert_eq!(res.as_deref(), Some("gemini-4-argon"));
    let printed = String::from_utf8(writer).unwrap();
    assert!(printed.contains("gemini-4-argon"), "{printed}");
    assert_eq!(
        std::fs::read_to_string(&env_path).unwrap(),
        "BLYTE_MODEL=gemini-4-argon\nBLYTE_MODEL_TIMEOUT_SECS=600\n"
    );

    // And via the CLI without --model: prints current value and leaves file unchanged.
    let out = blyte(cwd.path(), home.path(), &["skill", "install"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("gemini-4-argon"));
    assert_eq!(
        std::fs::read_to_string(&env_path).unwrap(),
        "BLYTE_MODEL=gemini-4-argon\nBLYTE_MODEL_TIMEOUT_SECS=600\n"
    );

    // --model replaces an existing BLYTE_MODEL.
    let out = blyte(
        cwd.path(),
        home.path(),
        &["skill", "install", "--model", "gemini-3.8-flash-high"],
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(&env_path).unwrap(),
        "BLYTE_MODEL=gemini-3.8-flash-high\nBLYTE_MODEL_TIMEOUT_SECS=600\n"
    );
}
