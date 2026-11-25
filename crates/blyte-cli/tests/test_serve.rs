use std::collections::{BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use blyte_agent::approval::Approver;
use blyte_cli::cli::SurfaceArgs;
use blyte_cli::http::{Request, Response};
use blyte_cli::repl::ReplConfig;
use blyte_cli::serve::{
    authenticated_api_get_routes, constant_time_eq, generate_token, route, run_with_hooks,
    run_with_io, Surface, RECONCILE_SAFETY_NET_INTERVAL, TOKEN_HEADER,
};
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
    let cfg_path = root.join(".git/config");
    let mut cfg = std::fs::read_to_string(&cfg_path).unwrap();
    cfg.push_str("\n[user]\n\temail = test@example.com\n\tname = Test User\n");
    std::fs::write(&cfg_path, cfg).unwrap();
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
        token: generate_token().unwrap(),
        requests: Arc::new(blyte_cli::requests::RequestsState::new()),
        publisher: Arc::new(blyte_cli::requests::RealPublisher),
        reconcile_lock: Mutex::new(()),
        reconcile_requested: std::sync::atomic::AtomicBool::new(false),
    };
    (surface, calls)
}

fn get(surface: &Surface, path: &str, query: Vec<(&str, &str)>) -> Request {
    get_headers(path, query, vec![(TOKEN_HEADER, &surface.token)])
}

fn get_headers(path: &str, query: Vec<(&str, &str)>, headers: Vec<(&str, &str)>) -> Request {
    Request {
        method: "GET".to_string(),
        path: path.to_string(),
        query: query
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        headers: headers
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: Vec::new(),
    }
}

fn post_json(surface: &Surface, path: &str, body: &serde_json::Value) -> Request {
    post_json_headers(path, body, vec![(TOKEN_HEADER, &surface.token)])
}

fn post_json_headers(path: &str, body: &serde_json::Value, headers: Vec<(&str, &str)>) -> Request {
    Request {
        method: "POST".to_string(),
        path: path.to_string(),
        query: Vec::new(),
        headers: headers
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
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
        converse: false,
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

    let resp = route(&surface, &get(&surface, "/", vec![]), 1_700_000_100);
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

    let resp = route(
        &surface,
        &get(&surface, "/api/session", vec![]),
        1_700_000_100,
    );
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
            &surface,
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
                &surface,
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

    let missing = route(
        &surface,
        &get(&surface, "/api/session/events", vec![]),
        1_700_000_100,
    );
    assert_eq!(missing.status, 400);
    let missing_err = parse_json(&missing)["error"].as_str().unwrap().to_string();
    assert!(missing_err.contains("after"), "got: {missing_err}");

    let bad_after = route(
        &surface,
        &get(&surface, "/api/session/events", vec![("after", "x")]),
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
            &surface,
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
            &surface,
            "/api/session/events",
            vec![("after", &last_seq), ("wait_ms", "50")],
        ),
        1_700_000_100,
    );
    let elapsed = start.elapsed();

    assert_eq!(resp.status, 200);
    assert!(
        elapsed >= Duration::from_millis(50),
        "expected >= 50ms wait, got {elapsed:?}"
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
            &surface,
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
            &surface,
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
            &surface,
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

    let runs_resp = route(&surface, &get(&surface, "/api/runs", vec![]), t);
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

    let diff_resp = route(&surface, &get(&surface, "/api/runs/T1/diff", vec![]), t);
    assert_eq!(diff_resp.status, 200);
    assert!(diff_resp.content_type.starts_with("text/plain"));
    let diff_text = String::from_utf8(diff_resp.body).unwrap();
    assert!(diff_text.contains("+world"), "got diff: {diff_text}");

    let dec_resp = route(
        &surface,
        &post_json(
            &surface,
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

    let runs_after = route(&surface, &get(&surface, "/api/runs", vec![]), t + 1);
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
    let resp = route(&surface, &get(&surface, "/api/runs", vec![]), t);
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
    let resp = route(&surface, &get(&surface, "/api/runs", vec![]), t);
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
    let resp = route(&surface, &get(&surface, "/api/runs", vec![]), t);
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
            &surface,
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
            &surface,
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
            &surface,
            "/api/runs/T1/decision",
            &serde_json::json!({ "action": "send_back" }),
        ),
        t,
    );
    assert_eq!(no_note.status, 400);
    assert_eq!(parse_json(&no_note)["error"], direct_no_note);
}

#[test]
fn send_back_reaches_the_conversation() {
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
            &surface,
            "/api/runs/T1/decision",
            &serde_json::json!({ "action": "send_back", "note": "fix edge case" }),
        ),
        t,
    );
    assert_eq!(resp.status, 200);
    let body = parse_json(&resp);
    let expected_msg = body["decision"]["conversation_message"]
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
            &surface,
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
        &post_json(
            &surface,
            "/api/session/model",
            &serde_json::json!({ "model": "" }),
        ),
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
        &post_json(&surface, "/api/session", &serde_json::json!({})),
        1_700_000_100,
    );
    assert_eq!(wrong.status, 405);
    assert!(parse_json(&wrong)["error"].is_string());

    let nope = route(&surface, &get(&surface, "/api/nope", vec![]), 1_700_000_100);
    assert_eq!(nope.status, 404);
    assert!(parse_json(&nope)["error"].is_string());

    let unknown_diff = route(
        &surface,
        &get(&surface, "/api/runs/NOPE/diff", vec![]),
        1_700_000_100,
    );
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

    let resp = route(&surface, &get(&surface, "/api/map", vec![]), 1_700_000_100);
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
    let home = tempfile::tempdir().unwrap();

    let conflict = Command::new(bin)
        .args(["--terminal", "--port", "1"])
        .env("HOME", home.path())
        .env("BLYTE_NO_UPDATE_CHECK", "1")
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
        .env("HOME", home.path())
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    assert!(!prompt_conflict.status.success());

    let subcmd_conflict = Command::new(bin)
        .args(["--no-open", "clean"])
        .env("HOME", home.path())
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    assert!(!subcmd_conflict.status.success());

    let bad_days = Command::new(bin)
        .args(["--days", "32"])
        .env("HOME", home.path())
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    assert!(!bad_days.status.success());

    let help = Command::new(bin)
        .arg("--help")
        .env("HOME", home.path())
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    assert!(help.status.success());
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(help_text.contains("surface"), "help should mention surface");
    assert!(
        help_text.contains("--terminal"),
        "help should mention --terminal"
    );
}

