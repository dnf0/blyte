use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::Sources;
use blyte_cli::mock::route;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn write_file(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn empty_sources() -> Sources {
    Sources {
        roots: vec![],
        runs_dirs: vec![],
        hours: 0,
    }
}

#[test]
fn route_root_serves_mock_page() {
    let src = empty_sources();
    for path in ["/", "/index.html"] {
        let (code, ctype, body) = route(path, &src, now(), 7);
        assert_eq!(code, 200);
        assert_eq!(ctype, "text/html; charset=utf-8");
        let html = String::from_utf8(body).unwrap();
        assert!(html.contains("id=\"queue\""), "{path} missing id=\"queue\"");
    }
}

#[test]
fn route_api_replay_returns_finished_run_and_window_days() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let started_at = t - 500;
    let wt = d.path().join("repos/repo-a/.wt");

    write_file(
        &wt.join("M99.start.json"),
        &format!(r#"{{"id":"M99","pid":999999,"started_at":{started_at}}}"#),
    );
    write_file(&wt.join("M99.log"), "[Turn 2] \n");
    write_file(
        &wt.join("M99.json"),
        r#"{"outcome":"verified","seconds":120,"commits":[],"changed_files":["src/lib.rs"]}"#,
    );
    write_file(
        &wt.join("M99.checks.json"),
        &format!(r#"{{"ts":{},"flags":[]}}"#, t - 300),
    );

    let src = Sources {
        roots: vec![d.path().join("repos")],
        runs_dirs: vec![],
        hours: 0,
    };

    let (code, ctype, body) = route("/api/replay", &src, t, 14);
    assert_eq!(code, 200);
    assert_eq!(ctype, "application/json");

    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["window_days"], 14);
    let runs = parsed["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"], "M99");
}

#[test]
fn route_serves_assets_and_404_for_unknown() {
    let src = empty_sources();
    let t = now();

    for (path, expected_ctype) in [
        ("/favicon.svg", "image/svg+xml"),
        ("/fonts/archivo.woff2", "font/woff2"),
        ("/fonts/plex-mono-400.woff2", "font/woff2"),
        ("/fonts/plex-mono-500.woff2", "font/woff2"),
    ] {
        let (code, ctype, body) = route(path, &src, t, 7);
        assert_eq!(code, 200, "{path}");
        assert_eq!(ctype, expected_ctype, "{path}");
        assert!(!body.is_empty(), "{path} body was empty");
    }

    let (code, _, _) = route("/nope", &src, t, 7);
    assert_eq!(code, 404);
}

#[test]
fn cli_rejects_out_of_range_days() {
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["mock", "--days", "0"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains('0'), "stderr did not contain 0: {stderr}");

    let out_high = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["mock", "--days", "32"])
        .output()
        .unwrap();
    assert!(!out_high.status.success());
    let stderr_high = String::from_utf8_lossy(&out_high.stderr);
    assert!(
        stderr_high.contains("32"),
        "stderr did not contain 32: {stderr_high}"
    );
}

#[test]
fn cli_port_in_use_exits_one_and_names_port() {
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = occupied.local_addr().unwrap().port();

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["mock", "--port", &port.to_string()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&port.to_string()),
        "stderr did not name port {port}: {stderr}"
    );
}

#[test]
fn cli_serves_replay_with_no_store_and_get_only() {
    let d = tempfile::tempdir().unwrap();
    let t = now();
    let wt = d.path().join("repos/repo-a/.wt");
    write_file(
        &wt.join("R1.start.json"),
        &format!(r#"{{"id":"R1","pid":999999,"started_at":{}}}"#, t - 200),
    );
    write_file(&wt.join("R1.log"), "[Turn 1] \n");
    write_file(
        &wt.join("R1.json"),
        r#"{"outcome":"verified","seconds":30,"commits":[],"changed_files":[]}"#,
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "mock",
            "--port",
            "0",
            "--days",
            "5",
            "--root",
            d.path().join("repos").to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let addr = line
        .trim()
        .strip_prefix("blyte mock: http://")
        .unwrap_or_else(|| panic!("unexpected stdout line: {line:?}"))
        .to_string();

    let mut s = TcpStream::connect(&addr).unwrap();
    s.write_all(b"GET /api/replay HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();

    let mut s_post = TcpStream::connect(&addr).unwrap();
    s_post
        .write_all(b"POST /api/replay HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut post_resp = String::new();
    s_post.read_to_string(&mut post_resp).unwrap();

    child.kill().unwrap();
    let _ = child.wait();

    assert!(resp.starts_with("HTTP/1.1 200 OK\r\n"), "{resp}");
    assert!(resp.contains("Cache-Control: no-store\r\n"), "{resp}");
    assert!(resp.contains("\"window_days\":5"), "{resp}");
    assert!(resp.contains("\"id\":\"R1\""), "{resp}");
    assert!(
        post_resp.starts_with("HTTP/1.1 404 Not Found\r\n"),
        "{post_resp}"
    );
}

fn git_init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success());
}

fn write_rust_crate(dir: &Path, crate_name: &str) {
    write_file(
        &dir.join("Cargo.toml"),
        &format!("[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\n"),
    );
    write_file(&dir.join("src/lib.rs"), "pub fn hello() {}\n");
}

#[test]
fn route_api_map_returns_the_map() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo");
    git_init(&repo);
    write_rust_crate(&repo, "demo");

    let src = Sources {
        roots: vec![repo],
        runs_dirs: vec![],
        hours: 0,
    };

    let (code, ctype, body) = route("/api/map?repo=demo", &src, now(), 7);
    assert_eq!(code, 200);
    assert_eq!(ctype, "application/json");

    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["repo"], "demo");
    assert_eq!(parsed["units"].as_array().unwrap().len(), 1);
}

