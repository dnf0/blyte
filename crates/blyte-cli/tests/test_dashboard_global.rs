//! Every dispatch registers its repository, and the dashboard watches all of them from any folder,
//! with no configuration: from the wrong folder a colleague saw nothing (2026-09-30).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn blyte(dir: &Path, home: &Path, state: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_blyte"));
    c.current_dir(dir)
        .env("HOME", home)
        .env("BLYTE_STATE_DIR", state)
        .env_remove("XDG_STATE_HOME")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL");
    c
}

#[test]
fn a_dispatched_repository_shows_on_a_dashboard_started_elsewhere() {
    let t = tempfile::tempdir().unwrap();
    let (home, state, elsewhere) = (
        t.path().join("home"),
        t.path().join("state"),
        t.path().join("elsewhere"),
    );
    let repo = t.path().join("alpha");
    for d in [&home, &elsewhere, &repo] {
        std::fs::create_dir_all(d).unwrap();
    }
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("f.txt"), "a\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(repo.join("task.md"), "x").unwrap();

    // Nothing registered yet: the dashboard says so.
    let mut c = blyte(&elsewhere, &home, &state)
        .args(["dashboard", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(c.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    c.kill().unwrap();
    let _ = c.wait();
    let mut err = String::new();
    c.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(err.contains("nothing to watch yet"), "{err}");

    // A dispatch (it fails fast with no key) registers the repository.
    let out = blyte(&repo, &home, &state)
        .args([
            "dispatch",
            "G1",
            "task.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ])
        .output()
        .unwrap();
    assert!(
        !out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let listed = std::fs::read_to_string(state.join("repos")).unwrap();
    assert_eq!(listed.lines().count(), 1, "{listed}");
    // Registering again does not duplicate it.
    let _ = blyte(&repo, &home, &state)
        .args([
            "dispatch",
            "G2",
            "task.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ])
        .output()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(state.join("repos"))
            .unwrap()
            .lines()
            .count(),
        1
    );

    let mut c = blyte(&elsewhere, &home, &state)
        .args(["dashboard", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(c.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let addr = line
        .trim()
        .strip_prefix("blyte dashboard: http://")
        .unwrap()
        .to_string();
    let mut s = std::net::TcpStream::connect(&addr).unwrap();
    s.write_all(b"GET /api/jobs HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    c.kill().unwrap();
    let _ = c.wait();
    let mut err = String::new();
    c.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(
        err.contains("watching 1 repositories") && err.contains("dispatch history"),
        "{err}"
    );
    assert!(
        resp.contains("\"alpha/G1\"") && resp.contains("\"alpha/G2\""),
        "{resp}"
    );
}
