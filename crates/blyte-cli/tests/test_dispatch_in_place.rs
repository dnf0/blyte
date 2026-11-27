use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use blyte_agent::sandbox;
use blyte_cli::dispatch::exit_code;
use blyte_cli::dispatch_in_place::{
    in_place_outcome, records_dir_for, unix_timestamp_secs, wrap_in_place_research,
    wrap_in_place_task, IN_PLACE_RULES, NON_GIT_RESEARCH_NOTE,
};

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

fn read_http_body(stream: &mut TcpStream) -> String {
    let mut buf = vec![0u8; 65536];
    let mut got = Vec::new();
    let mut header_end = None;
    let mut content_len = 0usize;
    loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
        if header_end.is_none() {
            let text = String::from_utf8_lossy(&got);
            if let Some(i) = text.find("\r\n\r\n") {
                header_end = Some(i + 4);
                content_len = text[..i]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
            }
        }
        if let Some(end) = header_end {
            if got.len() >= end + content_len {
                break;
            }
        }
    }
    match header_end {
        Some(end) => String::from_utf8_lossy(&got[end..]).into_owned(),
        None => String::new(),
    }
}

fn start_scripted_server(turns: Vec<serde_json::Value>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let idx = AtomicUsize::new(0);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = read_http_body(&mut stream);
            let i = idx.fetch_add(1, Ordering::SeqCst);
            let body = turns
                .get(i)
                .unwrap_or_else(|| turns.last().unwrap())
                .to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

fn write_tool_turn(path: &str, content: &str) -> serde_json::Value {
    let args = serde_json::json!({
        "path": path,
        "content": content,
    })
    .to_string();
    serde_json::json!({
        "id": "r-write",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "write",
                        "arguments": args,
                    }
                }]
            },
            "finish_reason": "tool_calls",
        }],
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 30,
            "total_tokens": 130,
        }
    })
}

fn stop_turn(message: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "r-done",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": message,
            },
            "finish_reason": "stop",
        }],
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "total_tokens": 120,
        }
    })
}

struct TestEnv {
    _tmp: tempfile::TempDir,
    folder: PathBuf,
    home: PathBuf,
    state_dir: PathBuf,
    task_file: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("folder");
        let home = tmp.path().join("home");
        let state_dir = tmp.path().join("state");
        let task_file = tmp.path().join("task.md");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(&task_file, "Do the task.\n").unwrap();
        Self {
            _tmp: tmp,
            folder,
            home,
            state_dir,
            task_file,
        }
    }

    fn canonical_folder(&self) -> PathBuf {
        self.folder.canonicalize().unwrap()
    }

    fn run_store(&self) -> PathBuf {
        blyte_cli::surface_dir::run_store(&self.state_dir, &self.canonical_folder()).unwrap()
    }

    fn lock_path(&self) -> PathBuf {
        blyte_cli::surface_dir::in_place_lock_path(&self.state_dir, &self.canonical_folder())
            .unwrap()
    }

    fn blyte(&self, args: &[&str], port: Option<u16>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
        cmd.args(args)
            .current_dir(&self.folder)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.home)
            .env("BLYTE_STATE_DIR", &self.state_dir)
            .env("BLYTE_NO_UPDATE_CHECK", "1")
            .env("BLYTE_NO_KEEP_AWAKE", "1")
            .env("BLYTE_USAGE_LOG", "off")
            .env("BLYTE_GATE_SLOT_DIR", self.home.join("slots"))
            .env("BLYTE_GATE_SLOTS", "16")
            .env("BLYTE_ENV_FILE", "")
            .env_remove("GEMINI_API_KEY")
            .env_remove("GOOGLE_API_KEY")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("OLLAMA_HOST")
            .env_remove("OLLAMA_API_KEY")
            .env_remove("OPENAI_BASE_URL")
            .env_remove("OPENAI_API_KEY")
            .env_remove("BLYTE_MODEL")
            .env_remove("BLYTE_MODEL_SHELL");
        if let Some(p) = port {
            cmd.env("OPENAI_BASE_URL", format!("http://127.0.0.1:{p}/v1"))
                .env("OPENAI_API_KEY", "test")
                .env("BLYTE_MODEL", "openai/gemini-3.8-flash");
        }
        cmd.output().unwrap()
    }
}