#[test]
fn the_terminal_flag_runs_the_terminal_session_on_piped_input() {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("--terminal")
        .env("HOME", home.path())
        .env("BLYTE_NO_UPDATE_CHECK", "1")
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

#[cfg(unix)]
#[test]
fn bare_blyte_starts_the_surface_without_a_terminal() {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;

    struct ChildGuard(Option<std::process::Child>);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(mut child) = self.0.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    let repo_dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    init_repo_with_commit(repo_dir.path());

    let mut child = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["--no-open", "--port", "0"])
        .current_dir(repo_dir.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env("BLYTE_ENV_FILE", "")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_USAGE_LOG", "off")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut guard = ChildGuard(Some(child));

    let stderr_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        buf
    });

    let (tx, rx) = mpsc::channel::<String>();
    let stdout_handle = std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let start_deadline = Instant::now() + Duration::from_secs(30);
    let mut seen_stdout = Vec::new();
    let mut started = false;
    let mut exited_early = None;
    loop {
        let remaining = start_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(line) => {
                let is_ready = line.starts_with("blyte: surface at ");
                seen_stdout.push(line);
                if is_ready {
                    started = true;
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(status) = guard.0.as_mut().unwrap().try_wait().unwrap() {
                    exited_early = Some(status);
                    guard.0 = None;
                    break;
                }
            }
        }
    }

    if !started {
        let status = match exited_early {
            Some(status) => status,
            None => {
                let mut child = guard.0.take().unwrap();
                match child.try_wait().unwrap() {
                    Some(status) => status,
                    None => {
                        let _ = child.kill();
                        child.wait().unwrap()
                    }
                }
            }
        };
        let stderr_text = stderr_handle.join().unwrap_or_default();
        drop(rx);
        let _ = stdout_handle.join();
        panic!(
            "surface did not print readiness line within 30s (status: {status:?})\nstdout:\n{}\nstderr:\n{stderr_text}",
            seen_stdout.join("\n")
        );
    }

    let surface_file = repo_dir.path().join(".wt/surface.json");
    assert!(
        surface_file.exists(),
        "expected {} to exist after surface startup",
        surface_file.display()
    );

    let pid = guard.0.as_ref().unwrap().id().to_string();
    let kill_status = Command::new("kill").args(["-TERM", &pid]).status().unwrap();
    assert!(
        kill_status.success(),
        "kill -TERM {pid} failed: {kill_status:?}"
    );

    let exit_deadline = Instant::now() + Duration::from_secs(10);
    let exit_status = loop {
        if let Some(status) = guard.0.as_mut().unwrap().try_wait().unwrap() {
            guard.0 = None;
            break Some(status);
        }
        if Instant::now() >= exit_deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    let Some(status) = exit_status else {
        let mut child = guard.0.take().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        let stderr_text = stderr_handle.join().unwrap_or_default();
        drop(rx);
        let _ = stdout_handle.join();
        panic!("surface did not exit within 10s of SIGTERM\nstderr:\n{stderr_text}");
    };

    let stderr_text = stderr_handle.join().unwrap_or_default();
    drop(rx);
    let _ = stdout_handle.join();

    assert!(
        status.success(),
        "expected surface to exit 0 after SIGTERM, got {status:?}\nstderr:\n{stderr_text}"
    );
    assert!(
        !surface_file.exists(),
        "expected {} to be removed on shutdown",
        surface_file.display()
    );
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
        std::thread::sleep(Duration::from_millis(5));
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
        std::thread::sleep(Duration::from_millis(5));
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
        "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{new_port}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw_resp = String::new();
    stream.read_to_string(&mut raw_resp).unwrap();
    assert!(raw_resp.starts_with("HTTP/1.1 200"));
    assert!(raw_resp.contains(new_sid));
}

fn flip_last_hex_char(token: &str) -> String {
    let mut chars: Vec<char> = token.chars().collect();
    let last = chars.last_mut().unwrap();
    *last = if *last == '0' { '1' } else { '0' };
    chars.into_iter().collect()
}

