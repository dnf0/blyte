use std::collections::{BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use blyte_cli::cli::SurfaceArgs;
use blyte_cli::http::{Request, Response};
use blyte_cli::repl::ReplConfig;
use blyte_cli::serve::{route, run_with_io, Surface};
use blyte_cli::session::{EntryBody, PromptRunner, Session};

struct RecordedCall {
    prompt: String,
}

struct ScriptedRunner {
    responses: VecDeque<Result<Vec<EntryBody>, String>>,
    calls: Arc<Mutex<Vec<RecordedCall>>>,
}

impl ScriptedRunner {
    fn new(
        responses: Vec<Result<Vec<EntryBody>, String>>,
    ) -> (Self, Arc<Mutex<Vec<RecordedCall>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runner = Self {
            responses: responses.into(),
            calls: Arc::clone(&calls),
        };
        (runner, calls)
    }
}

impl PromptRunner for ScriptedRunner {
    fn run(
        &mut self,
        prompt: &str,
        _model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        self.calls.lock().unwrap().push(RecordedCall {
            prompt: prompt.to_string(),
        });
        match self.responses.pop_front().unwrap_or(Ok(Vec::new())) {
            Ok(entries) => {
                for body in entries {
                    emit(body);
                }
                Ok(())
            }
            Err(msg) => Err(msg),
        }
    }
}

struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn init_git_repo(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());
    let _ = Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(root)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(root)
        .status();
}

