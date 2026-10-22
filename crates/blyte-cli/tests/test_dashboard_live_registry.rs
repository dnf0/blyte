//! A repository dispatched in after the dashboard started shows up without a restart (2026-10-01).

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

fn jobs(addr: &str) -> String {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.write_all(b"GET /api/jobs HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    resp
}

#[test]
fn a_repository_registered_later_appears_on_the_next_poll() {
    let t = tempfile::tempdir().unwrap();
    let (home, state, repo) = (
        t.path().join("home"),
        t.path().join("state"),
        t.path().join("late"),
    );
    for d in [&home, &repo] {
        std::fs::create_dir_all(d).unwrap();
    }
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("f.txt"), "a\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(repo.join("task.md"), "x").unwrap();

    let mut dash = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["dashboard", "--port", "0"])
        .current_dir(t.path())
        .env("HOME", &home)
        .env("BLYTE_STATE_DIR", &state)
        .env_remove("XDG_STATE_HOME")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(dash.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let addr = line
        .trim()
        .strip_prefix("blyte dashboard: http://")
        .unwrap()
        .to_string();
    assert!(!jobs(&addr).contains("\"late/L1\""));

    // The first dispatch in this repository, after the dashboard started (it fails fast: no key).
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "dispatch",
            "L1",
            "task.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ])
        .current_dir(&repo)
        .env("HOME", &home)
        .env("BLYTE_STATE_DIR", &state)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap();
    assert!(
        !out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let resp = jobs(&addr);
    dash.kill().unwrap();
    let _ = dash.wait();
    assert!(resp.contains("\"late/L1\""), "{resp}");
}
