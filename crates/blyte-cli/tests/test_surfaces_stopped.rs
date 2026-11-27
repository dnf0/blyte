use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use blyte_cli::session::{Entry, EntryBody};
use blyte_cli::surface_dir::{folder_store, surface_dir, write_record_in, SurfaceRecord};
use blyte_cli::surface_registry::{write_entry, RegistryEntry};
use blyte_cli::surfaces_cmd::{
    format_surfaces_table, run_stopped_surfaces, stopped_surfaces, StoppedSurface,
};

struct MockHealthServer {
    port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockHealthServer {
    fn start(surface_id: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock health server");
        listener
            .set_nonblocking(true)
            .expect("set mock health server nonblocking");
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let body = format!("{{\"surface_id\":\"{surface_id}\"}}");

        let handle = thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                        let mut buf = [0u8; 512];
                        let _ = stream.read(&mut buf);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.flush();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            port,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for MockHealthServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn init_git_repo(dir: &Path) {
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-m", "init"]);
}

fn stopped_record(id: &str, name: &str, started_at: u64) -> SurfaceRecord {
    SurfaceRecord {
        id: id.to_string(),
        name: name.to_string(),
        pid: 999_999,
        port: 0,
        started_at,
        session_id: format!("sess-{id}"),
    }
}

fn user_entry(seq: u64, ts: u64, text: &str) -> Entry {
    Entry {
        seq,
        ts,
        body: EntryBody::UserMessage {
            text: text.to_string(),
            reports: Vec::new(),
            queued: false,
            attachments: Vec::new(),
            pastes: Vec::new(),
        },
    }
}

fn idle_entry(seq: u64, ts: u64) -> Entry {
    Entry {
        seq,
        ts,
        body: EntryBody::Idle {
            turns: 1,
            cost_usd: Some(0.01),
            seconds: Some(1.5),
            first_output_seconds: Some(0.2),
        },
    }
}

fn write_entries_jsonl(dir: &Path, entries: &[Entry]) {
    let mut raw = String::new();
    for entry in entries {
        raw.push_str(&serde_json::to_string(entry).unwrap());
        raw.push('\n');
    }
    std::fs::write(dir.join("entries.jsonl"), raw).unwrap();
}

#[test]
fn stopped_surfaces_returns_newest_first_with_last_used_and_last_message() {
    let state = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let older_id = "s00000000000000aa";
    let older_dir = surface_dir(repo.path(), older_id);
    write_record_in(
        &older_dir,
        &stopped_record(older_id, "older", 1_700_000_000),
    )
    .unwrap();
    write_entries_jsonl(
        &older_dir,
        &[
            user_entry(1, 1_700_000_100, "\n   first prompt  \nsecond line"),
            idle_entry(2, 1_700_000_200),
        ],
    );

    let newer_id = "s00000000000000bb";
    let newer_dir = surface_dir(repo.path(), newer_id);
    write_record_in(
        &newer_dir,
        &stopped_record(newer_id, "newer", 1_700_000_050),
    )
    .unwrap();
    write_entries_jsonl(
        &newer_dir,
        &[
            user_entry(1, 1_700_000_300, "earlier prompt"),
            idle_entry(2, 1_700_000_400),
            user_entry(3, 1_700_000_500, "latest prompt\nmore details"),
            idle_entry(4, 1_700_000_600),
        ],
    );

    let tie_id = "s00000000000000a0";
    let tie_dir = surface_dir(repo.path(), tie_id);
    write_record_in(&tie_dir, &stopped_record(tie_id, "tie", 1_700_000_010)).unwrap();
    write_entries_jsonl(
        &tie_dir,
        &[
            user_entry(1, 1_700_000_150, "   \n  \t  "),
            idle_entry(2, 1_700_000_200),
        ],
    );

    let expected = vec![
        StoppedSurface {
            id: newer_id.to_string(),
            name: "newer".to_string(),
            last_used: 1_700_000_600,
            last_message: Some("latest prompt".to_string()),
        },
        StoppedSurface {
            id: tie_id.to_string(),
            name: "tie".to_string(),
            last_used: 1_700_000_200,
            last_message: None,
        },
        StoppedSurface {
            id: older_id.to_string(),
            name: "older".to_string(),
            last_used: 1_700_000_200,
            last_message: Some("first prompt".to_string()),
        },
    ];

    let got = stopped_surfaces(repo.path(), state.path()).unwrap();
    assert_eq!(got, expected);

    let subdir = repo.path().join("nested/sub");
    std::fs::create_dir_all(&subdir).unwrap();
    assert_eq!(stopped_surfaces(&subdir, state.path()).unwrap(), expected);

    let wt_parent = tempfile::tempdir().unwrap();
    let wt_path = wt_parent.path().join("linked");
    let wt_out = Command::new("git")
        .args(["worktree", "add", "-b", "wt-branch"])
        .arg(&wt_path)
        .current_dir(repo.path())
        .output()
        .expect("git worktree add");
    assert!(wt_out.status.success());
    assert_eq!(stopped_surfaces(&wt_path, state.path()).unwrap(), expected);
}

#[test]
fn running_surface_is_left_out() {
    let state = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let running_id = "s00000000000000cc";
    let server = MockHealthServer::start(running_id);
    let running_dir = surface_dir(repo.path(), running_id);
    write_record_in(
        &running_dir,
        &SurfaceRecord {
            id: running_id.to_string(),
            name: "live-one".to_string(),
            pid: std::process::id(),
            port: server.port,
            started_at: 1_700_001_000,
            session_id: format!("sess-{running_id}"),
        },
    )
    .unwrap();
    write_entries_jsonl(
        &running_dir,
        &[user_entry(1, 1_700_001_100, "still running")],
    );

    let stopped_id = "s00000000000000dd";
    let stopped_dir = surface_dir(repo.path(), stopped_id);
    write_record_in(
        &stopped_dir,
        &stopped_record(stopped_id, "stopped-one", 1_700_000_500),
    )
    .unwrap();
    write_entries_jsonl(
        &stopped_dir,
        &[user_entry(1, 1_700_000_600, "done for now")],
    );

    let got = stopped_surfaces(repo.path(), state.path()).unwrap();
    assert_eq!(
        got,
        vec![StoppedSurface {
            id: stopped_id.to_string(),
            name: "stopped-one".to_string(),
            last_used: 1_700_000_600,
            last_message: Some("done for now".to_string()),
        }]
    );
}

#[test]
fn surface_without_entry_log_uses_started_at_and_has_no_message() {
    let state = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let absent_id = "s00000000000000e1";
    let absent_dir = surface_dir(repo.path(), absent_id);
    write_record_in(
        &absent_dir,
        &stopped_record(absent_id, "never-used", 1_709_164_800),
    )
    .unwrap();

    let empty_id = "s00000000000000e2";
    let empty_dir = surface_dir(repo.path(), empty_id);
    write_record_in(
        &empty_dir,
        &stopped_record(empty_id, "empty-log", 1_709_000_000),
    )
    .unwrap();
    std::fs::write(empty_dir.join("entries.jsonl"), "").unwrap();

    let got = stopped_surfaces(repo.path(), state.path()).unwrap();
    assert_eq!(
        got,
        vec![
            StoppedSurface {
                id: absent_id.to_string(),
                name: "never-used".to_string(),
                last_used: 1_709_164_800,
                last_message: None,
            },
            StoppedSurface {
                id: empty_id.to_string(),
                name: "empty-log".to_string(),
                last_used: 1_709_000_000,
                last_message: None,
            },
        ]
    );
}

#[test]
fn truncated_final_line_is_ignored_and_bad_complete_line_is_an_error() {
    let state = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let trunc_id = "s00000000000000f1";
    let trunc_dir = surface_dir(repo.path(), trunc_id);
    write_record_in(
        &trunc_dir,
        &stopped_record(trunc_id, "truncated", 1_700_000_000),
    )
    .unwrap();
    let e1 = user_entry(1, 1_700_000_100, "before crash");
    let e2 = idle_entry(2, 1_700_000_200);
    let partial = format!(
        "{}\n{}\n{{\"seq\":3,\"ts\":1700000999,\"kind\":\"user_mess",
        serde_json::to_string(&e1).unwrap(),
        serde_json::to_string(&e2).unwrap(),
    );
    std::fs::write(trunc_dir.join("entries.jsonl"), partial).unwrap();

    let got = stopped_surfaces(repo.path(), state.path()).unwrap();
    assert_eq!(
        got,
        vec![StoppedSurface {
            id: trunc_id.to_string(),
            name: "truncated".to_string(),
            last_used: 1_700_000_200,
            last_message: Some("before crash".to_string()),
        }]
    );

    let corrupt_path = trunc_dir.join("entries.jsonl");
    let bad = format!(
        "{}\n{{not valid json\n{}\n",
        serde_json::to_string(&e1).unwrap(),
        serde_json::to_string(&e2).unwrap(),
    );
    std::fs::write(&corrupt_path, bad).unwrap();

    let err = stopped_surfaces(repo.path(), state.path()).unwrap_err();
    assert!(
        err.contains(&corrupt_path.display().to_string()),
        "expected error to contain path {}, got: {err}",
        corrupt_path.display()
    );
    assert!(
        err.contains("line 2"),
        "expected error to contain line number, got: {err}"
    );
}

#[test]
fn table_and_json_output_match_exact_expected_format() {
    let state = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());
    let canonical_repo = repo.path().canonicalize().unwrap();

    let empty_table_out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["surfaces", "--stopped"])
        .current_dir(repo.path())
        .env("BLYTE_STATE_DIR", state.path())
        .output()
        .expect("run blyte surfaces --stopped");
    assert!(empty_table_out.status.success());
    assert_eq!(
        String::from_utf8(empty_table_out.stdout).unwrap(),
        format!("no stopped surfaces in {}\n", canonical_repo.display())
    );

    let empty_json_out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["surfaces", "--stopped", "--json"])
        .current_dir(repo.path())
        .env("BLYTE_STATE_DIR", state.path())
        .output()
        .expect("run blyte surfaces --stopped --json");
    assert!(empty_json_out.status.success());
    assert_eq!(String::from_utf8(empty_json_out.stdout).unwrap(), "[]\n");