fn init_repo_with_commit(root: &Path) -> String {
    init_git_repo(root);
    write_file(&root.join("README.md"), "hello\n");
    assert!(Command::new("git")
        .args(["add", "README.md"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string()
}

fn write_finished_run_summary(
    repo: &Path,
    task_file: &Path,
    id: &str,
    base_sha: &str,
    started_at: u64,
) {
    let wt = repo.join(".wt");
    std::fs::create_dir_all(&wt).unwrap();
    write_file(task_file, &format!("# Implement {id}\n\nDetails.\n"));
    write_file(
        &wt.join(format!("{id}.start.json")),
        &format!(
            r#"{{"id":"{id}","pid":999999,"started_at":{started_at},"base_sha":"{base_sha}","task_file":"{}","research":false}}"#,
            task_file.display()
        ),
    );
    write_file(
        &wt.join(format!("{id}.log")),
        "[Turn 3] \n✓ Gate passed (attempt 1)\n",
    );
    write_file(
        &wt.join(format!("{id}.report.json")),
        r#"{"outcome":"verified","cost_usd":0.10,"turns":3,"gate_attempts":1,"model_id":"m","gate_failures":[]}"#,
    );
    write_file(
        &wt.join(format!("{id}.json")),
        r#"{"outcome":"verified","seconds":120,"final_message":"done","commits":[],"changed_files":["README.md"]}"#,
    );
}

fn add_run_worktree(repo: &Path, id: &str) {
    let wt = repo.join(".wt");
    std::fs::create_dir_all(&wt).unwrap();
    let wt_run = wt.join(id);
    assert!(Command::new("git")
        .args(["worktree", "add", "-q", "-b", &format!("wt/{id}")])
        .arg(&wt_run)
        .current_dir(repo)
        .status()
        .unwrap()
        .success());
    write_file(&wt_run.join("README.md"), &format!("hello\n{id}\n"));
    assert!(Command::new("git")
        .args(["add", "README.md"])
        .current_dir(&wt_run)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", &format!("feat: add {id}")])
        .current_dir(&wt_run)
        .status()
        .unwrap()
        .success());
}

fn make_surface(
    root: PathBuf,
    responses: Vec<Result<Vec<EntryBody>, String>>,
) -> (Surface, Arc<Mutex<Vec<RecordedCall>>>) {
    let repo = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap()
        .to_string();
    let (runner, calls) = ScriptedRunner::new(responses);
    let session = Session::start(Box::new(runner), "gemini-4-neon".to_string());
    let surface = Surface {
        root,
        repo,
        session,
        session_id: "sess-test-1".to_string(),
        started_at: 1_700_000_000,
        days: 14,
    };
    (surface, calls)
}

fn get(path: &str, query: Vec<(&str, &str)>) -> Request {
    Request {
        method: "GET".to_string(),
        path: path.to_string(),
        query: query
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: Vec::new(),
    }
}

fn post_json(path: &str, body: &serde_json::Value) -> Request {
    Request {
        method: "POST".to_string(),
        path: path.to_string(),
        query: Vec::new(),
        body: serde_json::to_vec(body).unwrap(),
    }
}

fn parse_json(resp: &Response) -> serde_json::Value {
    serde_json::from_slice(&resp.body).unwrap_or_else(|e| {
        panic!(
            "expected JSON body (status {}), got {:?}: {e}",
            resp.status,
            String::from_utf8_lossy(&resp.body)
        )
    })
}

fn default_repl_config(repo_root: PathBuf) -> ReplConfig {
    ReplConfig {
        repo_root,
        gate: None,
        context_strategy: "folded".to_string(),
        max_turns: 25,
        max_gates: 5,
        rollback: true,
        model: Some("gemini-4-neon".to_string()),
        container: None,
        orchestrate: false,
        fold_budget: None,
        dispatch_cmd: None,
        sandbox: "on".to_string(),
        network: None,
        allow_read: Vec::new(),
        pass_env: Vec::new(),
    }
}

#[test]
fn index_is_the_surface_page() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let (surface, _) = make_surface(repo, Vec::new());

    let resp = route(&surface, &get("/", vec![]), 1_700_000_100);
    assert_eq!(resp.status, 200);
    assert!(resp.content_type.starts_with("text/html"));
    let html = String::from_utf8(resp.body).unwrap();
    assert!(html.contains(r#"id="model-select""#));
}

#[test]
fn session_info_shape() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let (surface, _) = make_surface(repo, Vec::new());

    let resp = route(&surface, &get("/api/session", vec![]), 1_700_000_100);
    assert_eq!(resp.status, 200);
    let val = parse_json(&resp);
    let obj = val.as_object().unwrap();
    let keys: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = [
        "session_id",
        "repo",
        "started_at",
        "model",
        "effort",
        "effort_supported",
        "busy",
        "queued",
        "last_seq",
    ]
    .into_iter()
    .collect();
    assert_eq!(keys, expected);
    assert_eq!(obj.get("session_id").unwrap(), "sess-test-1");
    assert_eq!(obj.get("repo").unwrap(), "demo-repo");
    assert_eq!(obj.get("started_at").unwrap(), 1_700_000_000);
    assert_eq!(obj.get("model").unwrap(), "gemini-4-neon");
    assert_eq!(obj.get("effort").unwrap(), &serde_json::Value::Null);
    assert_eq!(obj.get("effort_supported").unwrap(), true);
    assert_eq!(obj.get("busy").unwrap(), false);
    assert_eq!(obj.get("queued").unwrap(), 0);
    assert_eq!(obj.get("last_seq").unwrap(), 0);
}

#[test]
fn message_then_events() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let (surface, _) = make_surface(
        repo,
        vec![Ok(vec![
            EntryBody::Turn {
                n: 1,
                text: "working".into(),
                input_tokens: 100,
                output_tokens: 40,
                cost_usd: Some(0.005),
            },
            EntryBody::ToolStarted {
                name: "read".into(),
                input: "Cargo.toml".into(),
            },
            EntryBody::ToolFinished {
                name: "read".into(),
                summary: "ok".into(),
                is_error: false,
            },
        ])],
    );

    let post_resp = route(
        &surface,
        &post_json(
            "/api/session/messages",
            &serde_json::json!({ "text": "hello surface" }),
        ),
        1_700_000_100,
    );
    assert_eq!(post_resp.status, 200);
    assert_eq!(parse_json(&post_resp), serde_json::json!({ "seq": 1 }));

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut collected = Vec::new();
    let mut after = 0u64;
    let last_seq = loop {
        assert!(Instant::now() < deadline, "timed out waiting for idle");
        let after_str = after.to_string();
        let ev_resp = route(
            &surface,
            &get(
                "/api/session/events",
                vec![("after", &after_str), ("wait_ms", "2000")],
            ),
            1_700_000_101,
        );
        assert_eq!(ev_resp.status, 200);
        let body = parse_json(&ev_resp);
        let batch_last_seq = body.get("last_seq").unwrap().as_u64().unwrap();
        let batch = body.get("entries").unwrap().as_array().unwrap();
        for entry in batch {
            after = entry.get("seq").unwrap().as_u64().unwrap();
            collected.push(entry.clone());
        }
        if collected
            .iter()
            .any(|e| e.get("kind").and_then(|v| v.as_str()) == Some("idle"))
        {
            break batch_last_seq;
        }
    };

    let kinds: Vec<&str> = collected
        .iter()
        .map(|e| e.get("kind").unwrap().as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "user_message",
            "turn",
            "tool_started",
            "tool_finished",
            "idle"
        ]
    );
    assert_eq!(
        last_seq,
        collected
            .last()
            .unwrap()
            .get("seq")
            .unwrap()
            .as_u64()
            .unwrap()
    );
}

#[test]
fn events_parameter_errors() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let (surface, _) = make_surface(repo, Vec::new());

    let missing = route(&surface, &get("/api/session/events", vec![]), 1_700_000_100);
    assert_eq!(missing.status, 400);
    let missing_err = parse_json(&missing)["error"].as_str().unwrap().to_string();
    assert!(missing_err.contains("after"), "got: {missing_err}");

    let bad_after = route(
        &surface,
        &get("/api/session/events", vec![("after", "x")]),
        1_700_000_100,
    );
    assert_eq!(bad_after.status, 400);
    let bad_after_err = parse_json(&bad_after)["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(bad_after_err.contains("after"), "got: {bad_after_err}");

    let high_wait = route(
        &surface,
        &get(
            "/api/session/events",
            vec![("after", "0"), ("wait_ms", "25001")],
        ),
        1_700_000_100,
    );
    assert_eq!(high_wait.status, 400);
    let high_wait_err = parse_json(&high_wait)["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(high_wait_err.contains("wait_ms"), "got: {high_wait_err}");
}

#[test]
fn events_wait_returns_empty() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let (surface, _) = make_surface(repo, Vec::new());

    let last_seq = surface.session.info().last_seq.to_string();
    let start = Instant::now();
    let resp = route(
        &surface,
        &get(
            "/api/session/events",
            vec![("after", &last_seq), ("wait_ms", "300")],
        ),
        1_700_000_100,
    );
    let elapsed = start.elapsed();

    assert_eq!(resp.status, 200);
    assert!(
        elapsed >= Duration::from_millis(300),
        "expected >= 300ms wait, got {elapsed:?}"
    );
    let body = parse_json(&resp);
    assert_eq!(body["entries"], serde_json::json!([]));
    assert_eq!(body["last_seq"], serde_json::json!(0));
}

#[test]
fn report_attachment_is_read_from_the_run() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    write_file(&repo.join(".wt/R1.report.md"), "finding");
    let (surface, calls) = make_surface(repo, vec![Ok(Vec::new())]);

    let resp = route(
        &surface,
        &post_json(
            "/api/session/messages",
            &serde_json::json!({ "text": "summarise", "reports": ["R1"] }),
        ),
        1_700_000_100,
    );
    assert_eq!(resp.status, 200);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for runner");
        let entries = surface.session.entries_after(0, Duration::from_millis(200));
        if entries
            .iter()
            .any(|e| matches!(e.body, EntryBody::Idle { .. }))
        {
            break;
        }
    }

    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(
        recorded[0].prompt.ends_with("Research report R1:\nfinding"),
        "got prompt: {:?}",
        recorded[0].prompt
    );
}

