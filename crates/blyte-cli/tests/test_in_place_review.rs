use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use blyte_agent::sandbox;

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
        std::fs::write(&task_file, "Update a.txt and create sub/b.txt.\n").unwrap();
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

    fn seed_run_t1(&self) {
        std::fs::write(self.folder.join("a.txt"), "new line\n").unwrap();
        std::fs::create_dir_all(self.folder.join("sub")).unwrap();
        std::fs::write(self.folder.join("sub/b.txt"), "created content\n").unwrap();

        let store = self.run_store();
        std::fs::create_dir_all(store.join("T1.before")).unwrap();
        std::fs::write(store.join("T1.before/a.txt"), "old line\n").unwrap();
        std::fs::create_dir_all(store.join("T1.after/sub")).unwrap();
        std::fs::write(store.join("T1.after/a.txt"), "new line\n").unwrap();
        std::fs::write(store.join("T1.after/sub/b.txt"), "created content\n").unwrap();
        std::fs::write(
            store.join("T1.changes.json"),
            "{\"path\":\"a.txt\",\"existed\":true}\n{\"path\":\"sub/b.txt\",\"existed\":false}\n",
        )
        .unwrap();
        std::fs::write(
            store.join("T1.diff"),
            "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old line\n+new line\n--- /dev/null\n+++ b/sub/b.txt\n@@ -0,0 +1 @@\n+created content\n",
        )
        .unwrap();
        std::fs::write(store.join("T1.log"), "").unwrap();
        std::fs::write(
            store.join("T1.start.json"),
            serde_json::json!({
                "id": "T1",
                "pid": 999999,
                "started_at": 1700000000,
                "in_place": true,
                "worktree": self.canonical_folder().display().to_string(),
                "branch": "",
                "base": null,
                "base_sha": null
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            store.join("T1.json"),
            serde_json::json!({
                "id": "T1",
                "worktree": self.canonical_folder().display().to_string(),
                "branch": "",
                "outcome": "finished",
                "exit": 0,
                "turns": 3,
                "cost_usd": 0.02,
                "changed_files": ["a.txt", "sub/b.txt"],
                "commits": [],
                "final_message": "updated a.txt and created sub/b.txt"
            })
            .to_string(),
        )
        .unwrap();
    }
}

#[test]
fn dispatch_writes_after_copies_and_review_shows_files_diff_and_verdict() {
    if let Err(reason) = sandbox_can_apply() {
        eprintln!(
            "skipping dispatch_writes_after_copies_and_review_shows_files_diff_and_verdict: {reason}"
        );
        return;
    }

    let env = TestEnv::new();
    std::fs::write(env.folder.join("a.txt"), "old line\n").unwrap();
    let port = start_scripted_server(vec![
        write_tool_turn("a.txt", "new line\n"),
        write_tool_turn("sub/b.txt", "created content\n"),
        stop_turn("updated a.txt and created sub/b.txt"),
    ]);

    let dispatch = env.blyte(
        &["dispatch", "T1", env.task_file.to_str().unwrap()],
        Some(port),
    );
    assert_eq!(
        dispatch.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&dispatch.stdout),
        String::from_utf8_lossy(&dispatch.stderr)
    );

    let store = env.run_store();
    assert_eq!(
        std::fs::read_to_string(store.join("T1.after/a.txt")).unwrap(),
        "new line\n"
    );
    assert_eq!(
        std::fs::read_to_string(store.join("T1.after/sub/b.txt")).unwrap(),
        "created content\n"
    );

    let review = env.blyte(&["review", "T1"], None);
    assert_eq!(
        review.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&review.stderr)
    );
    let out = String::from_utf8_lossy(&review.stdout);
    let checks_idx = out.find("== checks").expect("== checks heading");
    let files_idx = out.find("== files").expect("== files heading");
    let report_idx = out.find("== report").expect("== report heading");
    assert!(checks_idx < files_idx && files_idx < report_idx, "{out}");
    assert!(out.contains("modified a.txt"), "{out}");
    assert!(out.contains("created sub/b.txt"), "{out}");
    assert!(out.contains("outcome: finished"), "{out}");
    assert!(
        out.contains("final_message: updated a.txt and created sub/b.txt"),
        "{out}"
    );

    let review_diff = env.blyte(&["review", "T1", "--diff"], None);
    assert_eq!(
        review_diff.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&review_diff.stderr)
    );
    let diff_out = String::from_utf8_lossy(&review_diff.stdout);
    assert!(diff_out.contains("== diff"), "{diff_out}");
    assert!(diff_out.contains("-old line"), "{diff_out}");
    assert!(diff_out.contains("+new line"), "{diff_out}");

    let verdict = env.blyte(
        &["review", "T1", "--verdict", "accepted", "--notes", "x"],
        None,
    );
    assert_eq!(
        verdict.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&verdict.stderr)
    );
    let verdict_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(store.join("T1.verdict.json")).unwrap())
            .unwrap();
    assert_eq!(verdict_json["verdict"], "accepted");
    assert_eq!(verdict_json["notes"], "x");
    assert!(verdict_json["ts"].as_u64().is_some());
}