fn start_surface_and_read_url(repo: &Path) -> (u16, String) {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let repo_bg = repo.to_path_buf();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    std::thread::spawn(move || {
        let args = SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
        };
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let _ = run_with_io(&args, default_repl_config(repo_bg), &mut out, &mut err);
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let line = loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for printed surface URL; stderr={}",
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        let stdout = String::from_utf8(out_buf.lock().unwrap().clone()).unwrap();
        if let Some(found) = stdout.lines().find(|l| l.contains("blyte: surface at ")) {
            break found.to_string();
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let url = line
        .split("blyte: surface at ")
        .nth(1)
        .unwrap()
        .trim()
        .to_string();
    let rest = url.strip_prefix("http://127.0.0.1:").unwrap();
    let (port_str, fragment) = rest.split_once("/#token=").unwrap();
    let port: u16 = port_str.parse().unwrap();
    (port, fragment.to_string())
}

fn send_http(port: u16, raw_req: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(raw_req.as_bytes()).unwrap();
    let mut raw_resp = String::new();
    stream.read_to_string(&mut raw_resp).unwrap();
    raw_resp
}

#[test]
fn post_endpoints_reject_missing_and_wrong_token() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let t = now_secs();
    write_file(
        &repo.join(".wt/T1.json"),
        r#"{"outcome":"verified","seconds":10}"#,
    );
    let (surface, calls) = make_surface(repo.clone(), vec![Ok(Vec::new())]);
    let wrong = flip_last_hex_char(&surface.token);

    let msg_body = serde_json::json!({ "text": "hello" });
    for headers in [
        vec![],
        vec![(TOKEN_HEADER, "")],
        vec![(TOKEN_HEADER, wrong.as_str())],
        vec![(TOKEN_HEADER, &surface.token[..16])],
        vec![
            (TOKEN_HEADER, surface.token.as_str()),
            (TOKEN_HEADER, surface.token.as_str()),
        ],
    ] {
        let resp = route(
            &surface,
            &post_json_headers("/api/session/messages", &msg_body, headers),
            t,
        );
        assert_eq!(resp.status, 403);
        assert!(parse_json(&resp)["error"].is_string());
    }
    assert_eq!(surface.session.info().last_seq, 0);
    assert!(calls.lock().unwrap().is_empty());

    let effort_body = serde_json::json!({ "effort": "high" });
    for headers in [vec![], vec![(TOKEN_HEADER, wrong.as_str())]] {
        let resp = route(
            &surface,
            &post_json_headers("/api/session/effort", &effort_body, headers),
            t,
        );
        assert_eq!(resp.status, 403);
        assert!(parse_json(&resp)["error"].is_string());
    }
    assert_eq!(surface.session.info().effort, None);
    assert_eq!(surface.session.info().last_seq, 0);

    let model_body = serde_json::json!({ "model": "ollama/llama3" });
    for headers in [vec![], vec![(TOKEN_HEADER, wrong.as_str())]] {
        let resp = route(
            &surface,
            &post_json_headers("/api/session/model", &model_body, headers),
            t,
        );
        assert_eq!(resp.status, 403);
        assert!(parse_json(&resp)["error"].is_string());
    }
    assert_eq!(surface.session.info().model, "gemini-4-neon");
    assert_eq!(surface.session.info().last_seq, 0);

    let decision_body = serde_json::json!({ "action": "hold", "note": "wait" });
    for headers in [vec![], vec![(TOKEN_HEADER, wrong.as_str())]] {
        let resp = route(
            &surface,
            &post_json_headers("/api/runs/T1/decision", &decision_body, headers),
            t,
        );
        assert_eq!(resp.status, 403);
        assert!(parse_json(&resp)["error"].is_string());
    }
    assert!(!repo.join(".wt/T1.hold.json").exists());
    assert_eq!(surface.session.info().last_seq, 0);

    let changed_body = serde_json::json!({});
    for headers in [vec![], vec![(TOKEN_HEADER, wrong.as_str())]] {
        let resp = route(
            &surface,
            &post_json_headers("/api/runs/changed", &changed_body, headers),
            t,
        );
        assert_eq!(resp.status, 403);
        assert!(parse_json(&resp)["error"].is_string());
    }
    assert!(!surface.reconcile_requested.load(Ordering::Acquire));

    let ok_effort = route(
        &surface,
        &post_json_headers(
            "/api/session/effort",
            &effort_body,
            vec![("x-blyte-token", surface.token.as_str())],
        ),
        t,
    );
    assert_eq!(ok_effort.status, 200);
    assert_eq!(surface.session.info().effort.as_deref(), Some("high"));

    let ok_model_empty = route(
        &surface,
        &post_json(
            &surface,
            "/api/session/model",
            &serde_json::json!({ "model": "" }),
        ),
        t,
    );
    assert_eq!(ok_model_empty.status, 400);
    assert_eq!(
        parse_json(&ok_model_empty)["error"].as_str(),
        Some("field model must not be empty")
    );

    let ok_decision = route(
        &surface,
        &post_json(&surface, "/api/runs/T1/decision", &decision_body),
        t,
    );
    assert_eq!(ok_decision.status, 200);
    assert!(repo.join(".wt/T1.hold.json").exists());

    let ok_msg = route(
        &surface,
        &post_json(&surface, "/api/session/messages", &msg_body),
        t,
    );
    assert_eq!(ok_msg.status, 200);
    assert!(parse_json(&ok_msg)["seq"].as_u64().unwrap() > 0);
}

#[test]
fn secret_never_leaks_and_origin_check_stays() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("leak-check-repo");
    init_git_repo(&repo);
    let (port, token) = start_surface_and_read_url(&repo);
    assert_eq!(token.len(), 64);
    assert!(token
        .bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));

    for path in ["/", "/api/health"] {
        let resp = send_http(
            port,
            &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
        );
        assert!(
            resp.starts_with("HTTP/1.1 200"),
            "GET {path} failed: {resp}"
        );
        assert!(!resp.contains(&token), "secret leaked in GET {path}");
    }

    for path in ["/api/session", "/api/session/events?after=0", "/api/runs"] {
        let resp = send_http(
            port,
            &format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{TOKEN_HEADER}: {token}\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(
            resp.starts_with("HTTP/1.1 200"),
            "GET {path} failed: {resp}"
        );
        assert!(!resp.contains(&token), "secret leaked in GET {path}");
    }

    let body = r#"{"effort":"high"}"#;
    let missing_resp = send_http(
        port,
        &format!(
            "POST /api/session/effort HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(
        missing_resp.starts_with("HTTP/1.1 403"),
        "expected 403 without token, got: {missing_resp}"
    );

    let wrong = flip_last_hex_char(&token);
    let wrong_resp = send_http(
        port,
        &format!(
            "POST /api/session/effort HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\n{TOKEN_HEADER}: {wrong}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(
        wrong_resp.starts_with("HTTP/1.1 403"),
        "expected 403 with wrong token, got: {wrong_resp}"
    );

    let bad_origin_resp = send_http(
        port,
        &format!(
            "POST /api/session/effort HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://evil.example\r\n{TOKEN_HEADER}: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(
        bad_origin_resp.starts_with("HTTP/1.1 403"),
        "expected 403 for bad Origin even with valid token, got: {bad_origin_resp}"
    );

    let ok_resp = send_http(
        port,
        &format!(
            "POST /api/session/effort HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\n{TOKEN_HEADER}: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(
        ok_resp.starts_with("HTTP/1.1 200"),
        "expected 200 with valid token and origin, got: {ok_resp}"
    );

    for path in ["/api/session", "/api/session/events?after=0"] {
        let resp = send_http(
            port,
            &format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{TOKEN_HEADER}: {token}\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(resp.starts_with("HTTP/1.1 200"));
        assert!(
            !resp.contains(&token),
            "secret leaked after POST in GET {path}"
        );
    }

    let wt_dir = repo.join(".wt");
    assert!(wt_dir.join("surface.json").exists());
    for entry in std::fs::read_dir(&wt_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() && path.file_name().and_then(|n| n.to_str()) != Some("surface.token") {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(
                !content.contains(&token),
                "secret leaked in file {}",
                path.display()
            );
        }
    }
}

#[test]
fn two_starts_give_different_secrets() {
    let d1 = tempfile::tempdir().unwrap();
    let repo1 = d1.path().join("start-one");
    init_git_repo(&repo1);
    let (_port1, token1) = start_surface_and_read_url(&repo1);

    let d2 = tempfile::tempdir().unwrap();
    let repo2 = d2.path().join("start-two");
    init_git_repo(&repo2);
    let (_port2, token2) = start_surface_and_read_url(&repo2);

    assert_eq!(token1.len(), 64);
    assert_eq!(token2.len(), 64);
    assert_ne!(token1, token2);

    assert!(constant_time_eq(token1.as_bytes(), token1.as_bytes()));
    assert!(!constant_time_eq(token1.as_bytes(), token2.as_bytes()));
    assert!(!constant_time_eq(b"short", b"longer"));
    assert!(!constant_time_eq(b"longer", b"short"));
    assert!(constant_time_eq(b"", b""));
}

#[test]
fn approval_endpoint_answers_validates_and_requires_secret() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let (surface, _) = make_surface(repo, Vec::new());
    let t = 1_700_000_200;

    let wrong = flip_last_hex_char(&surface.token);
    for headers in [vec![], vec![(TOKEN_HEADER, wrong.as_str())]] {
        let resp = route(
            &surface,
            &post_json_headers(
                "/api/session/approvals/approval-1",
                &serde_json::json!({ "answer": "yes" }),
                headers,
            ),
            t,
        );
        assert_eq!(resp.status, 403);
        assert!(parse_json(&resp)["error"].is_string());
    }

    let wrong_method = route(
        &surface,
        &get(&surface, "/api/session/approvals/approval-1", vec![]),
        t,
    );
    assert_eq!(wrong_method.status, 405);

    let unknown = route(
        &surface,
        &post_json(
            &surface,
            "/api/session/approvals/unknown-id",
            &serde_json::json!({ "answer": "yes" }),
        ),
        t,
    );
    assert_eq!(unknown.status, 404);
    assert!(parse_json(&unknown)["error"].is_string());

    for bad_body in [
        serde_json::json!({}),
        serde_json::json!({ "answer": "maybe" }),
        serde_json::json!({ "answer": "YES" }),
        serde_json::json!({ "answer": true }),
        serde_json::json!({ "answer": "yes", "extra": 1 }),
        serde_json::json!("yes"),
    ] {
        let resp = route(
            &surface,
            &post_json(&surface, "/api/session/approvals/approval-1", &bad_body),
            t,
        );
        assert_eq!(resp.status, 400, "body={bad_body}");
        assert!(parse_json(&resp)["error"].is_string());
    }

    let invalid_json_req = Request {
        method: "POST".to_string(),
        path: "/api/session/approvals/approval-1".to_string(),
        query: Vec::new(),
        headers: vec![(TOKEN_HEADER.to_string(), surface.token.clone())],
        body: b"{not json".to_vec(),
    };
    let invalid_json_resp = route(&surface, &invalid_json_req, t);
    assert_eq!(invalid_json_resp.status, 400);

    let approver = surface.session.approver();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let cmd = "git push origin main && echo '<img src=x onerror=alert(1)>'";
    let reason = "push verified branch";
    let ask_thread = std::thread::spawn(move || {
        let _ = done_tx.send(approver.ask(cmd, reason));
    });

    let ev_resp = route(
        &surface,
        &get(
            &surface,
            "/api/session/events",
            vec![("after", "0"), ("wait_ms", "2000")],
        ),
        t,
    );
    assert_eq!(ev_resp.status, 200);
    let ev_json = parse_json(&ev_resp);
    let entries = ev_json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["kind"], "approval_requested");
    assert_eq!(entries[0]["command"], cmd);
    assert_eq!(entries[0]["reason"], reason);
    let id = entries[0]["id"].as_str().unwrap().to_string();

    let unauth_answer = route(
        &surface,
        &post_json_headers(
            &format!("/api/session/approvals/{id}"),
            &serde_json::json!({ "answer": "yes" }),
            vec![],
        ),
        t,
    );
    assert_eq!(unauth_answer.status, 403);
    assert!(done_rx.recv_timeout(Duration::from_millis(20)).is_err());

    let ok_yes = route(
        &surface,
        &post_json(
            &surface,
            &format!("/api/session/approvals/{id}"),
            &serde_json::json!({ "answer": "yes" }),
        ),
        t,
    );
    assert_eq!(ok_yes.status, 200);
    assert_eq!(
        parse_json(&ok_yes),
        serde_json::json!({ "id": id, "answer": "yes", "seq": 2 })
    );
    assert_eq!(
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        blyte_agent::approval::Approval::Approved
    );
    ask_thread.join().unwrap();

    let conflict = route(
        &surface,
        &post_json(
            &surface,
            &format!("/api/session/approvals/{id}"),
            &serde_json::json!({ "answer": "no" }),
        ),
        t,
    );
    assert_eq!(conflict.status, 409);
    assert!(parse_json(&conflict)["error"].is_string());

    let ev_after = route(
        &surface,
        &get(
            &surface,
            "/api/session/events",
            vec![("after", "1"), ("wait_ms", "0")],
        ),
        t,
    );
    assert_eq!(ev_after.status, 200);
    let ev_after_json = parse_json(&ev_after);
    let answered_entries = ev_after_json["entries"].as_array().unwrap();
    assert_eq!(answered_entries.len(), 1);
    assert_eq!(answered_entries[0]["kind"], "approval_answered");
    assert_eq!(answered_entries[0]["id"], id);
    assert_eq!(answered_entries[0]["command"], cmd);
    assert_eq!(answered_entries[0]["answer"], "yes");

    let approver2 = surface.session.approver();
    let (done2_tx, done2_rx) = std::sync::mpsc::channel();
    let ask_thread2 = std::thread::spawn(move || {
        let _ = done2_tx.send(approver2.ask("curl https://example.com", "fetch"));
    });

    let ev2_resp = route(
        &surface,
        &get(
            &surface,
            "/api/session/events",
            vec![("after", "2"), ("wait_ms", "2000")],
        ),
        t,
    );
    let ev2_json = parse_json(&ev2_resp);
    let id2 = ev2_json["entries"][0]["id"].as_str().unwrap().to_string();

    let ok_no = route(
        &surface,
        &post_json(
            &surface,
            &format!("/api/session/approvals/{id2}"),
            &serde_json::json!({ "answer": "no" }),
        ),
        t,
    );
    assert_eq!(ok_no.status, 200);
    assert_eq!(
        done2_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        blyte_agent::approval::Approval::Denied
    );
    ask_thread2.join().unwrap();
}

fn start_stoppable_surface(
    repo: &Path,
) -> (u16, String, Arc<AtomicBool>, std::thread::JoinHandle<i32>) {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let repo_bg = repo.to_path_buf();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let args = SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
        };
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = |_: &str| {};
        run_with_hooks(
            &args,
            default_repl_config(repo_bg),
            &mut out,
            &mut err,
            &mut opener,
            stop_bg,
        )
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let line = loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for printed surface URL; stderr={}",
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        let stdout = String::from_utf8(out_buf.lock().unwrap().clone()).unwrap();
        if let Some(found) = stdout.lines().find(|l| l.contains("blyte: surface at ")) {
            break found.to_string();
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let url = line
        .split("blyte: surface at ")
        .nth(1)
        .unwrap()
        .trim()
        .to_string();
    let rest = url.strip_prefix("http://127.0.0.1:").unwrap();
    let (port_str, fragment) = rest.split_once("/#token=").unwrap();
    let port: u16 = port_str.parse().unwrap();
    (port, fragment.to_string(), stop, handle)
}

#[test]
fn every_api_get_rejects_a_missing_or_wrong_token() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-crate");
    init_git_repo(&repo);
    write_file(
        &repo.join("Cargo.toml"),
        "[package]\nname = \"demo-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(&repo.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
    write_file(&repo.join("README.md"), "hello\n");
    assert!(Command::new("git")
        .args(["add", "."])
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
    let t = now_secs();
    add_run_worktree(&repo, "T1");
    write_finished_run_summary(
        &repo,
        &d.path().join("tasks/t1.md"),
        "T1",
        &base_sha,
        t - 600,
    );

    let (surface, _) = make_surface(repo, Vec::new());
    let wrong = flip_last_hex_char(&surface.token);
    let routes = authenticated_api_get_routes();
    assert!(!routes.is_empty());

    for pattern in routes {
        let path = pattern.replace("{id}", "T1");
        let query = if pattern == "/api/session/events" {
            vec![("after", "0"), ("wait_ms", "0")]
        } else {
            vec![]
        };

        let missing = route(&surface, &get_headers(&path, query.clone(), vec![]), t);
        assert_eq!(
            missing.status, 403,
            "GET {path} without token must return 403, got {}",
            missing.status
        );
        assert!(parse_json(&missing)["error"].is_string());

        let bad = route(
            &surface,
            &get_headers(&path, query.clone(), vec![(TOKEN_HEADER, wrong.as_str())]),
            t,
        );
        assert_eq!(
            bad.status, 403,
            "GET {path} with wrong token must return 403, got {}",
            bad.status
        );
        assert!(parse_json(&bad)["error"].is_string());

        let ok = route(&surface, &get(&surface, &path, query), t);
        if pattern == "/api/models" {
            assert!(
                (200..300).contains(&ok.status) || ok.status == 502,
                "GET {path} with right token returned unexpected status {}",
                ok.status
            );
        } else {
            assert!(
                (200..300).contains(&ok.status),
                "GET {path} with right token must return 2xx, got {} ({})",
                ok.status,
                String::from_utf8_lossy(&ok.body)
            );
        }
    }

    for public_path in ["/", "/index.html", "/favicon.svg", "/fonts/archivo.woff2"] {
        let resp = route(&surface, &get_headers(public_path, vec![], vec![]), t);
        assert_eq!(
            resp.status, 200,
            "GET {public_path} must remain unauthenticated, got {}",
            resp.status
        );
    }
}

#[test]
fn health_answers_without_a_token_and_gives_only_the_session_id() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("demo-repo");
    init_git_repo(&repo);
    let (surface, _) = make_surface(repo, Vec::new());

    let resp = route(
        &surface,
        &get_headers("/api/health", vec![], vec![]),
        1_700_000_100,
    );
    assert_eq!(resp.status, 200);
    let val = parse_json(&resp);
    let obj = val.as_object().unwrap();
    let keys: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    assert_eq!(keys, BTreeSet::from(["session_id"]));
    assert_eq!(obj["session_id"], surface.session_id);

    let post_unauth = route(
        &surface,
        &post_json_headers("/api/health", &serde_json::json!({}), vec![]),
        1_700_000_100,
    );
    assert_eq!(post_unauth.status, 403);

    let post_auth = route(
        &surface,
        &post_json(&surface, "/api/health", &serde_json::json!({})),
        1_700_000_100,
    );
    assert_eq!(post_auth.status, 405);
}

#[test]
fn second_start_opens_the_page_with_the_token() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("attach-token-repo");
    init_git_repo(&repo);

    let (port, token, stop, handle) = start_stoppable_surface(&repo);

    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let mut opened = Vec::new();
    let args2 = SurfaceArgs {
        port: 0,
        no_open: false,
        days: 14,
    };
    let mut opener = |url: &str| opened.push(url.to_string());
    let code2 = run_with_hooks(
        &args2,
        default_repl_config(repo.clone()),
        &mut out2,
        &mut err2,
        &mut opener,
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(
        code2,
        0,
        "second start failed: stderr={}",
        String::from_utf8_lossy(&err2)
    );
    assert_eq!(opened.len(), 1);
    let expected_url = format!("http://127.0.0.1:{port}/#token={token}");
    assert_eq!(opened[0], expected_url);
    assert!(opened[0].ends_with(&format!("#token={token}")));

    stop.store(true, Ordering::Relaxed);
    assert_eq!(handle.join().unwrap(), 0);
}

#[test]
fn token_file_is_private_and_removed_on_shutdown() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("token-file-repo");
    init_git_repo(&repo);

    let (_port, token, stop, handle) = start_stoppable_surface(&repo);

    let token_path = repo.join(".wt/surface.token");
    let surface_path = repo.join(".wt/surface.json");
    assert!(
        token_path.exists(),
        "expected {} to exist",
        token_path.display()
    );
    assert!(surface_path.exists());

    let stored_token = std::fs::read_to_string(&token_path).unwrap();
    assert_eq!(stored_token.trim(), token);

    let surface_json = std::fs::read_to_string(&surface_path).unwrap();
    assert!(!surface_json.contains(&token));

    #[cfg(unix)]
    {
        let mode = std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected mode 0600, got {:o}", mode);
    }

    stop.store(true, Ordering::Relaxed);
    assert_eq!(handle.join().unwrap(), 0);

    assert!(
        !token_path.exists(),
        "expected {} to be deleted on shutdown",
        token_path.display()
    );
    assert!(
        !surface_path.exists(),
        "expected {} to be deleted on shutdown",
        surface_path.display()
    );
}

#[test]
fn second_start_fails_loudly_without_the_token_file() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("missing-token-file-repo");
    init_git_repo(&repo);

    let (_port, _token, stop, handle) = start_stoppable_surface(&repo);
    let token_path = repo.join(".wt/surface.token");
    assert!(token_path.exists());
    std::fs::remove_file(&token_path).unwrap();

    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let mut opened = Vec::new();
    let args2 = SurfaceArgs {
        port: 0,
        no_open: false,
        days: 14,
    };
    let mut opener = |url: &str| opened.push(url.to_string());
    let code2 = run_with_hooks(
        &args2,
        default_repl_config(repo.clone()),
        &mut out2,
        &mut err2,
        &mut opener,
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(code2, 1);
    let err2_str = String::from_utf8(err2).unwrap();
    assert!(
        err2_str.contains("surface.token"),
        "expected stderr to name the token file, got: {err2_str}"
    );
    assert!(
        opened.is_empty(),
        "must not open a tab without the token, opened: {opened:?}"
    );

    stop.store(true, Ordering::Relaxed);
    assert_eq!(handle.join().unwrap(), 0);
}

fn start_stoppable_surface_with_model(
    repo: &Path,
    model: &str,
) -> (u16, String, Arc<AtomicBool>, std::thread::JoinHandle<i32>) {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let repo_bg = repo.to_path_buf();
    let model_bg = model.to_string();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let args = SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
        };
        let mut cfg = default_repl_config(repo_bg);
        cfg.model = Some(model_bg);
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = |_: &str| {};
        run_with_hooks(&args, cfg, &mut out, &mut err, &mut opener, stop_bg)
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let line = loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for printed surface URL; stderr={}",
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        let stdout = String::from_utf8(out_buf.lock().unwrap().clone()).unwrap();
        if let Some(found) = stdout.lines().find(|l| l.contains("blyte: surface at ")) {
            break found.to_string();
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let url = line
        .split("blyte: surface at ")
        .nth(1)
        .unwrap()
        .trim()
        .to_string();
    let rest = url.strip_prefix("http://127.0.0.1:").unwrap();
    let (port_str, fragment) = rest.split_once("/#token=").unwrap();
    let port: u16 = port_str.parse().unwrap();
    (port, fragment.to_string(), stop, handle)
}

fn http_json_body(raw_resp: &str) -> serde_json::Value {
    let (_, body) = raw_resp
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("invalid HTTP response: {raw_resp}"));
    serde_json::from_str(body).unwrap_or_else(|e| panic!("invalid JSON body ({e}): {body}"))
}

#[test]
fn events_survive_a_restart() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("restart-events-repo");
    init_git_repo(&repo);

    let (port1, token1, stop1, handle1) =
        start_stoppable_surface_with_model(&repo, "unknown-model-for-test");

    let msg_body = r#"{"text":"hello before restart"}"#;
    let post_resp = send_http(
        port1,
        &format!(
            "POST /api/session/messages HTTP/1.1\r\nHost: 127.0.0.1:{port1}\r\nOrigin: http://127.0.0.1:{port1}\r\n{TOKEN_HEADER}: {token1}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{msg_body}",
            msg_body.len()
        ),
    );
    assert!(
        post_resp.starts_with("HTTP/1.1 200"),
        "POST /api/session/messages failed: {post_resp}"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let before_entries = loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for idle entry before restart"
        );
        let events_resp = send_http(
            port1,
            &format!(
                "GET /api/session/events?after=0 HTTP/1.1\r\nHost: 127.0.0.1:{port1}\r\n{TOKEN_HEADER}: {token1}\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(
            events_resp.starts_with("HTTP/1.1 200"),
            "GET /api/session/events failed: {events_resp}"
        );
        let parsed = http_json_body(&events_resp);
        let entries = parsed["entries"].as_array().unwrap().clone();
        if entries.iter().any(|e| e["kind"] == "idle") {
            break entries;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        before_entries
            .iter()
            .any(|e| e["kind"] == "user_message" && e["text"] == "hello before restart"),
        "expected user_message before restart, got: {before_entries:?}"
    );
    assert!(repo.join(".wt/sessions/entries.jsonl").exists());

    stop1.store(true, Ordering::Relaxed);
    assert_eq!(handle1.join().unwrap(), 0);

    let (port2, token2, stop2, handle2) =
        start_stoppable_surface_with_model(&repo, "unknown-model-for-test");

    let events2_resp = send_http(
        port2,
        &format!(
            "GET /api/session/events?after=0 HTTP/1.1\r\nHost: 127.0.0.1:{port2}\r\n{TOKEN_HEADER}: {token2}\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(
        events2_resp.starts_with("HTTP/1.1 200"),
        "GET /api/session/events after restart failed: {events2_resp}"
    );
    let parsed2 = http_json_body(&events2_resp);
    let after_entries = parsed2["entries"].as_array().unwrap();
    assert_eq!(after_entries.len(), before_entries.len() + 1);
    assert_eq!(&after_entries[..before_entries.len()], &before_entries[..]);

    let notice = after_entries.last().unwrap();
    let expected_seq = before_entries.last().unwrap()["seq"].as_u64().unwrap() + 1;
    assert_eq!(notice["seq"].as_u64(), Some(expected_seq));
    assert_eq!(notice["kind"].as_str(), Some("error"));
    assert_eq!(
        notice["message"].as_str(),
        Some(
            "blyte restarted: the entries above are from before; the model does not remember them"
        )
    );

    stop2.store(true, Ordering::Relaxed);
    assert_eq!(handle2.join().unwrap(), 0);
}

#[test]
fn runs_changed_needs_the_token() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("changed-token-repo");
    init_git_repo(&repo);
    let t = now_secs();
    let (surface, _calls) = make_surface(repo.clone(), vec![]);
    let wrong = flip_last_hex_char(&surface.token);

    let missing = route(
        &surface,
        &Request {
            method: "POST".to_string(),
            path: "/api/runs/changed".to_string(),
            query: Vec::new(),
            headers: Vec::new(),
            body: Vec::new(),
        },
        t,
    );
    assert_eq!(missing.status, 403);
    assert!(!surface.reconcile_requested.load(Ordering::Acquire));

    let bad = route(
        &surface,
        &Request {
            method: "POST".to_string(),
            path: "/api/runs/changed".to_string(),
            query: Vec::new(),
            headers: vec![(TOKEN_HEADER.to_string(), wrong)],
            body: Vec::new(),
        },
        t,
    );
    assert_eq!(bad.status, 403);
    assert!(!surface.reconcile_requested.load(Ordering::Acquire));

    let ok = route(
        &surface,
        &Request {
            method: "POST".to_string(),
            path: "/api/runs/changed".to_string(),
            query: Vec::new(),
            headers: vec![(TOKEN_HEADER.to_string(), surface.token.clone())],
            body: Vec::new(),
        },
        t,
    );
    assert_eq!(ok.status, 202);
    assert_eq!(parse_json(&ok), serde_json::json!({}));
    assert!(surface.reconcile_requested.load(Ordering::Acquire));

    let wrong_method = route(&surface, &get(&surface, "/api/runs/changed", vec![]), t);
    assert_eq!(wrong_method.status, 405);
}

#[test]
fn the_safety_net_interval_is_30_seconds() {
    assert_eq!(RECONCILE_SAFETY_NET_INTERVAL, Duration::from_secs(30));
}

#[test]
fn a_runs_changed_post_combines_without_waiting_for_the_timer() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("changed-combine-repo");
    let base_sha = init_repo_with_commit(&repo);
    write_file(&repo.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n");

    let (port, token, stop, handle) = start_stoppable_surface(&repo);

    std::thread::sleep(Duration::from_millis(250));

    let wt_run = repo.join(".wt/NC1");
    std::fs::create_dir_all(repo.join(".wt")).unwrap();
    assert!(Command::new("git")
        .args(["worktree", "add", "-q", "-b", "wip/NC1"])
        .arg(&wt_run)
        .arg(&base_sha)
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    write_file(&wt_run.join("notify.txt"), "notified\n");
    assert!(Command::new("git")
        .args(["add", "notify.txt"])
        .current_dir(&wt_run)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", "feat: notify"])
        .current_dir(&wt_run)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&wt_run)
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let now = now_secs();
    write_file(
        &repo.join(".wt/NC1.start.json"),
        &format!(
            r#"{{"id":"NC1","pid":999999,"started_at":{now},"base_sha":"{base_sha}","task_file":".wt/tasks/NC1.md","research":false,"request":"req-notify"}}"#
        ),
    );
    write_file(
        &repo.join(".wt/NC1.report.json"),
        r#"{"outcome":"verified","turns":2}"#,
    );
    write_file(
        &repo.join(".wt/NC1.json"),
        r#"{"id":"NC1","outcome":"verified","branch":"wip/NC1","turns":2,"seconds":5,"cost_usd":0.01,"final_message":"done"}"#,
    );
    write_file(
        &repo.join(".wt/NC1.checks.json"),
        r#"{"flags":[],"fallbacks":[]}"#,
    );

    let rec_path = repo.join(".wt/combined/req-notify.json");
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !rec_path.exists(),
        "run should not have been combined before POST /api/runs/changed"
    );

    let post_resp = send_http(
        port,
        &format!(
            "POST /api/runs/changed HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{TOKEN_HEADER}: {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(
        post_resp.starts_with("HTTP/1.1 202"),
        "expected 202 from POST /api/runs/changed, got: {post_resp}"
    );
    assert_eq!(http_json_body(&post_resp), serde_json::json!({}));

    let deadline = Instant::now() + Duration::from_secs(5);
    while !rec_path.is_file() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }

    stop.store(true, Ordering::Relaxed);
    assert_eq!(handle.join().unwrap(), 0);

    assert!(
        rec_path.is_file(),
        "expected {} to exist within 5 s after POST /api/runs/changed",
        rec_path.display()
    );
    let rec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&rec_path).unwrap()).unwrap();
    assert_eq!(rec["state"].as_str(), Some("combined"));
    assert_eq!(rec["branch"].as_str(), Some("blyte/req-notify"));
}