fn dir_entries(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn overwrite_file_finishes_with_diff_and_clean_folder() {
    if let Err(reason) = sandbox_can_apply() {
        eprintln!("skipping overwrite_file_finishes_with_diff_and_clean_folder: {reason}");
        return;
    }

    let env = TestEnv::new();
    std::fs::write(env.folder.join("a.txt"), "old line\n").unwrap();
    let port = start_scripted_server(vec![
        write_tool_turn("a.txt", "new line\n"),
        stop_turn("updated a.txt"),
    ]);

    let out = env.blyte(
        &["dispatch", "T1", env.task_file.to_str().unwrap()],
        Some(port),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(summary["outcome"], "finished");
    assert_eq!(summary["branch"], "");
    assert_eq!(summary["commits"], serde_json::json!([]));
    assert_eq!(summary["changed_files"], serde_json::json!(["a.txt"]));
    assert_eq!(summary["gate_source"], "none");
    assert_eq!(
        summary["worktree"],
        env.canonical_folder().display().to_string()
    );

    assert_eq!(
        std::fs::read_to_string(env.folder.join("a.txt")).unwrap(),
        "new line\n"
    );
    assert!(!env.folder.join(".wt").exists());
    assert_eq!(
        dir_entries(&env.folder),
        BTreeSet::from(["a.txt".to_string()])
    );

    let store = env.run_store();
    let diff = std::fs::read_to_string(store.join("T1.diff")).unwrap();
    assert!(diff.contains("-old line"), "diff missing old line: {diff}");
    assert!(diff.contains("+new line"), "diff missing new line: {diff}");

    let start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(store.join("T1.start.json")).unwrap())
            .unwrap();
    assert_eq!(start["in_place"], true);
    assert_eq!(start["branch"], "");
    assert!(start["base"].is_null());
    assert!(start["base_sha"].is_null());
    assert_eq!(
        start["worktree"],
        env.canonical_folder().display().to_string()
    );

    let status = env.blyte(&["status", "T1"], None);
    assert_eq!(status.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&status.stdout).contains("outcome: finished"));

    let wait = env.blyte(&["wait", "T1"], None);
    assert_eq!(wait.status.code(), Some(0));
    let wait_out = String::from_utf8_lossy(&wait.stdout);
    assert!(wait_out.contains("T1") && wait_out.contains("finished"));

    let reuse = env.blyte(&["dispatch", "T1", env.task_file.to_str().unwrap()], None);
    assert_eq!(reuse.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&reuse.stderr).contains("already finished here"));
}