#[test]
fn review_files_diff_verdict_and_missing_run_errors() {
    let env = TestEnv::new();
    env.seed_run_t1();
    let store = env.run_store();

    let review = env.blyte(&["review", "T1"], None);
    assert_eq!(review.status.code(), Some(0));
    let out = String::from_utf8_lossy(&review.stdout);
    let checks_idx = out.find("== checks").expect("== checks");
    let files_idx = out.find("== files").expect("== files");
    let report_idx = out.find("== report").expect("== report");
    assert!(checks_idx < files_idx && files_idx < report_idx, "{out}");
    assert!(out.contains("nothing flagged"), "{out}");
    assert!(out.contains("modified a.txt"), "{out}");
    assert!(out.contains("created sub/b.txt"), "{out}");
    assert!(out.contains("outcome: finished"), "{out}");
    assert!(out.contains("turns: 3"), "{out}");
    assert!(out.contains("cost_usd: 0.02"), "{out}");

    let review_diff = env.blyte(&["review", "T1", "--diff"], None);
    assert_eq!(review_diff.status.code(), Some(0));
    let diff_out = String::from_utf8_lossy(&review_diff.stdout);
    assert!(diff_out.contains("-old line"), "{diff_out}");
    assert!(diff_out.contains("+new line"), "{diff_out}");

    let verdict = env.blyte(
        &["review", "T1", "--verdict", "accepted", "--notes", "x"],
        None,
    );
    assert_eq!(verdict.status.code(), Some(0));
    let verdict_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(store.join("T1.verdict.json")).unwrap())
            .unwrap();
    assert_eq!(verdict_json["verdict"], "accepted");
    assert_eq!(verdict_json["notes"], "x");

    let verdict_no_notes = env.blyte(&["review", "T1", "--verdict", "fixed"], None);
    assert_eq!(verdict_no_notes.status.code(), Some(0));
    let verdict_no_notes_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(store.join("T1.verdict.json")).unwrap())
            .unwrap();
    assert_eq!(verdict_no_notes_json["verdict"], "fixed");
    assert_eq!(verdict_no_notes_json["notes"], "");

    let nope = env.blyte(&["review", "NOPE"], None);
    assert_eq!(nope.status.code(), Some(2));
    let nope_err = String::from_utf8_lossy(&nope.stderr);
    assert!(
        nope_err.contains("no run NOPE in this folder"),
        "unexpected stderr: {nope_err}"
    );

    std::fs::write(store.join("RUN2.start.json"), "{\"id\":\"RUN2\"}\n").unwrap();
    let unfinished = env.blyte(&["review", "RUN2"], None);
    assert_eq!(unfinished.status.code(), Some(2));
    let unfinished_err = String::from_utf8_lossy(&unfinished.stderr);
    assert!(
        unfinished_err.contains("run RUN2 has not finished"),
        "unexpected stderr: {unfinished_err}"
    );

    std::fs::remove_file(store.join("T1.diff")).unwrap();
    let missing_diff = env.blyte(&["review", "T1", "--diff"], None);
    assert_eq!(missing_diff.status.code(), Some(2));
    let missing_diff_err = String::from_utf8_lossy(&missing_diff.stderr);
    assert!(
        missing_diff_err.contains("T1.diff"),
        "missing diff error should name file: {missing_diff_err}"
    );
}