struct BlockingRunner {
    entered: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

struct BlockingHandle {
    entered: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::SyncSender<()>,
}

impl BlockingRunner {
    fn new() -> (Self, BlockingHandle) {
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        (
            Self {
                entered: entered_tx,
                release: release_rx,
            },
            BlockingHandle {
                entered: entered_rx,
                release: release_tx,
            },
        )
    }
}

impl PromptRunner for BlockingRunner {
    fn run(
        &mut self,
        _prompt: &str,
        _model: &str,
        _emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        let _ = self.entered.send(());
        let _ = self.release.recv();
        Ok(())
    }
}

fn make_busy_surface(root: PathBuf) -> (Surface, BlockingHandle) {
    let repo = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap()
        .to_string();
    let (runner, handle) = BlockingRunner::new();
    let session = Session::start(Box::new(runner), "gemini-4-neon".to_string());
    let surface = Surface {
        root,
        repo,
        session,
        session_id: "sess-test-busy".to_string(),
        started_at: 1_700_000_000,
        days: 14,
        token: generate_token().unwrap(),
        requests: Arc::new(blyte_cli::requests::RequestsState::new()),
        publisher: Arc::new(blyte_cli::requests::RealPublisher),
        reconcile_lock: Mutex::new(()),
        reconcile_requested: std::sync::atomic::AtomicBool::new(false),
    };
    surface.session.submit("hold", Vec::new()).unwrap();
    handle.entered.recv_timeout(Duration::from_secs(5)).unwrap();
    (surface, handle)
}

fn create_verified_request_run(
    repo: &Path,
    base_sha: &str,
    id: &str,
    request: &str,
    file_name: &str,
    content: &str,
    started_at: u64,
) {
    let wt_run = repo.join(".wt").join(id);
    std::fs::create_dir_all(repo.join(".wt")).unwrap();
    let branch = format!("wip/{id}");
    assert!(Command::new("git")
        .args(["worktree", "add", "-q", "-b", &branch])
        .arg(&wt_run)
        .arg(base_sha)
        .current_dir(repo)
        .status()
        .unwrap()
        .success());
    write_file(&wt_run.join(file_name), content);
    assert!(Command::new("git")
        .args(["add", file_name])
        .current_dir(&wt_run)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", &format!("feat: {id}")])
        .current_dir(&wt_run)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&wt_run)
        .current_dir(repo)
        .status()
        .unwrap()
        .success());