#[test]
fn no_changes_returns_exit_4() {
    if let Err(reason) = sandbox_can_apply() {
        eprintln!("skipping no_changes_returns_exit_4: {reason}");
        return;
    }

    let env = TestEnv::new();
    std::fs::write(env.folder.join("a.txt"), "unchanged\n").unwrap();
    let port = start_scripted_server(vec![stop_turn("nothing to change")]);

    let out = env.blyte(
        &["dispatch", "T1", env.task_file.to_str().unwrap()],
        Some(port),
    );
    assert_eq!(
        out.status.code(),
        Some(4),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(summary["outcome"], "no_changes");
    assert_eq!(summary["changed_files"], serde_json::json!([]));
}

#[test]
fn gate_flag_with_created_file_returns_verified() {
    if let Err(reason) = sandbox_can_apply() {
        eprintln!("skipping gate_flag_with_created_file_returns_verified: {reason}");
        return;
    }

    let env = TestEnv::new();
    let port = start_scripted_server(vec![
        write_tool_turn("made.txt", "created\n"),
        stop_turn("created made.txt"),
    ]);

    let out = env.blyte(
        &[
            "dispatch",
            "T1",
            env.task_file.to_str().unwrap(),
            "--gate",
            "test -f made.txt",
        ],
        Some(port),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(summary["outcome"], "verified");
    assert_eq!(summary["changed_files"], serde_json::json!(["made.txt"]));
    assert_eq!(summary["gate_source"], "flag");
}

#[test]
fn second_dispatch_while_locked_refuses_without_start_record() {
    let env = TestEnv::new();
    let lock_path = env.lock_path();
    let holder = blyte_agent::folder_lock::Holder {
        pid: std::process::id(),
        kind: "run".to_string(),
        id: "HOLDER1".to_string(),
    };
    let _lock = blyte_agent::folder_lock::acquire(&lock_path, &holder).unwrap();

    let out = env.blyte(&["dispatch", "T2", env.task_file.to_str().unwrap()], None);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("HOLDER1"),
        "stderr should name holder id: {stderr}"
    );
    assert!(!env.run_store().join("T2.start.json").exists());
}

#[test]
fn reusing_finished_id_is_refused() {
    let env = TestEnv::new();
    let store = env.run_store();
    std::fs::write(
        store.join("T1.json"),
        serde_json::json!({"id": "T1", "outcome": "finished"}).to_string(),
    )
    .unwrap();

    let out = env.blyte(&["dispatch", "T1", env.task_file.to_str().unwrap()], None);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already finished here"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn base_and_research_base_flags_are_refused() {
    let env = TestEnv::new();

    let research_base = env.blyte(
        &[
            "dispatch",
            "R1",
            env.task_file.to_str().unwrap(),
            "--research",
            "--base",
            "main",
        ],
        None,
    );
    assert_eq!(research_base.status.code(), Some(2));

    let base = env.blyte(
        &[
            "dispatch",
            "B1",
            env.task_file.to_str().unwrap(),
            "--base",
            "main",
        ],
        None,
    );
    assert_eq!(base.status.code(), Some(2));
    let base_stderr = String::from_utf8_lossy(&base.stderr);
    assert!(
        base_stderr.contains("--base needs a git repository"),
        "unexpected stderr: {base_stderr}"
    );
}

#[test]
fn status_and_wait_read_in_place_store_records() {
    let env = TestEnv::new();
    let store = env.run_store();
    std::fs::write(
        store.join("T1.start.json"),
        serde_json::json!({
            "id": "T1",
            "pid": 999999,
            "started_at": 1700000000,
            "in_place": true,
            "worktree": env.canonical_folder().display().to_string(),
            "branch": "",
            "base": null,
            "base_sha": null
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        store.join("T1.report.json"),
        serde_json::json!({
            "outcome": "finished",
            "turns": 2,
            "cost_usd": 0.01,
            "duration_seconds": 3.0,
            "final_message": "updated a.txt"
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        store.join("T1.json"),
        serde_json::json!({
            "id": "T1",
            "worktree": env.canonical_folder().display().to_string(),
            "branch": "",
            "outcome": "finished",
            "exit": 0,
            "turns": 2,
            "cost_usd": 0.01,
            "changed_files": ["a.txt"],
            "commits": []
        })
        .to_string(),
    )
    .unwrap();

    let status = env.blyte(&["status", "T1"], None);
    assert_eq!(
        status.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_out = String::from_utf8_lossy(&status.stdout);
    assert!(
        status_out.contains("outcome: finished"),
        "unexpected status output: {status_out}"
    );

    let wait = env.blyte(&["wait", "T1"], None);
    assert_eq!(
        wait.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&wait.stderr)
    );
    let wait_out = String::from_utf8_lossy(&wait.stdout);
    assert!(
        wait_out.contains("T1") && wait_out.contains("finished"),
        "unexpected wait output: {wait_out}"
    );
}

#[test]
fn restart_of_interrupted_run_preserves_originals_and_appends_log() {
    if let Err(reason) = sandbox_can_apply() {
        eprintln!(
            "skipping restart_of_interrupted_run_preserves_originals_and_appends_log: {reason}"
        );
        return;
    }

    let env = TestEnv::new();
    let store = env.run_store();
    std::fs::write(env.folder.join("a.txt"), "mid\n").unwrap();
    let before_dir = store.join("R1.before");
    std::fs::create_dir_all(&before_dir).unwrap();
    std::fs::write(before_dir.join("a.txt"), "original\n").unwrap();
    std::fs::write(
        store.join("R1.changes.json"),
        "{\"path\":\"a.txt\",\"existed\":true,\"ts\":1}\n",
    )
    .unwrap();
    std::fs::write(store.join("R1.log"), "prior log line\n").unwrap();
    std::fs::write(store.join("R1.start.json"), "{\"id\":\"R1\"}\n").unwrap();

    let port = start_scripted_server(vec![
        write_tool_turn("a.txt", "final\n"),
        stop_turn("finished after restart"),
    ]);

    let out = env.blyte(
        &["dispatch", "R1", env.task_file.to_str().unwrap()],
        Some(port),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(summary["outcome"], "finished");
    assert_eq!(summary["changed_files"], serde_json::json!(["a.txt"]));

    let diff = std::fs::read_to_string(store.join("R1.diff")).unwrap();
    assert!(
        diff.contains("-original"),
        "diff should use saved original: {diff}"
    );
    assert!(
        diff.contains("+final"),
        "diff should show final text: {diff}"
    );

    let log = std::fs::read_to_string(store.join("R1.log")).unwrap();
    assert!(
        log.starts_with("prior log line\n"),
        "log should be appended to: {log}"
    );
}

#[test]
fn invalid_blyte_toml_is_refused() {
    let env = TestEnv::new();
    std::fs::write(env.folder.join("blyte.toml"), "not valid toml = = [\n").unwrap();
    let bad = env.blyte(&["dispatch", "C1", env.task_file.to_str().unwrap()], None);
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("error:"));
}

#[test]
fn config_gate_returns_verified() {
    if let Err(reason) = sandbox_can_apply() {
        eprintln!("skipping config_gate_returns_verified: {reason}");
        return;
    }

    let env = TestEnv::new();
    std::fs::write(
        env.folder.join("blyte.toml"),
        "[dispatch]\ngate = \"test -f from_cfg.txt\"\n",
    )
    .unwrap();
    let port = start_scripted_server(vec![
        write_tool_turn("from_cfg.txt", "ok\n"),
        stop_turn("created from_cfg.txt"),
    ]);
    let ok = env.blyte(
        &["dispatch", "C2", env.task_file.to_str().unwrap()],
        Some(port),
    );
    assert_eq!(
        ok.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&ok.stdout),
        String::from_utf8_lossy(&ok.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&ok.stdout).expect("JSON summary");
    assert_eq!(summary["outcome"], "verified");
    assert_eq!(summary["gate_source"], "blyte.toml");
}

#[test]
fn records_dir_for_returns_dot_wt_in_git_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(init.success());

    let expected = repo.canonicalize().unwrap().join(".wt");
    assert_eq!(records_dir_for(&repo).unwrap(), expected);
}

#[test]
fn outcome_table_and_in_place_rules() {
    assert_eq!(
        in_place_outcome("finished", false, true),
        ("finished", None)
    );
    assert_eq!(
        in_place_outcome("finished", false, false),
        ("no_changes", None)
    );
    assert_eq!(in_place_outcome("verified", true, true), ("verified", None));
    assert_eq!(
        in_place_outcome("verified", true, false),
        ("no_changes", None)
    );
    assert_eq!(
        in_place_outcome("gate_failed", true, true),
        ("gate_failed", None)
    );
    assert_eq!(
        in_place_outcome("budget_exhausted", false, false),
        ("budget_exhausted", None)
    );
    assert_eq!(in_place_outcome("error", false, false), ("error", None));

    let (out, err) = in_place_outcome("blocked", false, true);
    assert_eq!(out, "error");
    assert!(err.unwrap().contains("blocked"));

    let (out, err) = in_place_outcome("finished", true, true);
    assert_eq!(out, "error");
    assert!(err.unwrap().contains("finished"));

    let (out, err) = in_place_outcome("verified", false, true);
    assert_eq!(out, "error");
    assert!(err.unwrap().contains("verified"));

    assert_eq!(exit_code("finished"), 0);
    assert_eq!(exit_code("verified"), 0);
    assert_eq!(exit_code("no_changes"), 4);

    let wrapped = wrap_in_place_task("  Update a.txt\n");
    assert!(wrapped.contains("Update a.txt"));
    assert!(wrapped.ends_with(IN_PLACE_RULES));
    assert!(IN_PLACE_RULES.contains("write and edit"));
    assert!(IN_PLACE_RULES.contains("do not commit"));
}

#[test]
fn unix_timestamp_secs_rejects_pre_epoch_time() {
    let post_epoch = std::time::UNIX_EPOCH + std::time::Duration::from_secs(12345);
    assert_eq!(unix_timestamp_secs(post_epoch), Ok(12345));

    let pre_epoch = std::time::UNIX_EPOCH
        .checked_sub(std::time::Duration::from_secs(1))
        .expect("pre-epoch SystemTime");
    let err = unix_timestamp_secs(pre_epoch).unwrap_err();
    assert!(
        err.contains("UNIX_EPOCH"),
        "expected error naming UNIX_EPOCH, got: {err}"
    );
}

#[test]
fn pre_spawn_failure_removes_fresh_start_record_and_allows_reuse() {
    let env = TestEnv::new();
    let store = env.run_store();
    std::fs::create_dir_all(store.join("P1.log")).unwrap();

    let out = env.blyte(&["dispatch", "P1", env.task_file.to_str().unwrap()], None);
    assert_eq!(
        out.status.code(),
        Some(2),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error"),
        "expected error on stderr, got: {stderr}"
    );
    assert!(
        !store.join("P1.json").exists(),
        "pre-spawn failure must not write summary record"
    );
    assert!(
        !store.join("P1.start.json").exists(),
        "pre-spawn failure must clean up fresh start record"
    );

    std::fs::write(store.join("R2.start.json"), "{\"id\":\"R2\"}\n").unwrap();
    std::fs::create_dir_all(store.join("R2.log")).unwrap();
    let restart_out = env.blyte(&["dispatch", "R2", env.task_file.to_str().unwrap()], None);
    assert_eq!(restart_out.status.code(), Some(2));
    assert!(
        !store.join("R2.json").exists(),
        "pre-spawn failure on restart must not write summary record"
    );
    assert!(
        store.join("R2.start.json").exists(),
        "pre-spawn failure on restart must preserve existing start record"
    );

    if let Err(reason) = sandbox_can_apply() {
        eprintln!("skipping reuse dispatch part: {reason}");
        return;
    }

    std::fs::remove_dir(store.join("P1.log")).unwrap();
    std::fs::write(env.folder.join("a.txt"), "before\n").unwrap();
    let port = start_scripted_server(vec![
        write_tool_turn("a.txt", "after\n"),
        stop_turn("reused P1"),
    ]);
    let reused = env.blyte(
        &["dispatch", "P1", env.task_file.to_str().unwrap()],
        Some(port),
    );
    assert_eq!(
        reused.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&reused.stdout),
        String::from_utf8_lossy(&reused.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&reused.stdout).expect("JSON summary");
    assert_eq!(summary["outcome"], "finished");
}

#[test]
fn gate_failed_leaves_in_place_edits_recorded_without_rollback() {
    if let Err(reason) = sandbox_can_apply() {
        eprintln!("skipping gate_failed_leaves_in_place_edits_recorded_without_rollback: {reason}");
        return;
    }

    let env = TestEnv::new();
    std::fs::write(env.folder.join("a.txt"), "old line\n").unwrap();
    let port = start_scripted_server(vec![
        write_tool_turn("a.txt", "edited line\n"),
        stop_turn("updated a.txt"),
    ]);

    let out = env.blyte(
        &[
            "dispatch",
            "G1",
            env.task_file.to_str().unwrap(),
            "--gate",
            "false",
        ],
        Some(port),
    );
    assert_eq!(
        out.status.code(),
        Some(exit_code("gate_failed")),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(summary["outcome"], "gate_failed");
    assert_eq!(summary["changed_files"], serde_json::json!(["a.txt"]));

    assert_eq!(
        std::fs::read_to_string(env.folder.join("a.txt")).unwrap(),
        "edited line\n",
        "in-place edit must not be rolled back on gate failure"
    );

    let store = env.run_store();
    assert_eq!(
        std::fs::read_to_string(store.join("G1.before").join("a.txt")).unwrap(),
        "old line\n"
    );
    let diff = std::fs::read_to_string(store.join("G1.diff")).unwrap();
    assert!(diff.contains("-old line"), "diff missing old line: {diff}");
    assert!(
        diff.contains("+edited line"),
        "diff missing edited line: {diff}"
    );

    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(store.join("G1.report.json")).unwrap())
            .unwrap();
    assert_eq!(report["outcome"], "gate_failed");
    assert_eq!(report["rollbacks"], 0);

    let status = env.blyte(&["status", "G1"], None);
    assert_eq!(status.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&status.stdout).contains("outcome: gate_failed"));
}

#[test]
fn budget_exhausted_leaves_in_place_edits_recorded_without_rollback() {
    if let Err(reason) = sandbox_can_apply() {
        eprintln!(
            "skipping budget_exhausted_leaves_in_place_edits_recorded_without_rollback: {reason}"
        );
        return;
    }

    let env = TestEnv::new();
    std::fs::write(env.folder.join("a.txt"), "old line\n").unwrap();
    let port = start_scripted_server(vec![write_tool_turn("a.txt", "partial edit\n")]);

    let out = env.blyte(
        &[
            "dispatch",
            "B1",
            env.task_file.to_str().unwrap(),
            "--max-turns",
            "1",
        ],
        Some(port),
    );
    assert_eq!(
        out.status.code(),
        Some(exit_code("budget_exhausted")),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(summary["outcome"], "budget_exhausted");
    assert_eq!(summary["changed_files"], serde_json::json!(["a.txt"]));

    assert_eq!(
        std::fs::read_to_string(env.folder.join("a.txt")).unwrap(),
        "partial edit\n",
        "in-place edit must not be rolled back on budget exhaustion"
    );

    let store = env.run_store();
    let diff = std::fs::read_to_string(store.join("B1.diff")).unwrap();
    assert!(diff.contains("-old line"), "diff missing old line: {diff}");
    assert!(
        diff.contains("+partial edit"),
        "diff missing partial edit: {diff}"
    );
}

fn read_tool_turn(call_id: &str, path: &str) -> serde_json::Value {
    let args = serde_json::json!({ "path": path }).to_string();
    serde_json::json!({
        "id": "r-read",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": "read",
                        "arguments": args,
                    }
                }]
            },
            "finish_reason": "tool_calls",
        }],
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 25,
            "total_tokens": 125,
        }
    })
}

