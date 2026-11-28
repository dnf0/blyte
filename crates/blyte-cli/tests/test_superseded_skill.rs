//! Tests for warning about a superseded `blyte-research` user skill.
//!
//! An older, separately installed skill told sessions to "keep --max-turns tight (80)",
//! contradicting the bundled `blyte-implementer` skill. `blyte skill install` and
//! `blyte update` warn the user to remove it, without removing it automatically.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use blyte_cli::skill::{superseded_skills, superseded_warning, SKILL_MD};
use blyte_cli::update::{asset_name, repo};
use sha2::{Digest, Sha256};

fn blyte(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
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
    let release_bytes = format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'blyte 9.9.9 (test)'; exit 0; fi\necho \"$@\" >> {}\n",
        log.display()
    )
    .into_bytes();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let repo_name = repo();
    let asset = asset_name().expect("test platform has a release asset");
    let latest_path = format!("/repos/{repo_name}/releases/latest");
    let download_path = format!("/{repo_name}/releases/download/v9.9.9/{asset}");
    let checksum_path = format!("/{repo_name}/releases/download/v9.9.9/{asset}.sha256");
    let digest = Sha256::digest(&release_bytes);
    let mut digest_hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut digest_hex, "{byte:02x}");
    }
    let checksum_body = format!("{digest_hex}  {asset}\n");
    let handle = std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            if stop_flag.load(Ordering::SeqCst) {
                break;
            }
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let Ok(cloned) = stream.try_clone() else {
                continue;
            };
            let mut reader = BufReader::new(cloned);
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
                continue;
            }
            loop {
                let mut header = String::new();
                match reader.read_line(&mut header) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if header == "\r\n" || header == "\n" => break,
                    Ok(_) => {}
                }
            }
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or("");
            let path = parts.next().unwrap_or("");
            if method == "GET" && path == latest_path {
                let body = r#"{"tag_name":"v9.9.9"}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            } else if method == "GET" && path == download_path {
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    release_bytes.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&release_bytes);
            } else if method == "GET" && path == checksum_path {
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    checksum_body.len(),
                    checksum_body
                );
                let _ = stream.write_all(response.as_bytes());
            } else {
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
            let _ = stream.flush();
        }
    });

    let cargo_home = dir.path().join("cargo-home");
    std::fs::create_dir_all(&cargo_home).unwrap();
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

    let url = format!("http://{addr}");
    let out = Command::new(&bin)
        .arg("update")
        .env("BLYTE_UPDATE_API", &url)
        .env("BLYTE_UPDATE_DOWNLOAD", &url)
        .env("CARGO_HOME", &cargo_home)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("BLYTE_ENV_FILE", "")
        .output()
        .unwrap();

    stop.store(true, Ordering::SeqCst);
    let _ = TcpStream::connect(addr);
    let _ = handle.join();

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
