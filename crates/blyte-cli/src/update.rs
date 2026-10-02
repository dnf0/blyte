//! `blyte update`: replace this binary with the latest GitHub release (2026-09-30).
//!
//! Releases carry one static binary per platform (release-on-merge.yml). The repository is the
//! `repository` field of the workspace Cargo.toml. The repository is private,
//! so the download goes through the GitHub CLI (`gh`), which already holds the user's access;
//! blyte handles no token. The new binary must run before it replaces the old one, the old one is
//! kept beside it as `blyte.prev` (`blyte update --rollback` swaps them back), and every installed
//! copy of the skill is refreshed when unedited, for every harness (2026-10-01).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository whose releases are installed. The repository is the `repository` field
/// of the workspace Cargo.toml.
pub const REPO: &str = match parse_repo(env!("CARGO_PKG_REPOSITORY")) {
    Some(repo) => repo,
    None => panic!(concat!(
        "the build's repository field is not a GitHub URL: ",
        env!("CARGO_PKG_REPOSITORY")
    )),
};

/// The repository whose releases are installed (`owner/name`).
pub const fn repo() -> &'static str {
    REPO
}

/// Parse a GitHub repository URL into an `owner/name` pair, stripping a leading
/// `https://github.com/` and any trailing `/` or `.git`. Returns `None` if the URL is not a
/// valid GitHub repository URL.
pub const fn parse_repo(url: &str) -> Option<&str> {
    let bytes = url.as_bytes();
    let prefix = b"https://github.com/";
    if bytes.len() < prefix.len() {
        return None;
    }
    let mut i = 0;
    while i < prefix.len() {
        if bytes[i] != prefix[i] {
            return None;
        }
        i += 1;
    }

    let mut end = bytes.len();
    while end > prefix.len() && bytes[end - 1] == b'/' {
        end -= 1;
    }
    if end >= prefix.len() + 4
        && bytes[end - 4] == b'.'
        && bytes[end - 3] == b'g'
        && bytes[end - 2] == b'i'
        && bytes[end - 1] == b't'
    {
        end -= 4;
        while end > prefix.len() && bytes[end - 1] == b'/' {
            end -= 1;
        }
    }

    let start = prefix.len();
    if end <= start {
        return None;
    }

    let mut slash_count = 0;
    let mut j = start;
    let mut slash_idx = 0;
    while j < end {
        let b = bytes[j];
        if b == b'/' {
            slash_count += 1;
            slash_idx = j;
        } else if b.is_ascii_whitespace() {
            return None;
        }
        j += 1;
    }

    if slash_count != 1 || slash_idx == start || slash_idx == end - 1 {
        return None;
    }

    let (_, tail) = match bytes.split_at_checked(start) {
        Some(pair) => pair,
        None => return None,
    };
    let (target, _) = match tail.split_at_checked(end - start) {
        Some(pair) => pair,
        None => return None,
    };
    match std::str::from_utf8(target) {
        Ok(s) => Some(s),
        Err(_) => None,
    }
}

/// Alias for [`parse_repo`].
pub const fn parse_repo_url(url: &str) -> Option<&str> {
    parse_repo(url)
}

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
    let repo = repo();
    let out = Command::new(gh())
        .args([
            "release", "view", "--repo", repo, "--json", "tagName", "--jq", ".tagName",
        ])
        .output()
        .map_err(|e| {
            format!("could not run the GitHub CLI (gh): {e}; install it and run `gh auth login`")
        })?;
    if !out.status.success() {
        return Err(format!(
            "gh could not read the latest release of {repo}: {}",
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

/// Refresh every installed skill copy with `new_exe` when unedited, keeping whether blyte is the default.
pub fn refresh_skills(new_exe: &Path, home: &Path) -> Vec<String> {
    let mut lines = Vec::new();

    struct HarnessConfig {
        name: &'static str,
        skill_dirs: Vec<PathBuf>,
        instructions_file: PathBuf,
        install_args: Vec<&'static str>,
    }

    let harnesses = [
        HarnessConfig {
            name: "Claude Code",
            skill_dirs: vec![home.join(".claude/skills/blyte-implementer")],
            instructions_file: home.join(".claude/CLAUDE.md"),
            install_args: vec!["skill", "install", "--force"],
        },
        HarnessConfig {
            name: "Codex",
            skill_dirs: vec![
                home.join(".agents/skills/blyte-implementer"),
                home.join(".codex/skills/blyte-implementer"),
            ],
            instructions_file: home.join(".codex/AGENTS.md"),
            install_args: vec!["skill", "install", "--force", "--harness", "codex"],
        },
    ];

    for harness in harnesses {
        let existing_skills: Vec<(PathBuf, PathBuf)> = harness
            .skill_dirs
            .iter()
            .map(|d| (d.join("SKILL.md"), d.join(".blyte-installed")))
            .filter(|(s, _)| s.exists())
            .collect();

        if existing_skills.is_empty() {
            continue;
        }

        let mut first_edited: Option<PathBuf> = None;
        for (skill_file, installed_file) in &existing_skills {
            let is_unedited = match std::fs::read_to_string(skill_file) {
                Ok(skill_text) => {
                    if installed_file.exists() {
                        std::fs::read_to_string(installed_file)
                            .map(|inst_text| inst_text == skill_text)
                            .unwrap_or(false)
                    } else {
                        skill_text == crate::skill::SKILL_MD
                    }
                }
                Err(_) => false,
            };

            if !is_unedited {
                first_edited = Some(skill_file.clone());
                break;
            }
        }

        if let Some(edited_path) = first_edited {
            lines.push(format!(
                "skill not refreshed for {}: {} has been edited (run blyte skill install --force to replace it)",
                harness.name,
                edited_path.display()
            ));
        } else {
            let default_on = std::fs::read_to_string(&harness.instructions_file)
                .map(|t| t.contains("<!-- blyte:default -->"))
                .unwrap_or(false);

            let mut args = harness.install_args.clone();
            if !default_on {
                args.push("--no-default");
            }

            let ok = Command::new(new_exe)
                .args(&args)
                .env("HOME", home)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            if ok {
                lines.push(format!("skill refreshed for {}", harness.name));
            } else {
                lines.push(format!(
                    "skill not refreshed for {}: blyte skill install failed; run it by hand",
                    harness.name
                ));
            }
        }
    }

    lines
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
    let repo = repo();
    let asset = asset_name().ok_or_else(|| {
        format!(
            "no release binary for {}-{}; build it with `cargo install --git https://github.com/{repo} blyte`",
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
            repo,
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
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for line in refresh_skills(&exe, &home) {
            println!("{line}");
        }
        if !home.as_os_str().is_empty() {
            crate::skill::warn_superseded(&home);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_maps_github_urls_and_rejects_non_github() {
        assert_eq!(
            parse_repo("https://github.com/acme/blyte"),
            Some("acme/blyte")
        );
        assert_eq!(
            parse_repo("https://github.com/acme/blyte/"),
            Some("acme/blyte")
        );
        assert_eq!(
            parse_repo("https://github.com/acme/blyte.git"),
            Some("acme/blyte")
        );
        assert_eq!(parse_repo("https://gitlab.com/acme/blyte"), None);
        assert_eq!(repo(), parse_repo(env!("CARGO_PKG_REPOSITORY")).unwrap());
    }
}