fn bash_tool_turn(call_id: &str, command: &str) -> serde_json::Value {
    let args = serde_json::json!({ "command": command }).to_string();
    serde_json::json!({
        "id": "r-bash",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": "bash",
                        "arguments": args,
                    }
                }]
            },
            "finish_reason": "tool_calls",
        }],
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 25,
            "total_tokens": 125,
        }
    })
}

fn start_recording_server(
    turns: Vec<serde_json::Value>,
) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let idx = AtomicUsize::new(0);
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = std::sync::Arc::clone(&bodies);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let req_body = read_http_body(&mut stream);
            captured.lock().unwrap().push(req_body.clone());
            let body = if req_body.contains("### Claim ") {
                stop_turn(r#"[{"id": 1, "verdict": "supported", "reason": "matches quoted line"}]"#)
                    .to_string()
            } else {
                let i = idx.fetch_add(1, Ordering::SeqCst);
                turns
                    .get(i)
                    .unwrap_or_else(|| turns.last().unwrap())
                    .to_string()
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (port, bodies)
}

impl TestEnv {
    fn blyte_with_home(&self, args: &[&str], port: Option<u16>, home: &Path) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
        cmd.args(args)
            .current_dir(&self.folder)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", &self.home)
            .env("BLYTE_STATE_DIR", &self.state_dir)
            .env("BLYTE_NO_UPDATE_CHECK", "1")
            .env("BLYTE_NO_KEEP_AWAKE", "1")
            .env("BLYTE_USAGE_LOG", "off")
            .env("BLYTE_GATE_SLOT_DIR", self.home.join("slots"))
            .env("BLYTE_GATE_SLOTS", "16")
            .env("BLYTE_ENV_FILE", "")
            .env_remove("GEMINI_API_KEY")
            .env_remove("GOOGLE_API_KEY")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("OLLAMA_HOST")
            .env_remove("OLLAMA_API_KEY")
            .env_remove("OPENAI_BASE_URL")
            .env_remove("OPENAI_API_KEY")
            .env_remove("BLYTE_MODEL")
            .env_remove("BLYTE_MODEL_SHELL");
        if let Some(p) = port {
            cmd.env("OPENAI_BASE_URL", format!("http://127.0.0.1:{p}/v1"))
                .env("OPENAI_API_KEY", "test")
                .env("BLYTE_MODEL", "openai/gemini-3.8-flash");
        }
        cmd.output().unwrap()
    }
}