#[test]
fn missing_report_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let (surface, calls) = make_surface(repo.clone(), Vec::new());

    let missing_resp = route(
        &surface,
        &post_json(
            "/api/session/messages",
            &serde_json::json!({ "text": "look", "reports": ["R9"] }),
        ),
        1_700_000_100,
    );
    assert_eq!(missing_resp.status, 400);
    let err = parse_json(&missing_resp)["error"]
        .as_str()
        .unwrap()
        .to_string();
    let expected_path = repo.join(".wt/R9.report.md").display().to_string();
    assert!(err.contains("R9"), "error should name R9: {err}");
    assert!(
        err.contains(&expected_path),
        "error should name path {expected_path}: {err}"
    );
    assert_eq!(surface.session.info().last_seq, 0);
    assert!(calls.lock().unwrap().is_empty());

    let traversal_resp = route(
        &surface,
        &post_json(
            "/api/session/messages",
            &serde_json::json!({ "text": "look", "reports": ["../x"] }),
        ),
        1_700_000_100,
    );
    assert_eq!(traversal_resp.status, 400);
    assert_eq!(surface.session.info().last_seq, 0);
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn runs_carry_hold_and_checks() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    write_file(&repo.join("README.md"), "hello\n");
    assert!(Command::new("git")
        .args(["add", "README.md"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    let base_sha = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let wt = repo.join(".wt");
    std::fs::create_dir_all(&wt).unwrap();
    let wt_t1 = wt.join("T1");
    assert!(Command::new("git")
        .args(["worktree", "add", "-q", "-b", "wt/T1"])
        .arg(&wt_t1)
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    write_file(&wt_t1.join("README.md"), "hello\nworld\n");
    assert!(Command::new("git")
        .args(["add", "README.md"])
        .current_dir(&wt_t1)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "feat: add world"])
        .current_dir(&wt_t1)
        .status()
        .unwrap()
        .success());

    let t = now_secs();
    let task_file = d.path().join("tasks/t1.md");
    write_file(&task_file, "# Implement T1\n\nDetails.\n");
    write_file(
        &wt.join("T1.start.json"),
        &format!(
            r#"{{"id":"T1","pid":999999,"started_at":{},"base_sha":"{base_sha}","task_file":"{}","research":false}}"#,
            t - 600,
            task_file.display()
        ),
    );
    write_file(&wt.join("T1.log"), "[Turn 3] \n✓ Gate passed (attempt 1)\n");
    write_file(
        &wt.join("T1.report.json"),
        r#"{"outcome":"verified","cost_usd":0.10,"turns":3,"gate_attempts":1,"model_id":"m","gate_failures":[]}"#,
    );
    write_file(
        &wt.join("T1.json"),
        r#"{"outcome":"verified","seconds":120,"final_message":"done","commits":[],"changed_files":["README.md"]}"#,
    );

    let (surface, _) = make_surface(repo.clone(), Vec::new());

    let runs_resp = route(&surface, &get("/api/runs", vec![]), t);
    assert_eq!(
        runs_resp.status,
        200,
        "body={}",
        String::from_utf8_lossy(&runs_resp.body)
    );
    assert!(wt.join("T1.checks.json").exists());
    let runs_val = parse_json(&runs_resp);
    let runs = runs_val["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"], "T1");
    assert_eq!(runs[0]["hold"], serde_json::Value::Null);

    let diff_resp = route(&surface, &get("/api/runs/T1/diff", vec![]), t);
    assert_eq!(diff_resp.status, 200);
    assert!(diff_resp.content_type.starts_with("text/plain"));
    let diff_text = String::from_utf8(diff_resp.body).unwrap();
    assert!(diff_text.contains("+world"), "got diff: {diff_text}");

    let dec_resp = route(
        &surface,
        &post_json(
            "/api/runs/T1/decision",
            &serde_json::json!({ "action": "hold", "note": "later" }),
        ),
        t,
    );
    assert_eq!(dec_resp.status, 200);
    let dec_val = parse_json(&dec_resp);
    assert_eq!(dec_val["decision"]["run"], "T1");
    assert_eq!(dec_val["decision"]["action"], "hold");
    assert_eq!(dec_val["decision"]["note"], "later");
    let seq = dec_val["seq"].as_u64().unwrap();
    assert_eq!(seq, 1);

    let entries = surface.session.entries_after(0, Duration::ZERO);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].seq, seq);
    assert_eq!(
        entries[0].body,
        EntryBody::Decision {
            run: "T1".into(),
            action: "hold".into(),
            note: Some("later".into()),
        }
    );

    let runs_after = route(&surface, &get("/api/runs", vec![]), t + 1);
    assert_eq!(runs_after.status, 200);
    let runs_after_val = parse_json(&runs_after);
    assert_eq!(runs_after_val["runs"][0]["hold"]["note"], "later");
    assert_eq!(runs_after_val["runs"][0]["hold"]["ts"], t);
}