    let long_msg = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-extra-tail";
    let first_60: String = long_msg.chars().take(60).collect();
    assert_eq!(first_60.len(), 60);

    let id_a = "s0000000000000101";
    let dir_a = surface_dir(repo.path(), id_a);
    write_record_in(&dir_a, &stopped_record(id_a, "alpha", 1_700_000_000)).unwrap();
    write_entries_jsonl(&dir_a, &[user_entry(1, 1_700_000_100, long_msg)]);

    let id_b = "s0000000000000102";
    let dir_b = surface_dir(repo.path(), id_b);
    write_record_in(&dir_b, &stopped_record(id_b, "payments-api", 1_709_164_800)).unwrap();

    let table_out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["surfaces", "--stopped"])
        .current_dir(repo.path())
        .env("BLYTE_STATE_DIR", state.path())
        .output()
        .expect("run blyte surfaces --stopped");
    assert!(
        table_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&table_out.stderr)
    );
    let expected_table = format!(
        concat!(
            "NAME          ID                 LAST USED         LAST MESSAGE\n",
            "payments-api  s0000000000000102  2024-02-29 00:00  -\n",
            "alpha         s0000000000000101  2023-11-14 22:15  {first_60}\n",
            "\n",
            "resume with: blyte --resume <name or id>\n",
        ),
        first_60 = first_60
    );
    assert_eq!(String::from_utf8(table_out.stdout).unwrap(), expected_table);

    let json_out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["surfaces", "--stopped", "--json"])
        .current_dir(repo.path())
        .env("BLYTE_STATE_DIR", state.path())
        .output()
        .expect("run blyte surfaces --stopped --json");
    assert!(
        json_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json_out.stderr)
    );
    let expected_json = format!(
        concat!(
            "[\n",
            "  {{\n",
            "    \"id\": \"s0000000000000102\",\n",
            "    \"name\": \"payments-api\",\n",
            "    \"last_used\": 1709164800,\n",
            "    \"last_message\": null\n",
            "  }},\n",
            "  {{\n",
            "    \"id\": \"s0000000000000101\",\n",
            "    \"name\": \"alpha\",\n",
            "    \"last_used\": 1700000100,\n",
            "    \"last_message\": \"{long_msg}\"\n",
            "  }}\n",
            "]\n",
        ),
        long_msg = long_msg
    );
    assert_eq!(String::from_utf8(json_out.stdout).unwrap(), expected_json);

    let mut buf_out = Vec::new();
    let mut buf_err = Vec::new();
    let code = run_stopped_surfaces(repo.path(), state.path(), false, &mut buf_out, &mut buf_err);
    assert_eq!(code, 0);
    assert!(buf_err.is_empty());
    assert_eq!(String::from_utf8(buf_out).unwrap(), expected_table);
}