#[test]
fn research_run_reads_notes_and_writes_verified_report() {
    let env = TestEnv::new();
    std::fs::write(
        env.folder.join("notes.txt"),
        "line one\nimportant fact from notes\nline three\n",
    )
    .unwrap();
    let notes_full = env.canonical_folder().join("notes.txt");
    let notes_str = notes_full.display().to_string();
    let report_md = format!(
        "# Findings\n\nThe notes record the key fact at `{notes_str}:2`:\n\n```text\nimportant fact from notes\n```\n"
    );

    let (port, requests) = start_recording_server(vec![
        read_tool_turn("call_read", &notes_str),
        write_tool_turn("report.md", &report_md),
        stop_turn("wrote report.md"),
    ]);

    let out = env.blyte(
        &[
            "dispatch",
            "R1",
            env.task_file.to_str().unwrap(),
            "--research",
            "--sandbox",
            "off",
        ],
        Some(port),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    let store = env.run_store();
    let scratch_dir = store.join("R1");
    let expected_report = store.join("R1.report.md");

    assert_eq!(summary["outcome"], "verified");
    assert_eq!(summary["branch"], "");
    assert_eq!(summary["commits"], serde_json::json!([]));
    assert_eq!(summary["changed_files"], serde_json::json!([]));
    assert_eq!(summary["gate_source"], "research");
    assert_eq!(summary["worktree"], scratch_dir.display().to_string());
    assert_eq!(
        summary["research_report"],
        expected_report.display().to_string()
    );

    let saved_report = std::fs::read_to_string(&expected_report).unwrap();
    assert!(
        saved_report.contains("important fact from notes"),
        "report missing quoted line: {saved_report}"
    );
    assert!(
        !saved_report.contains("NOT VERIFIED"),
        "verified report should not have unverified banner: {saved_report}"
    );

    assert_eq!(
        dir_entries(&env.folder),
        BTreeSet::from(["notes.txt".to_string()])
    );

    let start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(store.join("R1.start.json")).unwrap())
            .unwrap();
    assert_eq!(start["in_place"], false);
    assert_eq!(start["research"], true);
    assert_eq!(start["branch"], "");
    assert_eq!(start["worktree"], scratch_dir.display().to_string());

    let recorded = requests.lock().unwrap();
    assert!(
        recorded
            .first()
            .is_some_and(|b| b.contains(NON_GIT_RESEARCH_NOTE)),
        "research prompt should contain non-git full-path citation note"
    );

    let wrapped = wrap_in_place_research("What is in notes.txt?", &env.canonical_folder(), "SRC");
    assert!(wrapped.contains(NON_GIT_RESEARCH_NOTE));
    assert!(wrapped.contains("SRC"));
    assert!(wrapped.contains("What is in notes.txt?"));
}

