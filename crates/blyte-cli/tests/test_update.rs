//! `blyte update` replaces the binary with the latest release, keeps the old one as blyte.prev,
//! rolls back, and refreshes an unedited skill (2026-09-30). A stand-in `gh` (BLYTE_GH) serves the
//! release, and the tests run a copy of blyte, so the real binary is never replaced.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use blyte_cli::skill::SKILL_MD;
use blyte_cli::update::{asset_name, parse_version};

struct Env {
    _dir: tempfile::TempDir,
    bin: PathBuf,
    gh: PathBuf,
    home: PathBuf,
    log: PathBuf,
}

fn exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A copy of blyte in a temp dir, a stand-in gh serving `tag` and a release binary that prints
/// `new_version` (or fails when `broken`), and an empty HOME.
fn setup(tag: &str, broken: bool) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin/blyte");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_blyte"), &bin).unwrap();
    let log = dir.path().join("calls.log");
    let release = dir.path().join("release-blyte");
    if broken {
        exe(&release, "#!/bin/sh\nexit 1\n");
    } else {
        exe(
            &release,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'blyte 9.9.9 (test)'; exit 0; fi\necho \"$@\" >> {}\n",
                log.display()
            ),
        );
    }
    let gh = dir.path().join("gh");
    exe(
        &gh,
        &format!(
            "#!/bin/sh\ncase \"$2\" in\n  view) echo '{tag}' ;;\n  download) while [ $# -gt 0 ]; do [ \"$1\" = -O ] && out=$2; shift; done; cp {} \"$out\" ;;\nesac\n",
            release.display()
        ),
    );
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    Env {
        _dir: dir,
        bin,
        gh,
        home,
        log,
    }
}

fn run(e: &Env, args: &[&str]) -> (i32, String) {
    let out = Command::new(&e.bin)
        .arg("update")
        .args(args)
        .env("BLYTE_GH", &e.gh)
        .env("HOME", &e.home)
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code().unwrap_or(-1), text)
}

fn version(bin: &Path) -> String {
    String::from_utf8_lossy(&Command::new(bin).arg("--version").output().unwrap().stdout)
        .into_owned()
}

#[test]
fn versions_parse_and_this_machine_has_an_asset() {
    assert_eq!(parse_version("v0.2.1"), Some((0, 2, 1)));
    assert_eq!(parse_version("0.10.3"), Some((0, 10, 3)));
    assert_eq!(parse_version("v1.2"), None);
    assert!(
        asset_name().is_some(),
        "the test machines are macOS or Linux x86_64"
    );
}

#[test]
fn check_reports_and_changes_nothing() {
    let e = setup("v9.9.9", false);
    let before = std::fs::read(&e.bin).unwrap();
    let (code, text) = run(&e, &["--check"]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("latest:    blyte 9.9.9"), "{text}");
    assert_eq!(std::fs::read(&e.bin).unwrap(), before);
}

#[test]
fn update_replaces_the_binary_keeps_the_old_one_and_rolls_back() {
    let e = setup("v9.9.9", false);
    let before = std::fs::read(&e.bin).unwrap();
    let (code, text) = run(&e, &[]);
    assert_eq!(code, 0, "{text}");
    assert!(version(&e.bin).contains("9.9.9"));
    let prev = e.bin.with_file_name("blyte.prev");
    assert_eq!(std::fs::read(&prev).unwrap(), before);

    // Roll back with the previous build (the real blyte), which is now blyte.prev.
    let out = Command::new(&prev)
        .args(["update", "--rollback"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read(&e.bin).unwrap(), before);
}

#[test]
fn an_up_to_date_install_is_left_alone() {
    let e = setup(&format!("v{}", env!("CARGO_PKG_VERSION")), false);
    let before = std::fs::read(&e.bin).unwrap();
    let (code, text) = run(&e, &[]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("up to date"), "{text}");
    assert_eq!(std::fs::read(&e.bin).unwrap(), before);
}

#[test]
fn a_download_that_does_not_run_changes_nothing() {
    let e = setup("v9.9.9", true);
    let before = std::fs::read(&e.bin).unwrap();
    let (code, text) = run(&e, &[]);
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("does not run"), "{text}");
    assert_eq!(std::fs::read(&e.bin).unwrap(), before);
    assert!(!e.bin.with_file_name("blyte.prev").exists());
}

#[test]
fn an_unedited_skill_is_refreshed_keeping_the_default_setting() {
    let e = setup("v9.9.9", false);
    let skill = e.home.join(".claude/skills/blyte-implementer/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, SKILL_MD).unwrap();
    let (code, text) = run(&e, &[]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("skill refreshed"), "{text}");
    // No default block in CLAUDE.md, so the refresh keeps "only when asked".
    assert_eq!(
        std::fs::read_to_string(&e.log).unwrap().trim(),
        "skill install --force --no-default"
    );
}

#[test]
fn an_edited_skill_is_not_touched() {
    let e = setup("v9.9.9", false);
    let skill = e.home.join(".claude/skills/blyte-implementer/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, "my own edits\n").unwrap();
    let (code, text) = run(&e, &[]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("has been edited"), "{text}");
    assert!(!e.log.exists());
}