#[test]
fn run_without_a_worktree_is_still_listed() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    let base_sha = init_repo_with_commit(&repo);
    let t = now_secs();
    write_finished_run_summary(
        &repo,
        &d.path().join("tasks/t2.md"),
        "T2",
        &base_sha,
        t - 600,
    );
    assert!(!repo.join(".wt/T2").exists());

    let (surface, _) = make_surface(repo.clone(), Vec::new());
    let resp = route(&surface, &get("/api/runs", vec![]), t);
    assert_eq!(
        resp.status,
        200,
        "body={}",
        String::from_utf8_lossy(&resp.body)
    );
    let val = parse_json(&resp);
    let runs = val["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"], "T2");
    assert_eq!(runs[0]["checks_recorded"], false);
    assert!(!repo.join(".wt/T2.checks.json").exists());
}

#[test]
fn run_with_a_worktree_gets_its_checks() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    let base_sha = init_repo_with_commit(&repo);
    add_run_worktree(&repo, "T1");
    let t = now_secs();
    write_finished_run_summary(
        &repo,
        &d.path().join("tasks/t1.md"),
        "T1",
        &base_sha,
        t - 600,
    );
    assert!(!repo.join(".wt/T1.checks.json").exists());

    let (surface, _) = make_surface(repo.clone(), Vec::new());
    let resp = route(&surface, &get("/api/runs", vec![]), t);
    assert_eq!(
        resp.status,
        200,
        "body={}",
        String::from_utf8_lossy(&resp.body)
    );
    let val = parse_json(&resp);
    let runs = val["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"], "T1");
    assert_eq!(runs[0]["checks_recorded"], true);
    assert!(repo.join(".wt/T1.checks.json").exists());
}

