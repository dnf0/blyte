//! `blyte update`: update this binary to the latest release (2026-09-30).
//!
//! Releases carry one static binary per platform (release-on-merge.yml). The repository is the
//! `repository` field of the workspace Cargo.toml. The latest release is read over HTTPS from the
//! GitHub API. If blyte was installed from crates.io via cargo it runs `cargo install blyte
//! --locked`; otherwise it downloads the platform binary over HTTPS and checks that it runs before
//! replacing the old one. The old binary is kept beside it as `blyte.prev` (`blyte update
//! --rollback` swaps them back), and every installed copy of the skill is refreshed when unedited,
//! for every harness (2026-10-01).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

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

/// How the running `blyte` binary was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    Cargo,
    Binary,
}

const CRATES_IO_GIT_SOURCE: &str = "(registry+https://github.com/rust-lang/crates.io-index)";
const CRATES_IO_SPARSE_SOURCE: &str = "(sparse+https://index.crates.io/)";

/// Determine whether `exe` was installed from crates.io with cargo or as a standalone binary.
pub fn install_method(exe: &Path, cargo_home: Option<&Path>) -> Result<InstallMethod, String> {
    let Some(cargo_home) = cargo_home else {
        return Ok(InstallMethod::Binary);
    };
    let Ok(canon_exe) = exe.canonicalize() else {
        return Ok(InstallMethod::Binary);
    };
    let cargo_bin = cargo_home.join("bin").join("blyte");
    let Ok(canon_cargo_bin) = cargo_bin.canonicalize() else {
        return Ok(InstallMethod::Binary);
    };
    if canon_exe != canon_cargo_bin {
        return Ok(InstallMethod::Binary);
    }

    let crates2 = cargo_home.join(".crates2.json");
    let text = match std::fs::read_to_string(&crates2) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "blyte is in {} but {} is missing, so how it was installed is unknown",
                cargo_home.join("bin").display(),
                crates2.display()
            ));
        }
        Err(e) => {
            return Err(format!("could not read {}: {e}", crates2.display()));
        }
    };
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("could not parse {}: {e}", crates2.display()))?;
    let installs = json
        .get("installs")
        .and_then(|v| v.as_object())
        .ok_or_else(|| format!("{} has no installs object", crates2.display()))?;
    let Some(key) = installs.keys().find(|k| k.starts_with("blyte ")) else {
        return Err(format!(
            "{} does not list blyte, so how it was installed is unknown",
            crates2.display()
        ));
    };
    if key.ends_with(CRATES_IO_GIT_SOURCE) || key.ends_with(CRATES_IO_SPARSE_SOURCE) {
        return Ok(InstallMethod::Cargo);
    }
    let source = key
        .split_once('(')
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap_or(key.as_str());
    Err(format!(
        "blyte was installed by cargo from {source}; update it with the cargo install command you used"
    ))
}

fn cargo_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CARGO_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(|h| PathBuf::from(h).join(".cargo"))
}

fn api_base() -> String {
    match std::env::var("BLYTE_UPDATE_API") {
        Ok(v) if !v.is_empty() => v.trim_end_matches('/').to_string(),
        _ => "https://api.github.com".to_string(),
    }
}

fn download_base() -> String {
    match std::env::var("BLYTE_UPDATE_DOWNLOAD") {
        Ok(v) if !v.is_empty() => v.trim_end_matches('/').to_string(),
        _ => "https://github.com".to_string(),
    }
}

fn cargo_bin() -> String {
    match std::env::var("BLYTE_CARGO") {
        Ok(v) if !v.is_empty() => v,
        _ => "cargo".to_string(),
    }
}

fn latest_tag() -> Result<String, String> {
    let repo = repo();
    let url = format!("{}/repos/{repo}/releases/latest", api_base());
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    let resp = agent
        .get(&url)
        .set("User-Agent", "blyte")
        .call()
        .map_err(|e| format!("could not read the latest release of {repo}: {e}"))?;
    let body: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("could not read the latest release of {repo}: {e}"))?;
    let tag = body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("the latest release of {repo} has no tag_name"))?
        .trim()
        .to_string();
    if parse_version(&tag).is_none() {
        return Err(format!("unexpected release tag: {tag:?}"));
    }
    Ok(tag)
}

