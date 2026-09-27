//! Installing the skill makes blyte the default, with parallel dispatch, without a flag: most
//! developers do not know to ask for either (2026-09-27).

use std::path::Path;
use std::process::Command;

use blyte_cli::skill::{DEFAULT_BLOCK, SKILL_MD};

fn blyte(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn plain_install_writes_the_default_block() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let out = blyte(cwd.path(), home.path(), &["skill", "install"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let md = std::fs::read_to_string(home.path().join(".claude/CLAUDE.md")).unwrap();
    assert!(md.contains(DEFAULT_BLOCK), "{md}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("updated"));
}

#[test]
fn the_block_and_the_skill_make_parallel_the_default() {
    assert!(
        DEFAULT_BLOCK.contains("dispatch them in parallel"),
        "{DEFAULT_BLOCK}"
    );
    assert!(SKILL_MD.contains("## 1. Plan for parallel work (the default)"));
    assert!(SKILL_MD.contains("Dispatch every independent task at once."));
    assert!(SKILL_MD.contains("dispatch them to blyte in parallel"));
}

#[test]
fn the_old_default_flag_is_still_accepted() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    assert_eq!(
        blyte(cwd.path(), home.path(), &["skill", "install", "--default"])
            .status
            .code(),
        Some(0)
    );
}