#[test]
fn one_cleaned_run_does_not_hide_the_others() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    let base_sha = init_repo_with_commit(&repo);
    add_run_worktree(&repo, "T1");
    let t = now_secs();
    write_finished_run_summary(
        &repo,
        &d.path().join("tasks/t1.md"),
        "T1",
        &base_sha,
        t - 600,
    );
    write_finished_run_summary(
        &repo,
        &d.path().join("tasks/t2.md"),
        "T2",
        &base_sha,
        t - 300,
    );
    assert!(!repo.join(".wt/T2").exists());

    let (surface, _) = make_surface(repo.clone(), Vec::new());
    let resp = route(&surface, &get("/api/runs", vec![]), t);
    assert_eq!(
        resp.status,
        200,
        "body={}",
        String::from_utf8_lossy(&resp.body)
    );
    let val = parse_json(&resp);
    let runs = val["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);

    let t1 = runs.iter().find(|r| r["id"] == "T1").unwrap();
    let t2 = runs.iter().find(|r| r["id"] == "T2").unwrap();
    assert_eq!(t1["checks_recorded"], true);
    assert_eq!(t2["checks_recorded"], false);
    assert!(repo.join(".wt/T1.checks.json").exists());
    assert!(!repo.join(".wt/T2.checks.json").exists());
}

#[test]
fn decision_errors() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let t = now_secs();
    write_file(
        &repo.join(".wt/T1.json"),
        r#"{"outcome":"verified","seconds":10}"#,
    );
    let (surface, _) = make_surface(repo.clone(), Vec::new());

    let bad_action = route(
        &surface,
        &post_json(
            "/api/runs/T1/decision",
            &serde_json::json!({ "action": "ship_it" }),
        ),
        t,
    );
    assert_eq!(bad_action.status, 400);
    let bad_action_err = parse_json(&bad_action)["error"]
        .as_str()
        .unwrap()
        .to_string();
    for expected in ["accept", "send_back", "redo", "hold", "mark_read"] {
        assert!(
            bad_action_err.contains(expected),
            "error should list {expected}: {bad_action_err}"
        );
    }

    let direct_missing = blyte_cli::queue_actions::decide(
        &repo,
        "NOPE",
        blyte_cli::queue_actions::Action::Hold,
        Some("n"),
        t,
    )
    .unwrap_err();
    let missing_run = route(
        &surface,
        &post_json(
            "/api/runs/NOPE/decision",
            &serde_json::json!({ "action": "hold", "note": "n" }),
        ),
        t,
    );
    assert_eq!(missing_run.status, 400);
    assert_eq!(parse_json(&missing_run)["error"], direct_missing);

    let direct_no_note = blyte_cli::queue_actions::decide(
        &repo,
        "T1",
        blyte_cli::queue_actions::Action::SendBack,
        None,
        t,
    )
    .unwrap_err();
    let no_note = route(
        &surface,
        &post_json(
            "/api/runs/T1/decision",
            &serde_json::json!({ "action": "send_back" }),
        ),
        t,
    );
    assert_eq!(no_note.status, 400);
    assert_eq!(parse_json(&no_note)["error"], direct_no_note);
}