    write_file(
        &repo.join(format!(".wt/tasks/{id}.md")),
        &format!("# Task {id}\n"),
    );
    write_file(
        &repo.join(format!(".wt/{id}.start.json")),
        &format!(
            r#"{{"id":"{id}","pid":999999,"started_at":{started_at},"base_sha":"{base_sha}","task_file":".wt/tasks/{id}.md","research":false,"request":"{request}"}}"#
        ),
    );
    write_file(
        &repo.join(format!(".wt/{id}.report.json")),
        r#"{"outcome":"verified","turns":2}"#,
    );
    write_file(
        &repo.join(format!(".wt/{id}.json")),
        &format!(
            r#"{{"id":"{id}","outcome":"verified","branch":"{branch}","turns":2,"seconds":5,"cost_usd":0.01,"final_message":"done"}}"#
        ),
    );
    write_file(
        &repo.join(format!(".wt/{id}.checks.json")),
        r#"{"flags":[],"fallbacks":[]}"#,
    );
}

fn add_commit_to_run_branch(repo: &Path, id: &str, file_name: &str, content: &str) {
    let wt_tmp = repo.join(".wt").join(format!("{id}-amend"));
    let branch = format!("wip/{id}");
    assert!(Command::new("git")
        .args(["worktree", "add", "-q"])
        .arg(&wt_tmp)
        .arg(&branch)
        .current_dir(repo)
        .status()
        .unwrap()
        .success());
    write_file(&wt_tmp.join(file_name), content);
    assert!(Command::new("git")
        .args(["add", file_name])
        .current_dir(&wt_tmp)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-q", "-m", &format!("fix: update {id}")])
        .current_dir(&wt_tmp)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&wt_tmp)
        .current_dir(repo)
        .status()
        .unwrap()
        .success());
}