#[test]
fn surfaces_without_stopped_prints_unchanged_output_for_same_state() {
    let state = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());

    let stopped_id = "s0000000000000201";
    let stopped_dir = surface_dir(repo.path(), stopped_id);
    write_record_in(
        &stopped_dir,
        &stopped_record(stopped_id, "old-work", 1_700_000_100),
    )
    .unwrap();
    write_entries_jsonl(&stopped_dir, &[user_entry(1, 1_700_000_200, "old prompt")]);

    let empty_running = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["surfaces"])
        .current_dir(repo.path())
        .env("BLYTE_STATE_DIR", state.path())
        .output()
        .expect("run blyte surfaces");
    assert!(empty_running.status.success());
    assert_eq!(
        String::from_utf8(empty_running.stdout).unwrap(),
        "no surfaces are running\n"
    );

    let live_id = "s0000000000000202";
    let server = MockHealthServer::start(live_id);
    let live_entry = RegistryEntry {
        id: live_id.to_string(),
        name: "active".to_string(),
        root: repo.path().canonicalize().unwrap(),
        branch: Some("main".to_string()),
        pid: std::process::id(),
        port: server.port,
        started_at: 1_700_001_000,
        doing: "working on tests".to_string(),
    };
    let reg_dir = state.path().join("surfaces");
    write_entry(&reg_dir, &live_entry).unwrap();

    let running_out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["surfaces"])
        .current_dir(repo.path())
        .env("BLYTE_STATE_DIR", state.path())
        .output()
        .expect("run blyte surfaces");
    assert!(running_out.status.success());
    assert_eq!(
        String::from_utf8(running_out.stdout).unwrap(),
        format_surfaces_table(&[live_entry], None)
    );
}

