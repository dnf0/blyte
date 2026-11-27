use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use blyte_agent::sandbox;
use blyte_cli::cli::Cli;
use blyte_cli::task_runner::{build_sandbox_profile, InPlace, TaskConfig};
use clap::Parser;
use serde_json::json;

fn sandbox_can_apply() -> Result<(), &'static str> {
    if !sandbox::available() {
        return Err("sandbox is unavailable");
    }
    #[cfg(target_os = "macos")]
    {
        let probe = Command::new("/usr/bin/sandbox-exec")
            .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
            .output();
        if let Ok(out) = probe {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.code() == Some(71) && stderr.contains("Operation not permitted") {
                return Err("nested sandbox-exec is not permitted in this environment");
            }
        }
    }
    Ok(())
}

fn scripted_openai_server(responses: Vec<String>) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let requests_bg = Arc::clone(&requests);
    std::thread::spawn(move || {
        for body in responses {
            let Some(mut stream) = listener.incoming().flatten().next() else {
                break;
            };
            requests_bg.fetch_add(1, Ordering::SeqCst);
            let mut buf = vec![0u8; 65536];
            let mut got = Vec::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got);
                if let Some(i) = text.find("\r\n\r\n") {
                    let len = text[..i]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if got.len() >= i + 4 + len {
                        break;
                    }
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.flush();
        }
    });
    (port, requests)
}

fn tool_call_response(id: &str, name: &str, args: serde_json::Value) -> String {
    json!({
        "id": id,
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": format!("call_{id}"),
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": args.to_string()
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15
        }
    })
    .to_string()
}

fn done_response(text: &str) -> String {
    json!({
        "id": "done",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": text
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15
        }
    })
    .to_string()
}

fn read_manifest(path: &Path) -> Vec<serde_json::Value> {
    if !path.exists() {
        return Vec::new();
    }
    let content = std::fs::read_to_string(path).unwrap();
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn run_blyte(workspace: &Path, port: u16, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .current_dir(workspace)
        .args(args)
        .env("OPENAI_API_KEY", "test-key")
        .env("OPENAI_BASE_URL", format!("http://127.0.0.1:{port}/v1"))
        .env("BLYTE_USAGE_LOG", "off")
        .output()
        .expect("spawn blyte")
}

#[test]
fn in_place_write_records_originals_and_reports_finished() {
    if let Err(reason) = sandbox_can_apply() {
        eprintln!("skipping in_place_write_records_originals_and_reports_finished: {reason}");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let before_dir = tmp.path().join("before");
    let manifest = tmp.path().join("manifest.jsonl");
    let report = tmp.path().join("report.json");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("a.txt"), "old content\n").unwrap();

    let (port, requests) = scripted_openai_server(vec![
        tool_call_response(
            "1",
            "write",
            json!({"path": "a.txt", "content": "new content\n"}),
        ),
        tool_call_response(
            "2",
            "write",
            json!({"path": "sub/b.txt", "content": "created content\n"}),
        ),
        done_response("all edits done"),
    ]);

    let out = run_blyte(
        &workspace,
        port,
        &[
            "edit files in place",
            "--model",
            "openai/test-model",
            "--snapshot-dir",
            before_dir.to_str().unwrap(),
            "--snapshot-manifest",
            manifest.to_str().unwrap(),
            "--report",
            report.to_str().unwrap(),
        ],
    );

    assert!(
        out.status.success(),
        "expected success, got {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(requests.load(Ordering::SeqCst), 3);

    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "new content\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("sub/b.txt")).unwrap(),
        "created content\n"
    );
    assert_eq!(
        std::fs::read_to_string(before_dir.join("a.txt")).unwrap(),
        "old content\n"
    );
    assert!(!before_dir.join("sub/b.txt").exists());

    let entries = read_manifest(&manifest);
    assert_eq!(entries.len(), 2, "manifest entries: {entries:?}");
    let a_entry = entries
        .iter()
        .find(|e| e["path"] == "a.txt")
        .expect("a.txt in manifest");
    assert_eq!(a_entry["existed"], json!(true));
    let b_entry = entries
        .iter()
        .find(|e| e["path"] == "sub/b.txt")
        .expect("sub/b.txt in manifest");
    assert_eq!(b_entry["existed"], json!(false));

    let report_val: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(report_val["outcome"], json!("finished"));
}

#[test]
fn in_place_shell_cannot_write_workspace() {
    if let Err(reason) = sandbox_can_apply() {
        eprintln!("skipping in_place_shell_cannot_write_workspace: {reason}");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let before_dir = tmp.path().join("before");
    let manifest = tmp.path().join("manifest.jsonl");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("a.txt"), "original\n").unwrap();

    let (port, _) = scripted_openai_server(vec![
        tool_call_response("1", "bash", json!({"command": "echo x > a.txt"})),
        done_response("attempted shell write"),
    ]);

    let out = run_blyte(
        &workspace,
        port,
        &[
            "try shell write",
            "--model",
            "openai/test-model",
            "--snapshot-dir",
            before_dir.to_str().unwrap(),
            "--snapshot-manifest",
            manifest.to_str().unwrap(),
        ],
    );

    assert!(
        out.status.success(),
        "expected success, got {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "original\n"
    );
    assert!(
        read_manifest(&manifest).is_empty(),
        "expected empty manifest after blocked shell write"
    );
}

#[test]
fn in_place_with_sandbox_off_fails_before_model_call() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let before_dir = tmp.path().join("before");
    let manifest = tmp.path().join("manifest.jsonl");
    std::fs::create_dir_all(&workspace).unwrap();

    let (port, requests) = scripted_openai_server(vec![done_response("should not be called")]);

    let out = run_blyte(
        &workspace,
        port,
        &[
            "edit files in place",
            "--model",
            "openai/test-model",
            "--sandbox",
            "off",
            "--snapshot-dir",
            before_dir.to_str().unwrap(),
            "--snapshot-manifest",
            manifest.to_str().unwrap(),
        ],
    );

    assert!(!out.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("in-place runs need the sandbox"),
        "stderr did not mention sandbox requirement: {stderr}"
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "no model call should be made when sandbox is off"
    );
}