#[test]
fn review_research_run_omits_files_and_flags_non_finished_and_log_checks() {
    let env = TestEnv::new();
    let store = env.run_store();
    std::fs::write(
        store.join("R1.start.json"),
        serde_json::json!({
            "id": "R1",
            "in_place": true,
            "research": true
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        store.join("R1.json"),
        serde_json::json!({
            "id": "R1",
            "outcome": "budget_exhausted",
            "turns": 10,
            "cost_usd": 0.5,
            "changed_files": [],
            "final_message": "ran out of turns"
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        store.join("R1.log"),
        "● bash sudo id -> exit 0\n● bash cat ~/secret.txt -> exit 0\npath escapes the workspace: /var/other/secret.txt. This run can read only workspace files\n",
    )
    .unwrap();

    let review = env.blyte(&["review", "R1"], None);
    assert_eq!(
        review.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&review.stderr)
    );
    let out = String::from_utf8_lossy(&review.stdout);
    assert!(out.contains("== checks"), "{out}");
    assert!(!out.contains("== files"), "{out}");
    assert!(out.contains("== report"), "{out}");
    assert!(out.contains("outcome budget_exhausted"), "{out}");
    assert!(out.contains("host command(s)"), "{out}");
    assert!(out.contains("read(s) outside"), "{out}");
    assert!(out.contains("refused reads: 1"), "{out}");
    assert!(out.contains("== host commands"), "{out}");
    assert!(out.contains("== reads outside"), "{out}");

    let undo_research = env.blyte(&["undo", "R1"], None);
    assert_eq!(undo_research.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&undo_research.stderr).contains("research"));
}

#[test]
fn undo_restores_and_removes_files_marks_undone_and_flags_review() {
    let env = TestEnv::new();
    env.seed_run_t1();
    let store = env.run_store();

    let undo = env.blyte(&["undo", "T1"], None);
    assert_eq!(
        undo.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    let undo_out = String::from_utf8_lossy(&undo.stdout);
    assert!(undo_out.contains("a.txt"), "{undo_out}");
    assert!(undo_out.contains("sub/b.txt"), "{undo_out}");

    assert_eq!(
        std::fs::read_to_string(env.folder.join("a.txt")).unwrap(),
        "old line\n"
    );
    assert!(!env.folder.join("sub/b.txt").exists());
    assert!(env.folder.join("sub").is_dir());

    let undone_marker: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(store.join("T1.undone.json")).unwrap())
            .unwrap();
    assert!(undone_marker["undone_at"].as_u64().is_some());

    let second = env.blyte(&["undo", "T1"], None);
    assert_eq!(second.status.code(), Some(2));
    let second_err = String::from_utf8_lossy(&second.stderr);
    assert!(
        second_err.contains("already undone"),
        "unexpected stderr: {second_err}"
    );

    let review = env.blyte(&["review", "T1"], None);
    assert_eq!(review.status.code(), Some(0));
    let review_out = String::from_utf8_lossy(&review.stdout);
    let checks_section = review_out
        .split("== files")
        .next()
        .expect("checks before files");
    assert!(
        checks_section.contains("undone"),
        "review should flag undo under == checks:\n{review_out}"
    );
}

#[test]
fn undo_refuses_when_file_was_edited_by_hand_without_changing_anything() {
    let env = TestEnv::new();
    env.seed_run_t1();

    std::fs::write(env.folder.join("a.txt"), "hand edit\n").unwrap();

    let undo = env.blyte(&["undo", "T1"], None);
    assert_eq!(undo.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&undo.stderr);
    assert!(
        stderr.contains("changed since run T1 ended") && stderr.contains("a.txt"),
        "unexpected stderr: {stderr}"
    );

    assert_eq!(
        std::fs::read_to_string(env.folder.join("a.txt")).unwrap(),
        "hand edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(env.folder.join("sub/b.txt")).unwrap(),
        "created content\n"
    );
    assert!(!env.run_store().join("T1.undone.json").exists());
}

#[test]
fn undo_refuses_when_before_copy_is_missing_without_changing_anything() {
    let env = TestEnv::new();
    env.seed_run_t1();
    let store = env.run_store();
    std::fs::remove_file(store.join("T1.before/a.txt")).unwrap();

    let undo = env.blyte(&["undo", "T1"], None);
    assert_eq!(undo.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&undo.stderr);
    assert!(
        stderr.contains("T1.before") && stderr.contains("a.txt"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(env.folder.join("a.txt")).unwrap(),
        "new line\n"
    );
    assert!(env.folder.join("sub/b.txt").exists());
    assert!(!store.join("T1.undone.json").exists());
}

#[test]
fn undo_refuses_while_lock_is_held_by_live_process() {
    let env = TestEnv::new();
    env.seed_run_t1();

    let holder = blyte_agent::folder_lock::Holder {
        pid: std::process::id(),
        kind: "run".to_string(),
        id: "LIVE_HOLDER".to_string(),
    };
    let _lock = blyte_agent::folder_lock::acquire(&env.lock_path(), &holder).unwrap();

    let undo = env.blyte(&["undo", "T1"], None);
    assert_eq!(undo.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&undo.stderr);
    assert!(
        stderr.contains("LIVE_HOLDER"),
        "stderr should name holder: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(env.folder.join("a.txt")).unwrap(),
        "new line\n"
    );
    assert!(env.folder.join("sub/b.txt").exists());
    assert!(!env.run_store().join("T1.undone.json").exists());
}

#[test]
fn interrupted_undo_completes_when_run_again() {
    let env = TestEnv::new();
    env.seed_run_t1();

    std::fs::write(env.folder.join("a.txt"), "old line\n").unwrap();

    let undo = env.blyte(&["undo", "T1"], None);
    assert_eq!(
        undo.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(env.folder.join("a.txt")).unwrap(),
        "old line\n"
    );
    assert!(!env.folder.join("sub/b.txt").exists());
    assert!(env.run_store().join("T1.undone.json").exists());
}

#[test]
fn undo_refuses_when_after_copy_is_missing_without_changing_anything() {
    let env = TestEnv::new();
    env.seed_run_t1();
    let store = env.run_store();
    std::fs::remove_file(store.join("T1.after/sub/b.txt")).unwrap();

    let undo = env.blyte(&["undo", "T1"], None);
    assert_eq!(undo.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&undo.stderr);
    assert!(
        stderr.contains("T1.after") && stderr.contains("sub/b.txt"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(env.folder.join("a.txt")).unwrap(),
        "new line\n"
    );
    assert!(env.folder.join("sub/b.txt").exists());
    assert!(!store.join("T1.undone.json").exists());
}

#[test]
fn undo_inside_git_repository_is_refused() {
    let env = TestEnv::new();
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&env.folder)
        .status()
        .unwrap();
    assert!(init.success());

    let undo = env.blyte(&["undo", "T1"], None);
    assert_eq!(undo.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&undo.stderr);
    assert!(
        stderr.contains("undo is for runs in a folder that is not a git repository"),
        "unexpected stderr: {stderr}"
    );
}