#[test]
fn non_git_folder_lists_stopped_surfaces_the_same_way() {
    let state = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let canonical_folder = folder.path().canonicalize().unwrap();

    let empty_out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["surfaces", "--stopped"])
        .current_dir(folder.path())
        .env("BLYTE_STATE_DIR", state.path())
        .output()
        .expect("run blyte surfaces --stopped in plain folder");
    assert!(empty_out.status.success());
    assert_eq!(
        String::from_utf8(empty_out.stdout).unwrap(),
        format!("no stopped surfaces in {}\n", canonical_folder.display())
    );

    let store_surfaces = folder_store(state.path(), folder.path())
        .unwrap()
        .join("surfaces");

    let live_id = "s0000000000000301";
    let server = MockHealthServer::start(live_id);
    write_record_in(
        &store_surfaces.join(live_id),
        &SurfaceRecord {
            id: live_id.to_string(),
            name: "running-plain".to_string(),
            pid: std::process::id(),
            port: server.port,
            started_at: 1_700_002_000,
            session_id: format!("sess-{live_id}"),
        },
    )
    .unwrap();

    let id_old = "s0000000000000302";
    let dir_old = store_surfaces.join(id_old);
    write_record_in(&dir_old, &stopped_record(id_old, "notes", 1_700_000_000)).unwrap();
    write_entries_jsonl(&dir_old, &[user_entry(1, 1_700_000_100, "draft notes")]);

    let id_new = "s0000000000000303";
    let dir_new = store_surfaces.join(id_new);
    write_record_in(&dir_new, &stopped_record(id_new, "review", 1_709_164_800)).unwrap();

    let got = stopped_surfaces(folder.path(), state.path()).unwrap();
    assert_eq!(
        got,
        vec![
            StoppedSurface {
                id: id_new.to_string(),
                name: "review".to_string(),
                last_used: 1_709_164_800,
                last_message: None,
            },
            StoppedSurface {
                id: id_old.to_string(),
                name: "notes".to_string(),
                last_used: 1_700_000_100,
                last_message: Some("draft notes".to_string()),
            },
        ]
    );

    let cli_out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["surfaces", "--stopped"])
        .current_dir(folder.path())
        .env("BLYTE_STATE_DIR", state.path())
        .output()
        .expect("run blyte surfaces --stopped in plain folder");
    assert!(cli_out.status.success());
    let expected_table = concat!(
        "NAME    ID                 LAST USED         LAST MESSAGE\n",
        "review  s0000000000000303  2024-02-29 00:00  -\n",
        "notes   s0000000000000302  2023-11-14 22:15  draft notes\n",
        "\n",
        "resume with: blyte --resume <name or id>\n",
    );
    assert_eq!(String::from_utf8(cli_out.stdout).unwrap(), expected_table);
}