#[test]
fn send_back_reaches_the_orchestrator() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let t = now_secs();
    write_file(
        &repo.join(".wt/T1.json"),
        r#"{"outcome":"verified","seconds":10}"#,
    );
    let (surface, calls) = make_surface(repo, vec![Ok(Vec::new())]);

    let resp = route(
        &surface,
        &post_json(
            "/api/runs/T1/decision",
            &serde_json::json!({ "action": "send_back", "note": "fix edge case" }),
        ),
        t,
    );
    assert_eq!(resp.status, 200);
    let body = parse_json(&resp);
    let expected_msg = body["decision"]["orchestrator_message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!expected_msg.is_empty());

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for runner");
        let entries = surface.session.entries_after(0, Duration::from_millis(200));
        if entries
            .iter()
            .any(|e| matches!(e.body, EntryBody::Idle { .. }))
        {
            break;
        }
    }

    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].prompt, expected_msg);
}

#[test]
fn effort_and_model_errors() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let (surface, _) = make_surface(repo, Vec::new());

    let bad_effort = route(
        &surface,
        &post_json(
            "/api/session/effort",
            &serde_json::json!({ "effort": "max" }),
        ),
        1_700_000_100,
    );
    assert_eq!(bad_effort.status, 400);
    let err = parse_json(&bad_effort)["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        err.contains("effort must be low, medium or high"),
        "got: {err}"
    );

    let empty_model = route(
        &surface,
        &post_json("/api/session/model", &serde_json::json!({ "model": "" })),
        1_700_000_100,
    );
    assert_eq!(empty_model.status, 400);
    assert!(parse_json(&empty_model)["error"].is_string());
    assert_eq!(surface.session.info().model, "gemini-4-neon");
}

#[test]
fn wrong_method_and_unknown_path() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let (surface, _) = make_surface(repo, Vec::new());

    let wrong = route(
        &surface,
        &post_json("/api/session", &serde_json::json!({})),
        1_700_000_100,
    );
    assert_eq!(wrong.status, 405);
    assert!(parse_json(&wrong)["error"].is_string());

    let nope = route(&surface, &get("/api/nope", vec![]), 1_700_000_100);
    assert_eq!(nope.status, 404);
    assert!(parse_json(&nope)["error"].is_string());

    let unknown_diff = route(&surface, &get("/api/runs/NOPE/diff", vec![]), 1_700_000_100);
    assert_eq!(unknown_diff.status, 404);
    assert!(parse_json(&unknown_diff)["error"].is_string());
}

#[test]
fn map_is_served() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-crate");
    init_git_repo(&repo);
    write_file(
        &repo.join("Cargo.toml"),
        "[package]\nname = \"demo-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(&repo.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
    let (surface, _) = make_surface(repo, Vec::new());

    let resp = route(&surface, &get("/api/map", vec![]), 1_700_000_100);
    assert_eq!(resp.status, 200);
    let val = parse_json(&resp);
    let groups = val["groups"].as_array().unwrap();
    let units = val["units"].as_array().unwrap();
    assert!(!groups.is_empty(), "expected non-empty groups array: {val}");
    assert!(!units.is_empty(), "expected non-empty units array: {val}");
}

#[test]
fn terminal_flag_conflicts() {
    let bin = env!("CARGO_BIN_EXE_blyte");

    let conflict = Command::new(bin)
        .args(["--terminal", "--port", "1"])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    let stderr = String::from_utf8_lossy(&conflict.stderr);
    assert!(
        stderr.contains("--terminal") || stderr.contains("--port"),
        "expected usage error naming conflicting flags, got: {stderr}"
    );

    let prompt_conflict = Command::new(bin)
        .args(["--port", "1", "do something"])
        .output()
        .unwrap();
    assert!(!prompt_conflict.status.success());

    let subcmd_conflict = Command::new(bin)
        .args(["--no-open", "clean"])
        .output()
        .unwrap();
    assert!(!subcmd_conflict.status.success());

    let bad_days = Command::new(bin).args(["--days", "32"]).output().unwrap();
    assert!(!bad_days.status.success());

    let help = Command::new(bin).arg("--help").output().unwrap();
    assert!(help.status.success());
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(help_text.contains("surface"), "help should mention surface");
    assert!(
        help_text.contains("--terminal"),
        "help should mention --terminal"
    );
}