fn download_release(repo: &str, tag: &str, asset: &str, tmp: &Path) -> Result<(), String> {
    let url = format!("{}/{repo}/releases/download/{tag}/{asset}", download_base());
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(600))
        .build();
    let resp = match agent.get(&url).set("User-Agent", "blyte").call() {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(tmp);
            return Err(format!("download failed: {e}"));
        }
    };
    let mut reader = resp.into_reader();
    let mut file = match std::fs::File::create(tmp) {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_file(tmp);
            return Err(format!("download failed: {e}"));
        }
    };
    if let Err(e) = std::io::copy(&mut reader, &mut file) {
        drop(file);
        let _ = std::fs::remove_file(tmp);
        return Err(format!("download failed: {e}"));
    }
    Ok(())
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

fn version_of(bin: &Path) -> Result<String, String> {
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("failed to run --version: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if stderr.is_empty() {
            return Err(format!("--version exited with {}", out.status));
        }
        return Err(format!("--version exited with {}: {stderr}", out.status));
    }
    let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if ver.is_empty() {
        return Err("--version produced empty output".to_string());
    }
    Ok(ver)
}

/// Swap `exe` and `blyte.prev`.
pub fn rollback(exe: &Path) -> Result<String, String> {
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
    version_of(exe).map_err(|e| {
        format!(
            "rollback binary at {} could not be verified: {e}",
            exe.display()
        )
    })
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

            let out = Command::new(new_exe).args(&args).env("HOME", home).output();

            match out {
                Ok(o) if o.status.success() => {
                    lines.push(format!("skill refreshed for {}", harness.name));
                    for line in String::from_utf8_lossy(&o.stdout).lines() {
                        if line.starts_with("auto-compact:") {
                            lines.push(line.to_string());
                        }
                    }
                }
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    let cause = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty());
                    match cause {
                        Some(c) => lines.push(format!(
                            "skill not refreshed for {}: blyte skill install failed; run it by hand ({c})",
                            harness.name
                        )),
                        None => lines.push(format!(
                            "skill not refreshed for {}: blyte skill install failed; run it by hand",
                            harness.name
                        )),
                    }
                }
                Err(e) => {
                    lines.push(format!(
                        "skill not refreshed for {}: blyte skill install failed; run it by hand ({e})",
                        harness.name
                    ));
                }
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
    let exe = current_exe()?;
    let prev = prev_path(&exe);
    let method = install_method(&exe, cargo_home().as_deref())?;
    let new_version = match method {
        InstallMethod::Cargo => {
            std::fs::copy(&exe, &prev)
                .map_err(|e| format!("could not keep the previous build: {e}"))?;
            let status = Command::new(cargo_bin())
                .args(["install", "blyte", "--locked"])
                .status()
                .map_err(|e| format!("could not run cargo: {e}"))?;
            if !status.success() {
                return Err("cargo install blyte --locked failed".to_string());
            }
            version_of(&exe)?
        }
        InstallMethod::Binary => {
            let repo = repo();
            let asset = asset_name().ok_or_else(|| {
                format!(
                    "no release binary for {}-{}; build it with `cargo install --git https://github.com/{repo} blyte`",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                )
            })?;
            let tmp = exe.with_file_name(format!(".blyte-update-{}", std::process::id()));
            download_release(repo, &tag, asset, &tmp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
                    .map_err(|e| format!("could not make the download executable: {e}"))?;
            }
            let Ok(new_version) = version_of(&tmp) else {
                let _ = std::fs::remove_file(&tmp);
                return Err(
                    "the downloaded binary does not run; the installed blyte is unchanged".into(),
                );
            };
            std::fs::copy(&exe, &prev)
                .map_err(|e| format!("could not keep the previous build: {e}"))?;
            std::fs::rename(&tmp, &exe)
                .map_err(|e| format!("could not replace {}: {e}", exe.display()))?;
            new_version
        }
    };
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

    #[cfg(unix)]
    #[test]
    fn rollback_errors_when_version_check_fails() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("blyte");
        let prev = dir.path().join("blyte.prev");
        std::fs::write(&exe, "#!/bin/sh\necho 'blyte 2.0.0'\n").unwrap();
        std::fs::write(&prev, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&prev, std::fs::Permissions::from_mode(0o755)).unwrap();

        let err = rollback(&exe).unwrap_err();
        assert!(err.contains("could not be verified"), "{err}");
        assert!(err.contains(&exe.display().to_string()), "{err}");
    }

    #[test]
    fn install_method_returns_binary_when_exe_elsewhere_or_no_cargo_home() {
        let dir = tempfile::tempdir().unwrap();
        let cargo_home = dir.path().join("cargo");
        std::fs::create_dir_all(cargo_home.join("bin")).unwrap();
        let elsewhere = dir.path().join("other/blyte");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, b"bin").unwrap();

        assert_eq!(
            install_method(&elsewhere, Some(&cargo_home)),
            Ok(InstallMethod::Binary)
        );
        assert_eq!(install_method(&elsewhere, None), Ok(InstallMethod::Binary));

        let cargo_bin = cargo_home.join("bin/blyte");
        std::fs::write(&cargo_bin, b"other").unwrap();
        assert_eq!(
            install_method(&elsewhere, Some(&cargo_home)),
            Ok(InstallMethod::Binary)
        );
    }

    #[test]
    fn install_method_detects_crates_io_git_registry() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("bin/blyte");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"bin").unwrap();
        std::fs::write(
            dir.path().join(".crates2.json"),
            r#"{"installs":{"blyte 0.74.1 (registry+https://github.com/rust-lang/crates.io-index)":{}}}"#,
        )
        .unwrap();

        assert_eq!(
            install_method(&exe, Some(dir.path())),
            Ok(InstallMethod::Cargo)
        );
    }

    #[test]
    fn install_method_detects_crates_io_sparse_registry() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("bin/blyte");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"bin").unwrap();
        std::fs::write(
            dir.path().join(".crates2.json"),
            r#"{"installs":{"blyte 0.74.1 (sparse+https://index.crates.io/)":{}}}"#,
        )
        .unwrap();

        assert_eq!(
            install_method(&exe, Some(dir.path())),
            Ok(InstallMethod::Cargo)
        );
    }

    #[test]
    fn install_method_rejects_git_install_source() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("bin/blyte");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"bin").unwrap();
        std::fs::write(
            dir.path().join(".crates2.json"),
            r#"{"installs":{"blyte 0.74.1 (git+https://github.com/dnf0/blyte#abc)":{}}}"#,
        )
        .unwrap();

        let err = install_method(&exe, Some(dir.path())).unwrap_err();
        assert!(err.contains("installed by cargo from"), "{err}");
    }

    #[test]
    fn install_method_errors_when_crates2_json_missing() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("bin/blyte");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"bin").unwrap();

        let err = install_method(&exe, Some(dir.path())).unwrap_err();
        assert!(err.contains("is missing"), "{err}");
    }

    #[test]
    fn install_method_errors_when_crates2_json_does_not_list_blyte() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("bin/blyte");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"bin").unwrap();
        std::fs::write(
            dir.path().join(".crates2.json"),
            r#"{"installs":{"blyte-agent 0.1.0 (registry+https://github.com/rust-lang/crates.io-index)":{}}}"#,
        )
        .unwrap();

        let err = install_method(&exe, Some(dir.path())).unwrap_err();
        assert!(err.contains("does not list blyte"), "{err}");
    }

    #[test]
    fn install_method_errors_when_crates2_json_unparseable_or_missing_installs() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("bin/blyte");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"bin").unwrap();
        let crates2 = dir.path().join(".crates2.json");

        std::fs::write(&crates2, "not json").unwrap();
        let err = install_method(&exe, Some(dir.path())).unwrap_err();
        assert!(err.contains(&crates2.display().to_string()), "{err}");

        std::fs::write(&crates2, r#"{"other":{}}"#).unwrap();
        let err = install_method(&exe, Some(dir.path())).unwrap_err();
        assert!(err.contains(&crates2.display().to_string()), "{err}");
    }
}