#[test]
fn research_run_refuses_write_to_folder() {
    let env = TestEnv::new();
    let target = env.canonical_folder().join("x.txt");
    let target_str = target.display().to_string();

    let (port, requests) = start_recording_server(vec![
        write_tool_turn(&target_str, "should be refused\n"),
        write_tool_turn("report.md", "# Findings\n\nWrite was refused.\n"),
        stop_turn("done"),
    ]);

    let out = env.blyte(
        &[
            "dispatch",
            "R_WRITE",
            env.task_file.to_str().unwrap(),
            "--research",
            "--sandbox",
            "off",
        ],
        Some(port),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !env.folder.join("x.txt").exists(),
        "x.txt must not exist in folder after refused write"
    );
    assert!(dir_entries(&env.folder).is_empty());

    let recorded = requests.lock().unwrap();
    let second_req = recorded.get(1).expect("second turn request");
    assert!(
        second_req.contains("error") || second_req.contains("outside"),
        "write tool should have returned refusal to model: {second_req}"
    );
}

#[test]
fn research_run_succeeds_while_folder_lock_is_held() {
    let env = TestEnv::new();
    let lock_path = env.lock_path();
    let holder = blyte_agent::folder_lock::Holder {
        pid: std::process::id(),
        kind: "run".to_string(),
        id: "HOLDER1".to_string(),
    };
    let _lock = blyte_agent::folder_lock::acquire(&lock_path, &holder).unwrap();

    let (port, _requests) = start_recording_server(vec![
        write_tool_turn("report.md", "# Findings\n\nRan alongside holder.\n"),
        stop_turn("done"),
    ]);

    let out = env.blyte(
        &[
            "dispatch",
            "R_LOCK",
            env.task_file.to_str().unwrap(),
            "--research",
            "--sandbox",
            "off",
        ],
        Some(port),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(summary["outcome"], "verified");
    assert!(env.run_store().join("R_LOCK.report.md").exists());
}

#[test]
fn research_run_read_tool_refuses_aws_credentials() {
    let env = TestEnv::new();
    let secret = "AKIA_TEST_SECRET_READ_TOOL_12345";
    std::fs::create_dir_all(env.folder.join(".aws")).unwrap();
    std::fs::write(
        env.folder.join(".aws").join("credentials"),
        format!("[default]\naws_access_key_id = {secret}\n"),
    )
    .unwrap();
    let root = env.canonical_folder();
    let creds = root.join(".aws").join("credentials");
    let creds_str = creds.display().to_string();

    let (port, requests) = start_recording_server(vec![
        read_tool_turn("call_read_creds", &creds_str),
        write_tool_turn("report.md", "# Findings\n\nCould not read credentials.\n"),
        stop_turn("done"),
    ]);

    let out = env.blyte_with_home(
        &[
            "dispatch",
            "R_CREDS_READ",
            env.task_file.to_str().unwrap(),
            "--research",
            "--sandbox",
            "off",
        ],
        Some(port),
        &root,
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let recorded = requests.lock().unwrap();
    for (idx, body) in recorded.iter().enumerate() {
        assert!(
            !body.contains(secret),
            "request {idx} leaked credential secret: {body}"
        );
    }
    let after_read = recorded.get(1).expect("turn after read tool");
    assert!(
        after_read.contains("credential path")
            || after_read.contains("read was not made")
            || after_read.contains("denied")
            || after_read.contains("protected")
            || after_read.contains("refused"),
        "expected read tool refusal in turn 2 request: {after_read}"
    );
}

#[test]
fn research_run_refuses_reading_aws_credentials() {
    if let Err(reason) = sandbox_can_apply() {
        eprintln!("skipping research_run_refuses_reading_aws_credentials: {reason}");
        return;
    }

    let env = TestEnv::new();
    let secret = "AKIA_TEST_SECRET_VALUE_987654";
    std::fs::create_dir_all(env.folder.join(".aws")).unwrap();
    std::fs::write(
        env.folder.join(".aws").join("credentials"),
        format!("[default]\naws_access_key_id = {secret}\n"),
    )
    .unwrap();
    let root = env.canonical_folder();
    let creds = root.join(".aws").join("credentials");
    let creds_str = creds.display().to_string();

    let (port, requests) = start_recording_server(vec![
        read_tool_turn("call_read_creds", &creds_str),
        bash_tool_turn("call_bash_creds", &format!("cat '{creds_str}'")),
        write_tool_turn("report.md", "# Findings\n\nCould not read credentials.\n"),
        stop_turn("done"),
    ]);

    let out = env.blyte_with_home(
        &[
            "dispatch",
            "R_CREDS",
            env.task_file.to_str().unwrap(),
            "--research",
        ],
        Some(port),
        &root,
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let recorded = requests.lock().unwrap();
    for (idx, body) in recorded.iter().enumerate() {
        assert!(
            !body.contains(secret),
            "request {idx} leaked credential secret: {body}"
        );
    }
    let after_read = recorded.get(1).expect("turn after read tool");
    assert!(
        after_read.contains("credential path")
            || after_read.contains("read was not made")
            || after_read.contains("denied")
            || after_read.contains("protected")
            || after_read.contains("refused"),
        "expected read tool refusal in turn 2 request: {after_read}"
    );
    let after_bash = recorded.get(2).expect("turn after bash tool");
    assert!(
        after_bash.contains("Operation not permitted")
            || after_bash.contains("Permission denied")
            || after_bash.contains("exit"),
        "expected sandboxed shell refusal in turn 3 request: {after_bash}"
    );
}

#[test]
fn research_run_report_to_receives_report() {
    let env = TestEnv::new();
    let dest = env._tmp.path().join("out").join("copied-report.md");

    let (port, _requests) = start_recording_server(vec![
        write_tool_turn("report.md", "# Findings\n\nCopied report body.\n"),
        stop_turn("done"),
    ]);

    let out = env.blyte(
        &[
            "dispatch",
            "R_COPY",
            env.task_file.to_str().unwrap(),
            "--research",
            "--sandbox",
            "off",
            "--report-to",
            dest.to_str().unwrap(),
        ],
        Some(port),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(summary["outcome"], "verified");

    let copied = std::fs::read_to_string(&dest).unwrap();
    assert!(
        copied.contains("Copied report body."),
        "report-to missing body: {copied}"
    );
    let store_report = std::fs::read_to_string(env.run_store().join("R_COPY.report.md")).unwrap();
    assert_eq!(copied, store_report);
}

#[test]
fn research_run_unverified_report_gets_banner_and_restart_reuses_scratch() {
    let env = TestEnv::new();
    let dest = env._tmp.path().join("unverified.md");
    let (port1, _req1) = start_recording_server(vec![write_tool_turn(
        "report.md",
        "# Partial\n\nRan out of turns.\n",
    )]);

    let out1 = env.blyte(
        &[
            "dispatch",
            "R_UNV",
            env.task_file.to_str().unwrap(),
            "--research",
            "--sandbox",
            "off",
            "--max-turns",
            "1",
            "--report-to",
            dest.to_str().unwrap(),
        ],
        Some(port1),
    );
    assert_eq!(
        out1.status.code(),
        Some(exit_code("budget_exhausted")),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out1.stdout),
        String::from_utf8_lossy(&out1.stderr)
    );
    let store = env.run_store();
    let store_report = std::fs::read_to_string(store.join("R_UNV.report.md")).unwrap();
    assert!(
        store_report.contains("did not pass") || store_report.contains("NOT VERIFIED"),
        "expected unverified banner in store report: {store_report}"
    );
    let dest_report = std::fs::read_to_string(&dest).unwrap();
    assert!(
        dest_report.contains("did not pass") || dest_report.contains("NOT VERIFIED"),
        "expected unverified banner in --report-to file: {dest_report}"
    );

    let scratch = store.join("R_RESTART");
    std::fs::create_dir_all(&scratch).unwrap();
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&scratch)
        .status()
        .unwrap();
    assert!(init.success());
    std::fs::write(scratch.join("prior.txt"), "kept\n").unwrap();
    std::fs::write(store.join("R_RESTART.log"), "prior line\n").unwrap();
    std::fs::write(
        store.join("R_RESTART.start.json"),
        "{\"id\":\"R_RESTART\"}\n",
    )
    .unwrap();

    let (port2, _req2) = start_recording_server(vec![
        write_tool_turn("report.md", "# Restarted\n\nFinished after restart.\n"),
        stop_turn("done"),
    ]);
    let out2 = env.blyte(
        &[
            "dispatch",
            "R_RESTART",
            env.task_file.to_str().unwrap(),
            "--research",
            "--sandbox",
            "off",
        ],
        Some(port2),
    );
    assert_eq!(
        out2.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(scratch.join("prior.txt")).unwrap(),
        "kept\n"
    );
    let log = std::fs::read_to_string(store.join("R_RESTART.log")).unwrap();
    assert!(log.starts_with("prior line\n"));
}
