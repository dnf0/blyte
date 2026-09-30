//! `blyte update`: replace this binary with the latest GitHub release (2026-09-30).
//!
//! Releases carry one static binary per platform (release-on-merge.yml). The repository is private,
//! so the download goes through the GitHub CLI (`gh`), which already holds the user's access;
//! blyte handles no token. The new binary must run before it replaces the old one, the old one is
//! kept beside it as `blyte.prev` (`blyte update --rollback` swaps them back), and the Claude Code
//! skill is refreshed when the installed copy is this version's, unedited.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository whose releases are installed.
pub const REPO: &str = "climate-x-org/blyte";

/// The release asset for this machine, or `None` where no binary is built.
pub fn asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("blyte-aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("blyte-x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("blyte-x86_64-unknown-linux-musl"),
        _ => None,
    }
}

/// `v0.2.1` or `0.2.1` as `(0, 2, 1)`.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.trim().trim_start_matches('v').split('.');
    let v = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts
            .next()?
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()?,
    );
    Some(v)
}

/// The GitHub CLI; `BLYTE_GH` names another (the tests use a stand-in).
fn gh() -> String {
    std::env::var("BLYTE_GH").unwrap_or_else(|_| "gh".to_string())
}

fn latest_tag() -> Result<String, String> {
    let out = Command::new(gh())
        .args([
            "release", "view", "--repo", REPO, "--json", "tagName", "--jq", ".tagName",
        ])
        .output()
        .map_err(|e| {
            format!("could not run the GitHub CLI (gh): {e}; install it and run `gh auth login`")
        })?;
    if !out.status.success() {
        return Err(format!(
            "gh could not read the latest release of {REPO}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let tag = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if parse_version(&tag).is_none() {
        return Err(format!("unexpected release tag: {tag:?}"));
    }
    Ok(tag)
}

/// The installed binary: this one, or its `blyte` sibling when this is `blyte.prev` (so a rollback
/// also works from the previous build).
fn current_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("cannot find this blyte binary: {e}"))?;
    Ok(if exe.file_name().is_some_and(|n| n == "blyte.prev") {
        exe.with_file_name("blyte")
    } else {
        exe
    })
}

fn prev_path(exe: &Path) -> PathBuf {
    exe.with_file_name("blyte.prev")
}

fn version_of(bin: &Path) -> Option<String> {
    let out = Command::new(bin).arg("--version").output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Swap `exe` and `blyte.prev`.
fn rollback(exe: &Path) -> Result<String, String> {
    let prev = prev_path(exe);
    if !prev.is_file() {
        return Err(format!(
            "no previous build to roll back to ({} is missing)",
            prev.display()
        ));
    }
    let swap = exe.with_file_name(".blyte-swap");
    std::fs::rename(exe, &swap).map_err(|e| format!("rollback failed: {e}"))?;
    std::fs::rename(&prev, exe).map_err(|e| format!("rollback failed: {e}"))?;
    std::fs::rename(&swap, &prev).map_err(|e| format!("rollback failed: {e}"))?;
    Ok(version_of(exe).unwrap_or_else(|| "blyte (version unknown)".into()))
}

/// Refresh the installed Claude Code skill with `new_exe` when the installed copy is this
/// version's own, unedited, keeping whether blyte is the default. Returns a line to print.
fn refresh_skill(new_exe: &Path) -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let skill = PathBuf::from(&home).join(".claude/skills/blyte-implementer/SKILL.md");
    let installed = std::fs::read_to_string(&skill).ok()?;
    if installed != crate::skill::SKILL_MD {
        return Some(format!(
            "skill not refreshed: {} has been edited (run `blyte skill install --force` to replace it)",
            skill.display()
        ));
    }
    let claude_md = PathBuf::from(&home).join(".claude/CLAUDE.md");
    let default_on = std::fs::read_to_string(claude_md)
        .map(|t| t.contains("<!-- blyte:default -->"))
        .unwrap_or(false);
    let mut args = vec!["skill", "install", "--force"];
    if !default_on {
        args.push("--no-default");
    }
    let ok = Command::new(new_exe)
        .args(&args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    Some(if ok {
        "skill refreshed (new Claude Code sessions pick it up)".to_string()
    } else {
        "skill not refreshed: `blyte skill install` failed; run it by hand".to_string()
    })
}

fn update(check: bool) -> Result<(), String> {
    let current = env!("CARGO_PKG_VERSION");
    let tag = latest_tag()?;
    let (now, latest) = (parse_version(current), parse_version(&tag));
    println!(
        "installed: blyte {current}\nlatest:    blyte {}",
        tag.trim_start_matches('v')
    );
    if latest <= now {
        println!("blyte is up to date");
        return Ok(());
    }
    if check {
        println!("run `blyte update` to install it");
        return Ok(());
    }
    let asset = asset_name().ok_or_else(|| {
        format!(
            "no release binary for {}-{}; build it with `cargo install --git https://github.com/{REPO} blyte`",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let exe = current_exe()?;
    let tmp = exe.with_file_name(format!(".blyte-update-{}", std::process::id()));
    let out = Command::new(gh())
        .args([
            "release",
            "download",
            &tag,
            "--repo",
            REPO,
            "--pattern",
            asset,
            "--clobber",
            "-O",
        ])
        .arg(&tmp)
        .output()
        .map_err(|e| format!("could not run the GitHub CLI (gh): {e}"))?;
    if !out.status.success() || !tmp.is_file() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "download failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("could not make the download executable: {e}"))?;
    }
    let Some(new_version) = version_of(&tmp) else {
        let _ = std::fs::remove_file(&tmp);
        return Err("the downloaded binary does not run; the installed blyte is unchanged".into());
    };
    let prev = prev_path(&exe);
    std::fs::copy(&exe, &prev).map_err(|e| format!("could not keep the previous build: {e}"))?;
    std::fs::rename(&tmp, &exe).map_err(|e| format!("could not replace {}: {e}", exe.display()))?;
    println!(
        "updated to {new_version}; the previous build is kept as {}",
        prev.display()
    );
    println!("`blyte update --rollback` restores it");
    if let Some(line) = refresh_skill(&exe) {
        println!("{line}");
    }
    Ok(())
}

/// `blyte update [--check | --rollback]`; returns the process exit code.
pub fn run(check: bool, rollback_requested: bool) -> i32 {
    let result = if rollback_requested {
        current_exe()
            .and_then(|exe| rollback(&exe))
            .map(|v| println!("rolled back to {v}"))
    } else {
        update(check)
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}