#[test]
fn route_api_map_without_repo_is_400() {
    let src = empty_sources();
    for path in ["/api/map", "/api/map?repo="] {
        let (code, ctype, body) = route(path, &src, now(), 7);
        assert_eq!(code, 400, "{path}");
        assert_eq!(ctype, "application/json", "{path}");
        assert_eq!(
            body, br#"{"error":"missing query parameter: repo"}"#,
            "{path}"
        );
    }
}

#[test]
fn route_api_map_unknown_repo_is_404() {
    let src = empty_sources();
    let (code, ctype, body) = route("/api/map?repo=ghost", &src, now(), 7);
    assert_eq!(code, 404);
    assert_eq!(ctype, "application/json");
    assert_eq!(
        body,
        br#"{"error":"no repository named ghost under the mock's roots"}"#
    );
}

#[test]
fn route_api_map_unsupported_repo_is_422() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("docs-only");
    git_init(&repo);
    write_file(&repo.join("README.md"), "# docs\n");

    let src = Sources {
        roots: vec![repo],
        runs_dirs: vec![],
        hours: 0,
    };

    let (code, ctype, body) = route("/api/map?repo=docs-only", &src, now(), 7);
    assert_eq!(code, 422);
    assert_eq!(ctype, "application/json");

    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let err = parsed["error"].as_str().unwrap();
    assert!(
        err.starts_with("no supported language in"),
        "unexpected error: {err}"
    );
}

#[test]
fn route_api_map_decodes_the_name() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("my repo");
    git_init(&repo);
    write_rust_crate(&repo, "my_repo");

    let src = Sources {
        roots: vec![repo],
        runs_dirs: vec![],
        hours: 0,
    };

    let (code, ctype, body) = route("/api/map?repo=my%20repo", &src, now(), 7);
    assert_eq!(code, 200);
    assert_eq!(ctype, "application/json");

    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["repo"], "my repo");
}

#[test]
fn route_api_map_prefers_git_dir_then_path_order() {
    let d = tempfile::tempdir().unwrap();
    let repo_a = d.path().join("a/demo");
    let repo_b = d.path().join("b/demo");
    write_rust_crate(&repo_a, "from_a");
    git_init(&repo_b);
    write_rust_crate(&repo_b, "from_b");

    let src = Sources {
        roots: vec![repo_a.clone(), repo_b.clone()],
        runs_dirs: vec![],
        hours: 0,
    };
    let (code, _, body) = route("/api/map?repo=demo", &src, now(), 7);
    assert_eq!(code, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["units"][0]["id"], "from_b");

    git_init(&repo_a);
    let src_both = Sources {
        roots: vec![repo_b, repo_a],
        runs_dirs: vec![],
        hours: 0,
    };
    let (code, _, body) = route("/api/map?repo=demo", &src_both, now(), 7);
    assert_eq!(code, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["units"][0]["id"], "from_a");

    let plain_x = d.path().join("x/plain");
    let plain_y = d.path().join("y/plain");
    write_rust_crate(&plain_x, "from_x");
    write_rust_crate(&plain_y, "from_y");
    let src_none = Sources {
        roots: vec![plain_y, plain_x],
        runs_dirs: vec![],
        hours: 0,
    };
    let (code, _, body) = route("/api/map?repo=plain", &src_none, now(), 7);
    assert_eq!(code, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["units"][0]["id"], "from_x");
}

#[test]
fn cli_serves_map_with_no_store() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo");
    git_init(&repo);
    write_rust_crate(&repo, "demo");

    let mut child = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["mock", "--port", "0", "--root", repo.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let addr = line
        .trim()
        .strip_prefix("blyte mock: http://")
        .unwrap_or_else(|| panic!("unexpected stdout line: {line:?}"))
        .to_string();

    let mut s = TcpStream::connect(&addr).unwrap();
    s.write_all(b"GET /api/map?repo=demo HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();

    let mut s_post = TcpStream::connect(&addr).unwrap();
    s_post
        .write_all(b"POST /api/map?repo=demo HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut post_resp = String::new();
    s_post.read_to_string(&mut post_resp).unwrap();

    child.kill().unwrap();
    let _ = child.wait();

    assert!(resp.starts_with("HTTP/1.1 200 OK\r\n"), "{resp}");
    assert!(resp.contains("Cache-Control: no-store\r\n"), "{resp}");
    assert!(resp.contains("\"repo\":\"demo\""), "{resp}");
    assert!(
        post_resp.starts_with("HTTP/1.1 404 Not Found\r\n"),
        "{post_resp}"
    );
}