#[test]
fn in_place_flag_usage_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let before_dir = tmp.path().join("before");
    let manifest = tmp.path().join("manifest.jsonl");
    let before_str = before_dir.to_str().unwrap();
    let manifest_str = manifest.to_str().unwrap();

    let cases: &[&[&str]] = &[
        &["edit files", "--snapshot-dir", before_str],
        &["edit files", "--snapshot-manifest", manifest_str],
        &[
            "edit files",
            "--snapshot-dir",
            before_str,
            "--snapshot-manifest",
            manifest_str,
            "--converse",
        ],
        &["edit files", "--snapshot-dir", before_str, "--converse"],
        &[
            "edit files",
            "--snapshot-manifest",
            manifest_str,
            "--converse",
        ],
        &[
            "edit files",
            "--snapshot-dir",
            before_str,
            "--snapshot-manifest",
            manifest_str,
            "--container",
            "cid123",
        ],
    ];

    for args in cases {
        let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
            .current_dir(tmp.path())
            .args(*args)
            .output()
            .expect("spawn blyte");
        assert!(
            !out.status.success(),
            "expected usage error for args {args:?}, stdout: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(
            out.status.code(),
            Some(2),
            "expected clap usage exit code 2 for args {args:?}, stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn in_place_second_run_preserves_first_original() {
    if let Err(reason) = sandbox_can_apply() {
        eprintln!("skipping in_place_second_run_preserves_first_original: {reason}");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let before_dir = tmp.path().join("before");
    let manifest = tmp.path().join("manifest.jsonl");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("a.txt"), "first original\n").unwrap();

    let (port1, _) = scripted_openai_server(vec![
        tool_call_response(
            "1",
            "write",
            json!({"path": "a.txt", "content": "after first run\n"}),
        ),
        done_response("first run done"),
    ]);

    let out1 = run_blyte(
        &workspace,
        port1,
        &[
            "first edit pass",
            "--model",
            "openai/test-model",
            "--snapshot-dir",
            before_dir.to_str().unwrap(),
            "--snapshot-manifest",
            manifest.to_str().unwrap(),
        ],
    );
    assert!(
        out1.status.success(),
        "first run failed: {}",
        String::from_utf8_lossy(&out1.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "after first run\n"
    );
    assert_eq!(
        std::fs::read_to_string(before_dir.join("a.txt")).unwrap(),
        "first original\n"
    );

    let (port2, _) = scripted_openai_server(vec![
        tool_call_response(
            "1",
            "write",
            json!({"path": "a.txt", "content": "after second run\n"}),
        ),
        done_response("second run done"),
    ]);

    let out2 = run_blyte(
        &workspace,
        port2,
        &[
            "second edit pass",
            "--model",
            "openai/test-model",
            "--snapshot-dir",
            before_dir.to_str().unwrap(),
            "--snapshot-manifest",
            manifest.to_str().unwrap(),
        ],
    );
    assert!(
        out2.status.success(),
        "second run failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "after second run\n"
    );
    assert_eq!(
        std::fs::read_to_string(before_dir.join("a.txt")).unwrap(),
        "first original\n"
    );

    let entries = read_manifest(&manifest);
    assert_eq!(entries.len(), 1, "manifest entries: {entries:?}");
    assert_eq!(entries[0]["path"], json!("a.txt"));
    assert_eq!(entries[0]["existed"], json!(true));
}

#[test]
fn in_place_cli_sets_in_place_and_disables_rollback() {
    let tmp = tempfile::tempdir().unwrap();
    let before_dir = tmp.path().join("before");
    let manifest = tmp.path().join("manifest.jsonl");
    let cli = Cli::try_parse_from([
        "blyte",
        "edit files in place",
        "--rollback",
        "--snapshot-dir",
        before_dir.to_str().unwrap(),
        "--snapshot-manifest",
        manifest.to_str().unwrap(),
    ])
    .expect("parse cli");
    let config = TaskConfig::from(&cli);
    assert_eq!(
        config.in_place,
        Some(InPlace {
            before_dir,
            manifest,
        })
    );
    assert!(!config.rollback);
}

#[test]
fn in_place_snapshot_fs_failure_fails_before_model_call() {
    if !sandbox::available() {
        eprintln!(
            "skipping in_place_snapshot_fs_failure_fails_before_model_call: sandbox is unavailable"
        );
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let not_a_dir = tmp.path().join("before_file");
    let manifest = tmp.path().join("manifest.jsonl");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(&not_a_dir, "blocking file").unwrap();

    let (port, requests) = scripted_openai_server(vec![done_response("should not be called")]);

    let out = run_blyte(
        &workspace,
        port,
        &[
            "edit files in place",
            "--model",
            "openai/test-model",
            "--snapshot-dir",
            not_a_dir.to_str().unwrap(),
            "--snapshot-manifest",
            manifest.to_str().unwrap(),
        ],
    );

    assert!(!out.status.success(), "expected non-zero exit");
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "no model call should be made when SnapshotFs::new fails"
    );
}

#[test]
fn in_place_sandbox_profile_forbids_workspace_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let before_dir = tmp.path().join("before");
    let manifest = tmp.path().join("manifest.jsonl");
    std::fs::create_dir_all(&workspace).unwrap();

    let cli = Cli::try_parse_from([
        "blyte",
        "edit files in place",
        "--snapshot-dir",
        before_dir.to_str().unwrap(),
        "--snapshot-manifest",
        manifest.to_str().unwrap(),
    ])
    .expect("parse cli");
    let mut config = TaskConfig::from(&cli);
    config.root = workspace.clone();

    let root_canon = workspace.canonicalize().unwrap();
    match build_sandbox_profile(&config).unwrap() {
        sandbox::Profile::Seatbelt(s) => {
            assert!(
                !s.contains(&format!(
                    "(allow file-write* (subpath \"{}\"))",
                    root_canon.display()
                )),
                "seatbelt profile must not allow writes to workspace: {s}"
            );
        }
        sandbox::Profile::LandlockConfined { write, .. }
        | sandbox::Profile::Landlock(write)
        | sandbox::Profile::LandlockOffline(write) => {
            assert!(
                !write
                    .iter()
                    .any(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()) == root_canon),
                "landlock profile must not allow writes to workspace: {write:?}"
            );
        }
    }
}