#[test]
fn a_combined_run_without_a_verdict_is_marked_combined() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("combined-marked-repo");
    let base_sha = init_repo_with_commit(&repo);
    write_file(&repo.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n");
    let now = now_secs();
    create_verified_request_run(&repo, &base_sha, "CM1", "req-cm", "cm1.txt", "one\n", now);

    let (surface, _calls) = make_surface(repo.clone(), vec![]);
    assert!(blyte_cli::requests::maybe_combine(&surface, "req-cm").unwrap());
    surface.requests.wait_for_idle("req-cm");

    let resp = route(&surface, &get(&surface, "/api/runs", vec![]), now + 1);
    assert_eq!(resp.status, 200);
    let body = parse_json(&resp);
    let runs = body["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"], "CM1");
    assert_eq!(runs[0]["combined"], true);
    assert!(runs[0]["verdict"].is_null());
    assert_eq!(
        blyte_cli::requests::run_is_combined(&repo, "CM1", Some("req-cm")),
        Ok(true)
    );
    assert_eq!(
        blyte_cli::requests::run_is_combined(&repo, "CM1", None),
        Ok(false)
    );
    assert_eq!(
        blyte_cli::requests::run_is_combined(&repo, "OTHER", Some("req-cm")),
        Ok(false)
    );
}