#[test]
fn piped_stdin_still_runs_the_terminal_session() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"/exit\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Blyte Interactive Harness"));
}

#[test]
fn second_start_attaches() {
    let d1 = tempfile::tempdir().unwrap();
    let repo1 = d1.path().join("attach-repo");
    init_git_repo(&repo1);
    let surface_file1 = repo1.join(".wt/surface.json");

    let out1_buf = Arc::new(Mutex::new(Vec::new()));
    let err1_buf = Arc::new(Mutex::new(Vec::new()));
    let repo1_bg = repo1.clone();
    let out1_bg = Arc::clone(&out1_buf);
    let err1_bg = Arc::clone(&err1_buf);
    std::thread::spawn(move || {
        let args = SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
        };
        let mut out = SharedBuf(out1_bg);
        let mut err = SharedBuf(err1_bg);
        let _ = run_with_io(&args, default_repl_config(repo1_bg), &mut out, &mut err);
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let first_record: serde_json::Value = loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for surface.json; stderr={}",
            String::from_utf8_lossy(&err1_buf.lock().unwrap())
        );
        if let Ok(raw) = std::fs::read_to_string(&surface_file1) {
            if let Ok(v) = serde_json::from_str(&raw) {
                break v;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let port = first_record["port"].as_u64().unwrap() as u16;
    assert!(port > 0);

    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let code = run_with_io(
        &SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
        },
        default_repl_config(repo1),
        &mut out2,
        &mut err2,
    );
    assert_eq!(code, 0, "stderr={}", String::from_utf8_lossy(&err2));
    let out2_str = String::from_utf8(out2).unwrap();
    assert!(
        out2_str.contains("surface already running"),
        "expected 'surface already running', got: {out2_str}"
    );
    assert!(
        out2_str.contains(&format!("http://127.0.0.1:{port}/")),
        "expected port {port} in output, got: {out2_str}"
    );

    let closed_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = closed_listener.local_addr().unwrap().port();
    drop(closed_listener);

    let d2 = tempfile::tempdir().unwrap();
    let repo2 = d2.path().join("stale-repo");
    init_git_repo(&repo2);
    let surface_file2 = repo2.join(".wt/surface.json");
    write_file(
        &surface_file2,
        &format!(
            r#"{{"pid":999999,"port":{closed_port},"session_id":"stale-sess","started_at":1000}}"#
        ),
    );

    let out3_buf = Arc::new(Mutex::new(Vec::new()));
    let err3_buf = Arc::new(Mutex::new(Vec::new()));
    let repo2_bg = repo2.clone();
    let out3_bg = Arc::clone(&out3_buf);
    let err3_bg = Arc::clone(&err3_buf);
    std::thread::spawn(move || {
        let args = SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
        };
        let mut out = SharedBuf(out3_bg);
        let mut err = SharedBuf(err3_bg);
        let _ = run_with_io(&args, default_repl_config(repo2_bg), &mut out, &mut err);
    });

    let deadline2 = Instant::now() + Duration::from_secs(5);
    let new_record: serde_json::Value = loop {
        assert!(
            Instant::now() < deadline2,
            "timed out waiting for stale surface.json replacement; stderr={}",
            String::from_utf8_lossy(&err3_buf.lock().unwrap())
        );
        if let Ok(raw) = std::fs::read_to_string(&surface_file2) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if v.get("session_id").and_then(|s| s.as_str()) != Some("stale-sess") {
                    break v;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let err3_str = String::from_utf8(err3_buf.lock().unwrap().clone()).unwrap();
    assert!(
        err3_str.contains("stale"),
        "expected stale warning on stderr, got: {err3_str}"
    );
    let new_port = new_record["port"].as_u64().unwrap() as u16;
    let new_sid = new_record["session_id"].as_str().unwrap();

    let mut stream = TcpStream::connect(("127.0.0.1", new_port)).unwrap();
    let req = format!(
        "GET /api/session HTTP/1.1\r\nHost: 127.0.0.1:{new_port}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw_resp = String::new();
    stream.read_to_string(&mut raw_resp).unwrap();
    assert!(raw_resp.starts_with("HTTP/1.1 200"));
    assert!(raw_resp.contains(new_sid));
}