#[test]
fn a_run_whose_request_is_not_combined_is_not_marked() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("uncombined-repo");
    let base_sha = init_repo_with_commit(&repo);
    write_file(&repo.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n");
    let now = now_secs();
    create_verified_request_run(&repo, &base_sha, "UC1", "req-uc", "uc1.txt", "one\n", now);

    let (surface, handle) = make_busy_surface(repo.clone());
    let resp = route(&surface, &get(&surface, "/api/runs", vec![]), now + 1);
    let _ = handle.release.send(());

    assert_eq!(resp.status, 200);
    let body = parse_json(&resp);
    let runs = body["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"], "UC1");
    assert_eq!(runs[0]["combined"], false);
    assert!(!repo.join(".wt/combined/req-uc.json").exists());
}

#[test]
fn a_run_changed_after_the_combine_is_not_marked() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("stale-combine-repo");
    let base_sha = init_repo_with_commit(&repo);
    write_file(&repo.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n");
    let now = now_secs();
    create_verified_request_run(&repo, &base_sha, "SC1", "req-sc", "sc1.txt", "v1\n", now);

    let (combine_surface, _calls) = make_surface(repo.clone(), vec![]);
    assert!(blyte_cli::requests::maybe_combine(&combine_surface, "req-sc").unwrap());
    combine_surface.requests.wait_for_idle("req-sc");

    add_commit_to_run_branch(&repo, "SC1", "sc1.txt", "v2\n");

    let (surface, handle) = make_busy_surface(repo.clone());
    let resp = route(&surface, &get(&surface, "/api/runs", vec![]), now + 2);
    let _ = handle.release.send(());

    assert_eq!(resp.status, 200);
    let body = parse_json(&resp);
    let runs = body["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"], "SC1");
    assert_eq!(runs[0]["combined"], false);
}

#[test]
fn a_run_in_a_failed_combine_is_not_marked() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("failed-combine-repo");
    let base_sha = init_repo_with_commit(&repo);
    write_file(&repo.join("blyte.toml"), "[dispatch]\ngate = \"false\"\n");
    let now = now_secs();
    create_verified_request_run(&repo, &base_sha, "FC1", "req-fc", "fc1.txt", "one\n", now);

    let (surface, _calls) = make_surface(repo.clone(), vec![]);
    assert!(blyte_cli::requests::maybe_combine(&surface, "req-fc").unwrap());
    surface.requests.wait_for_idle("req-fc");

    let rec_path = repo.join(".wt/combined/req-fc.json");
    let rec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&rec_path).unwrap()).unwrap();
    assert_eq!(rec["state"].as_str(), Some("check_failed"));

    let resp = route(&surface, &get(&surface, "/api/runs", vec![]), now + 1);
    assert_eq!(resp.status, 200);
    let body = parse_json(&resp);
    let runs = body["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"], "FC1");
    assert_eq!(runs[0]["combined"], false);
}

#[test]
fn an_unreadable_combine_record_is_a_500_naming_the_file() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("bad-combine-record-repo");
    let base_sha = init_repo_with_commit(&repo);
    write_file(&repo.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n");
    let now = now_secs();
    create_verified_request_run(&repo, &base_sha, "BR1", "req-bad", "br1.txt", "one\n", now);

    let bad_path = repo.join(".wt/combined/req-bad.json");
    write_file(&bad_path, "{not valid json");

    let (surface, handle) = make_busy_surface(repo.clone());
    let resp = route(&surface, &get(&surface, "/api/runs", vec![]), now + 1);
    let _ = handle.release.send(());

    assert_eq!(resp.status, 500);
    let body = String::from_utf8_lossy(&resp.body);
    assert!(
        body.contains(&bad_path.display().to_string()),
        "expected 500 body to name {}, got: {body}",
        bad_path.display()
    );
}
