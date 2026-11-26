use std::collections::{BTreeSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use blyte_agent::approval::Approver;
use blyte_agent::sandbox::{surface_message_path_for_id, surface_token_path_for_id};
use blyte_cli::cli::SurfaceArgs;
use blyte_cli::http::{Request, Response};
use blyte_cli::repl::ReplConfig;
use blyte_cli::serve::{
    authenticated_api_get_routes, constant_time_eq, generate_token, route, run_with_hooks,
    run_with_io, run_with_runner, Surface, MESSAGE_HEADER, RECONCILE_SAFETY_NET_INTERVAL,
    TOKEN_HEADER,
};
use blyte_cli::session::{peer_prompt, EntryBody, PromptRunner, Session};
use blyte_cli::surface_dir::{entries_path, list_records, record_path, surfaces_dir};
use blyte_cli::surface_registry::{entry_path, registry_dir, write_entry, RegistryEntry};

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
            r#"{{"id":"{id}","pid":999999,"started_at":{started_at},"base_sha":"{base_sha}","task_file":"{}","research":false,"surface":"s00000000000000aa"}}"#,
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
        surface_id: "s00000000000000aa".to_string(),
        surface_name: repo.clone(),
        root,
        repo,
        session,
        session_id: "sess-test-1".to_string(),
        started_at: 1_700_000_000,
        days: 14,
        token: generate_token().unwrap(),
        message_credential: generate_token().unwrap(),
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

fn ensure_isolated_state_home() {
    static STATE_HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    STATE_HOME.get_or_init(|| {
        let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
        std::fs::create_dir_all(base).unwrap();
        let dir = tempfile::tempdir_in(base).unwrap();
        std::env::set_var("XDG_STATE_HOME", dir.path());
        std::env::set_var("BLYTE_STATE_DIR", dir.path().join("blyte"));
        dir
    });
}

fn default_repl_config(repo_root: PathBuf) -> ReplConfig {
    ensure_isolated_state_home();
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
        "surface_name",
        "repo",
        "repository",
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
    assert_eq!(obj.get("surface_name").unwrap(), "demo-repo");
    assert_eq!(obj.get("repo").unwrap(), "demo-repo");
    assert_eq!(obj.get("repository").unwrap(), true);
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
            r#"{{"id":"T1","pid":999999,"started_at":{},"base_sha":"{base_sha}","task_file":"{}","research":false,"surface":"s00000000000000aa"}}"#,
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
        &repo.join(".wt/T1.start.json"),
        &format!(r#"{{"id":"T1","started_at":{t},"surface":"s00000000000000aa"}}"#),
    );
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
        &repo.join(".wt/T1.start.json"),
        &format!(r#"{{"id":"T1","started_at":{t},"surface":"s00000000000000aa"}}"#),
    );
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
    let repo = repo_dir.path().join("demo-repo");
    let home = tempfile::tempdir().unwrap();
    init_repo_with_commit(&repo);

    let mut child = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["--no-open", "--port", "0"])
        .current_dir(&repo)
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
                let is_ready = parse_surface_ready_line(&line).is_some();
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

    assert!(
        !repo.join(".wt/surface.json").exists(),
        "legacy .wt/surface.json must not be created"
    );
    let records = list_records(&repo).unwrap();
    assert_eq!(
        records.len(),
        1,
        "expected one surface record after startup, got {records:?}"
    );
    let surface_file = record_path(&repo, &records[0].id);
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
        surface_file.exists(),
        "expected {} to remain on shutdown so surface can be resumed",
        surface_file.display()
    );
}

fn parse_surface_ready_line(line: &str) -> Option<(String, u16, String)> {
    let rest = line.trim().strip_prefix("blyte: surface ")?;
    let (name, url) = rest.split_once(" at ")?;
    let after_scheme = url.trim().strip_prefix("http://127.0.0.1:")?;
    let (port_str, token) = after_scheme.split_once("/#token=")?;
    let port: u16 = port_str.parse().ok()?;
    Some((name.to_string(), port, token.to_string()))
}

fn fetch_health(port: u16) -> serde_json::Value {
    let resp = send_http(
        port,
        &format!("GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
    );
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "GET /api/health failed: {resp}"
    );
    http_json_body(&resp)
}

struct RunningSurface {
    name: String,
    port: u16,
    token: String,
    stop: Arc<AtomicBool>,
    err_buf: Arc<Mutex<Vec<u8>>>,
    handle: std::thread::JoinHandle<i32>,
}

fn start_stoppable_surface_with_args(
    repo: &Path,
    args: SurfaceArgs,
    model: Option<&str>,
) -> RunningSurface {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let repo_bg = repo.to_path_buf();
    let model_bg = model.map(str::to_string);
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let mut cfg = default_repl_config(repo_bg);
        if let Some(m) = model_bg {
            cfg.model = Some(m);
        }
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = |_: &str| {};
        run_with_hooks(&args, cfg, &mut out, &mut err, &mut opener, stop_bg)
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let (name, port, token) = loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for printed surface URL; stderr={}",
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        let stdout = String::from_utf8(out_buf.lock().unwrap().clone()).unwrap();
        if let Some(parsed) = stdout.lines().find_map(parse_surface_ready_line) {
            break parsed;
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    RunningSurface {
        name,
        port,
        token,
        stop,
        err_buf,
        handle,
    }
}

#[test]
fn two_surfaces_in_same_checkout_run_independently() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("multi-repo");
    init_git_repo(&repo);

    let RunningSurface {
        name: name1,
        port: port1,
        token: token1,
        stop: stop1,
        handle: handle1,
        ..
    } = start_stoppable_surface_with_args(
        &repo,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
        None,
    );
    let RunningSurface {
        name: name2,
        port: port2,
        token: token2,
        stop: stop2,
        handle: handle2,
        ..
    } = start_stoppable_surface_with_args(
        &repo,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
        None,
    );

    assert_ne!(port1, port2);
    assert_ne!(token1, token2);
    assert_eq!(name1, "multi-repo");
    assert_eq!(name2, "multi-repo-2");

    let health1 = fetch_health(port1);
    let health2 = fetch_health(port2);
    let id1 = health1["surface_id"].as_str().unwrap().to_string();
    let id2 = health2["surface_id"].as_str().unwrap().to_string();
    assert_ne!(id1, id2);
    assert_eq!(health1["surface_name"].as_str(), Some("multi-repo"));
    assert_eq!(health2["surface_name"].as_str(), Some("multi-repo-2"));

    let surface_dirs: Vec<PathBuf> = std::fs::read_dir(surfaces_dir(&repo))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(surface_dirs.len(), 2, "{surface_dirs:?}");

    let tok_path1 = surface_token_path_for_id(&id1).unwrap();
    let tok_path2 = surface_token_path_for_id(&id2).unwrap();
    assert!(tok_path1.exists());
    assert!(tok_path2.exists());
    assert_ne!(tok_path1, tok_path2);

    let cross1 = send_http(
        port1,
        &format!(
            "GET /api/session HTTP/1.1\r\nHost: 127.0.0.1:{port1}\r\n{TOKEN_HEADER}: {token2}\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(
        cross1.starts_with("HTTP/1.1 403"),
        "token2 must be refused by surface1: {cross1}"
    );
    let cross2 = send_http(
        port2,
        &format!(
            "GET /api/session HTTP/1.1\r\nHost: 127.0.0.1:{port2}\r\n{TOKEN_HEADER}: {token1}\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(
        cross2.starts_with("HTTP/1.1 403"),
        "token1 must be refused by surface2: {cross2}"
    );

    assert!(!repo.join(".wt/surface.json").exists());
    assert!(!repo.join(".wt/sessions").exists());

    stop1.store(true, Ordering::Relaxed);
    stop2.store(true, Ordering::Relaxed);
    assert_eq!(handle1.join().unwrap(), 0);
    assert_eq!(handle2.join().unwrap(), 0);
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
            ..Default::default()
        };
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let _ = run_with_io(&args, default_repl_config(repo_bg), &mut out, &mut err);
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for printed surface URL; stderr={}",
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        let stdout = String::from_utf8(out_buf.lock().unwrap().clone()).unwrap();
        if let Some((_name, port, token)) = stdout.lines().find_map(parse_surface_ready_line) {
            return (port, token);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
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
        &repo.join(".wt/T1.start.json"),
        &format!(r#"{{"id":"T1","started_at":{t},"surface":"s00000000000000aa"}}"#),
    );
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
    assert!(!wt_dir.join("surface.json").exists());
    assert!(!wt_dir.join("surface.token").exists());
    assert!(!wt_dir.join("sessions").exists());
    assert_eq!(list_records(&repo).unwrap().len(), 1);
    assert_no_file_under_contains(&wt_dir, &token);
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
    let running = start_stoppable_surface_with_args(
        repo,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
        None,
    );
    (running.port, running.token, running.stop, running.handle)
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
fn health_answers_without_a_token_and_gives_surface_and_session_ids() {
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
    assert_eq!(
        keys,
        BTreeSet::from(["session_id", "surface_id", "surface_name"])
    );
    assert_eq!(obj["session_id"], surface.session_id);
    assert_eq!(obj["surface_id"], surface.surface_id);
    assert_eq!(obj["surface_name"], surface.surface_name);

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
fn duplicate_name_while_running_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("dup-name-repo");
    init_git_repo(&repo);

    let RunningSurface { stop, handle, .. } = start_stoppable_surface_with_args(
        &repo,
        SurfaceArgs {
            no_open: true,
            name: Some("x".to_string()),
            ..Default::default()
        },
        None,
    );

    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let mut opener = |_: &str| {};
    let code2 = run_with_hooks(
        &SurfaceArgs {
            no_open: true,
            name: Some("x".to_string()),
            ..Default::default()
        },
        default_repl_config(repo.clone()),
        &mut out2,
        &mut err2,
        &mut opener,
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(code2, 1);
    let err2_str = String::from_utf8(err2).unwrap();
    assert!(
        err2_str.contains("already named x"),
        "expected 'already named x', got: {err2_str}"
    );

    stop.store(true, Ordering::Relaxed);
    assert_eq!(handle.join().unwrap(), 0);
}

#[test]
fn token_file_is_private_and_removed_on_shutdown() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("token-file-repo");
    init_git_repo(&repo);

    let (port, token, stop, handle) = start_stoppable_surface(&repo);

    let health = fetch_health(port);
    let surface_id = health["surface_id"].as_str().unwrap().to_string();
    let token_path = surface_token_path_for_id(&surface_id).unwrap();
    let surface_path = record_path(&repo, &surface_id);
    let entries_file = entries_path(&repo, &surface_id);
    assert!(
        !repo.join(".wt/surface.json").exists(),
        ".wt/surface.json must not be created"
    );
    assert!(
        !repo.join(".wt/surface.token").exists(),
        ".wt/surface.token must not be created"
    );
    assert!(
        !repo.join(".wt/sessions").exists(),
        ".wt/sessions must not be created"
    );
    assert!(
        token_path.exists(),
        "expected {} to exist",
        token_path.display()
    );
    assert!(surface_path.exists());
    assert!(entries_file.exists());

    let stored_token = std::fs::read_to_string(&token_path).unwrap();
    assert_eq!(stored_token.trim(), token);

    let surface_json = std::fs::read_to_string(&surface_path).unwrap();
    assert!(!surface_json.contains(&token));

    #[cfg(unix)]
    {
        let mode = std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected mode 0600, got {:o}", mode);
        let dir_mode = std::fs::metadata(token_path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "expected dir mode 0700, got {:o}",
            dir_mode
        );
    }

    stop.store(true, Ordering::Relaxed);
    assert_eq!(handle.join().unwrap(), 0);

    assert!(
        !token_path.exists(),
        "expected {} to be deleted on shutdown",
        token_path.display()
    );
    assert!(
        surface_path.exists(),
        "expected {} to remain on shutdown",
        surface_path.display()
    );
    assert!(
        entries_file.exists(),
        "expected {} to remain on shutdown",
        entries_file.display()
    );
}

#[test]
fn resume_while_running_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("resume-running-repo");
    init_git_repo(&repo);

    let RunningSurface {
        name, stop, handle, ..
    } = start_stoppable_surface_with_args(
        &repo,
        SurfaceArgs {
            no_open: true,
            name: Some("alpha".to_string()),
            ..Default::default()
        },
        None,
    );
    assert_eq!(name, "alpha");

    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let mut opener = |_: &str| {};
    let code2 = run_with_hooks(
        &SurfaceArgs {
            no_open: true,
            resume: Some("alpha".to_string()),
            ..Default::default()
        },
        default_repl_config(repo.clone()),
        &mut out2,
        &mut err2,
        &mut opener,
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(code2, 1);
    let err2_str = String::from_utf8(err2).unwrap();
    assert!(
        err2_str.contains("already running"),
        "expected 'already running', got: {err2_str}"
    );

    stop.store(true, Ordering::Relaxed);
    assert_eq!(handle.join().unwrap(), 0);
}

#[test]
fn resume_missing_name_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("resume-missing-repo");
    init_git_repo(&repo);

    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut opener = |_: &str| {};
    let code = run_with_hooks(
        &SurfaceArgs {
            no_open: true,
            resume: Some("nosuch".to_string()),
            ..Default::default()
        },
        default_repl_config(repo),
        &mut out,
        &mut err,
        &mut opener,
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(code, 1);
    let err_str = String::from_utf8(err).unwrap();
    assert!(
        err_str.contains("no stopped surface named nosuch"),
        "expected 'no stopped surface named nosuch', got: {err_str}"
    );
}

#[test]
fn linked_worktree_uses_main_checkout() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("main-repo");
    init_repo_with_commit(&repo);
    let linked = d.path().join("linked-wt");
    assert!(Command::new("git")
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            "wt-branch",
            linked.to_str().unwrap(),
        ])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let RunningSurface {
        name,
        port,
        stop,
        err_buf,
        handle,
        ..
    } = start_stoppable_surface_with_args(
        &linked,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
        None,
    );
    assert_eq!(name, "main-repo");

    let health = fetch_health(port);
    let surface_id = health["surface_id"].as_str().unwrap().to_string();
    let err_str = String::from_utf8(err_buf.lock().unwrap().clone()).unwrap();
    assert!(
        err_str.contains("linked worktree"),
        "expected 'linked worktree' on stderr, got: {err_str}"
    );
    assert!(
        record_path(&repo, &surface_id).exists(),
        "expected surface record under main checkout"
    );
    assert!(
        !linked.join(".wt").exists(),
        "linked worktree must not get a .wt directory"
    );

    stop.store(true, Ordering::Relaxed);
    assert_eq!(handle.join().unwrap(), 0);
}

#[test]
fn bound_port_returns_error() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("bound-port-repo");
    init_git_repo(&repo);

    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let busy_port = occupied.local_addr().unwrap().port();

    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut opener = |_: &str| {};
    let code = run_with_hooks(
        &SurfaceArgs {
            port: busy_port,
            no_open: true,
            ..Default::default()
        },
        default_repl_config(repo),
        &mut out,
        &mut err,
        &mut opener,
        Arc::new(AtomicBool::new(false)),
    );
    drop(occupied);

    assert_eq!(code, 1);
    let err_str = String::from_utf8(err).unwrap();
    assert!(
        err_str.contains("failed to bind"),
        "expected 'failed to bind', got: {err_str}"
    );
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

    let RunningSurface {
        name: name1,
        port: port1,
        token: token1,
        stop: stop1,
        handle: handle1,
        ..
    } = start_stoppable_surface_with_args(
        &repo,
        SurfaceArgs {
            no_open: true,
            ..Default::default()
        },
        Some("unknown-model-for-test"),
    );
    let health1 = fetch_health(port1);
    let id1 = health1["surface_id"].as_str().unwrap().to_string();

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
    assert!(entries_path(&repo, &id1).exists());
    assert!(!repo.join(".wt/sessions").exists());

    stop1.store(true, Ordering::Relaxed);
    assert_eq!(handle1.join().unwrap(), 0);

    let RunningSurface {
        name: name2,
        port: port2,
        token: token2,
        stop: stop2,
        handle: handle2,
        ..
    } = start_stoppable_surface_with_args(
        &repo,
        SurfaceArgs {
            no_open: true,
            resume: Some(name1.clone()),
            ..Default::default()
        },
        Some("unknown-model-for-test"),
    );
    assert_eq!(name2, name1);
    let health2 = fetch_health(port2);
    assert_eq!(health2["surface_id"].as_str(), Some(id1.as_str()));
    assert_eq!(health2["surface_name"].as_str(), Some(name1.as_str()));

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
    let surface_id = blyte_cli::surface_dir::list_records(&repo)
        .unwrap()
        .into_iter()
        .next()
        .expect("surface record")
        .id;
    write_file(
        &repo.join(".wt/NC1.start.json"),
        &format!(
            r#"{{"id":"NC1","pid":999999,"started_at":{now},"base_sha":"{base_sha}","task_file":".wt/tasks/NC1.md","research":false,"request":"req-notify","surface":"{surface_id}"}}"#
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
        surface_id: "s00000000000000aa".to_string(),
        surface_name: repo.clone(),
        root,
        repo,
        session,
        session_id: "sess-test-busy".to_string(),
        started_at: 1_700_000_000,
        days: 14,
        token: generate_token().unwrap(),
        message_credential: generate_token().unwrap(),
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
            r#"{{"id":"{id}","pid":999999,"started_at":{started_at},"base_sha":"{base_sha}","task_file":".wt/tasks/{id}.md","research":false,"request":"{request}","surface":"s00000000000000aa"}}"#
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

#[test]
fn a_combined_run_whose_branch_was_removed_is_still_marked() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("removed-branch-repo");
    let base_sha = init_repo_with_commit(&repo);
    write_file(&repo.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n");
    let now = now_secs();
    create_verified_request_run(&repo, &base_sha, "RB1", "req-rb", "rb1.txt", "one\n", now);

    let (surface, _calls) = make_surface(repo.clone(), vec![]);
    assert!(blyte_cli::requests::maybe_combine(&surface, "req-rb").unwrap());
    surface.requests.wait_for_idle("req-rb");

    assert!(Command::new("git")
        .args(["branch", "-D", "wip/RB1"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let resp = route(&surface, &get(&surface, "/api/runs", vec![]), now + 1);
    assert_eq!(resp.status, 200);
    let body = parse_json(&resp);
    let runs = body["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"], "RB1");
    assert_eq!(runs[0]["combined"], true);
}

#[test]
fn a_new_run_in_a_combined_request_does_not_break_the_runs_list() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("new-run-in-combined-repo");
    let base_sha = init_repo_with_commit(&repo);
    write_file(&repo.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n");
    let now = now_secs();
    create_verified_request_run(&repo, &base_sha, "NR1", "req-nr", "nr1.txt", "one\n", now);

    let (surface, _calls) = make_surface(repo.clone(), vec![]);
    assert!(blyte_cli::requests::maybe_combine(&surface, "req-nr").unwrap());
    surface.requests.wait_for_idle("req-nr");

    write_file(&repo.join(".wt/tasks/NR2.md"), "# Task NR2\n");
    write_file(
        &repo.join(".wt/NR2.start.json"),
        &format!(
            r#"{{"id":"NR2","pid":{},"started_at":{},"base_sha":"{base_sha}","task_file":".wt/tasks/NR2.md","research":false,"request":"req-nr","surface":"s00000000000000aa"}}"#,
            std::process::id(),
            now + 1
        ),
    );

    let resp = route(&surface, &get(&surface, "/api/runs", vec![]), now + 2);
    assert_eq!(resp.status, 200);
    let body = parse_json(&resp);
    let runs = body["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"], "NR1");
    assert_eq!(runs[0]["combined"], true);
}

#[test]
fn a_second_finished_run_not_in_the_record_is_not_marked() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("second-run-unrecorded-repo");
    let base_sha = init_repo_with_commit(&repo);
    write_file(&repo.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n");
    let now = now_secs();
    create_verified_request_run(&repo, &base_sha, "SR1", "req-sr", "sr1.txt", "one\n", now);

    let (combine_surface, _calls) = make_surface(repo.clone(), vec![]);
    assert!(blyte_cli::requests::maybe_combine(&combine_surface, "req-sr").unwrap());
    combine_surface.requests.wait_for_idle("req-sr");

    create_verified_request_run(
        &repo,
        &base_sha,
        "SR2",
        "req-sr",
        "sr2.txt",
        "two\n",
        now + 1,
    );

    let (surface, handle) = make_busy_surface(repo.clone());
    let resp = route(&surface, &get(&surface, "/api/runs", vec![]), now + 2);
    let _ = handle.release.send(());

    assert_eq!(resp.status, 200);
    let body = parse_json(&resp);
    let runs = body["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    let sr1 = runs.iter().find(|r| r["id"] == "SR1").unwrap();
    let sr2 = runs.iter().find(|r| r["id"] == "SR2").unwrap();
    assert_eq!(sr1["combined"], true);
    assert_eq!(sr2["combined"], false);
}

#[test]
fn polling_a_done_request_records_no_session_error_and_refuses_push_and_pr() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("done-request-serve-repo");
    let base_sha = init_repo_with_commit(&repo);
    write_file(&repo.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n");
    let now = now_secs();
    create_verified_request_run(&repo, &base_sha, "DR1", "req-done", "dr1.txt", "one\n", now);

    let (surface, _calls) = make_surface(repo.clone(), vec![]);
    assert!(blyte_cli::requests::maybe_combine(&surface, "req-done").unwrap());
    surface.requests.wait_for_idle("req-done");

    assert!(Command::new("git")
        .args(["branch", "-D", "wip/DR1"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let poll1 = route(&surface, &get(&surface, "/api/requests", vec![]), now + 1);
    assert_eq!(poll1.status, 200);
    let body1 = parse_json(&poll1);
    assert_eq!(body1[0]["request"], "req-done");
    assert_eq!(body1[0]["state"], "done");
    assert!(body1[0]["error"].is_null());

    let poll2 = route(&surface, &get(&surface, "/api/requests", vec![]), now + 2);
    assert_eq!(poll2.status, 200);
    let body2 = parse_json(&poll2);
    assert_eq!(body2[0]["request"], "req-done");
    assert_eq!(body2[0]["state"], "done");
    assert!(body2[0]["error"].is_null());

    let runs_poll = route(&surface, &get(&surface, "/api/runs", vec![]), now + 3);
    assert_eq!(runs_poll.status, 200);

    let events_resp = route(
        &surface,
        &get(&surface, "/api/session/events", vec![("after", "0")]),
        now + 4,
    );
    assert_eq!(events_resp.status, 200);
    let events = parse_json(&events_resp);
    let error_entries: Vec<&serde_json::Value> = events["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "error")
        .collect();
    assert!(
        error_entries.is_empty(),
        "expected no session errors, got: {error_entries:?}"
    );

    let push_resp = route(
        &surface,
        &post_json(
            &surface,
            "/api/requests/req-done/push",
            &serde_json::json!({}),
        ),
        now + 5,
    );
    assert_eq!(push_resp.status, 409);
    let push_body = String::from_utf8_lossy(&push_resp.body);
    assert!(
        push_body.contains("already merged and removed"),
        "push body: {push_body}"
    );

    let pr_resp = route(
        &surface,
        &post_json(
            &surface,
            "/api/requests/req-done/pull-request",
            &serde_json::json!({"title": "PR title", "body": "PR body"}),
        ),
        now + 6,
    );
    assert_eq!(pr_resp.status, 409);
    let pr_body = String::from_utf8_lossy(&pr_resp.body);
    assert!(
        pr_body.contains("already merged and removed"),
        "pr body: {pr_body}"
    );
}

#[test]
fn api_runs_lists_only_owned_runs() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("owned-runs-repo");
    let base_sha = init_repo_with_commit(&repo);
    let t = now_secs();

    add_run_worktree(&repo, "RAA");
    add_run_worktree(&repo, "RBB");
    add_run_worktree(&repo, "RNULL");

    write_finished_run_summary(
        &repo,
        &d.path().join("tasks/raa.md"),
        "RAA",
        &base_sha,
        t - 60,
    );
    write_finished_run_summary(
        &repo,
        &d.path().join("tasks/rbb.md"),
        "RBB",
        &base_sha,
        t - 50,
    );
    write_finished_run_summary(
        &repo,
        &d.path().join("tasks/rnull.md"),
        "RNULL",
        &base_sha,
        t - 40,
    );
    write_file(
        &repo.join(".wt/RBB.start.json"),
        &format!(
            r#"{{"id":"RBB","pid":999999,"started_at":{},"base_sha":"{base_sha}","task_file":"{}","research":false,"surface":"s00000000000000bb"}}"#,
            t - 50,
            d.path().join("tasks/rbb.md").display()
        ),
    );
    write_file(
        &repo.join(".wt/RNULL.start.json"),
        &format!(
            r#"{{"id":"RNULL","pid":999999,"started_at":{},"base_sha":"{base_sha}","task_file":"{}","research":false,"surface":null}}"#,
            t - 40,
            d.path().join("tasks/rnull.md").display()
        ),
    );

    let (surface, _) = make_surface(repo, Vec::new());
    let resp = route(&surface, &get(&surface, "/api/runs", vec![]), t);
    assert_eq!(resp.status, 200);
    let body = parse_json(&resp);
    let ids: Vec<&str> = body["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["RAA"]);
}

#[test]
fn decision_and_diff_for_another_surfaces_run_return_403_and_write_nothing() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("refuse-other-run-repo");
    let base_sha = init_repo_with_commit(&repo);
    let t = now_secs();

    add_run_worktree(&repo, "RBB");
    write_finished_run_summary(
        &repo,
        &d.path().join("tasks/rbb.md"),
        "RBB",
        &base_sha,
        t - 50,
    );
    write_file(
        &repo.join(".wt/RBB.start.json"),
        &format!(
            r#"{{"id":"RBB","pid":999999,"started_at":{},"base_sha":"{base_sha}","task_file":"{}","research":false,"surface":"s00000000000000bb"}}"#,
            t - 50,
            d.path().join("tasks/rbb.md").display()
        ),
    );

    add_run_worktree(&repo, "RNULL");
    write_finished_run_summary(
        &repo,
        &d.path().join("tasks/rnull.md"),
        "RNULL",
        &base_sha,
        t - 40,
    );
    write_file(
        &repo.join(".wt/RNULL.start.json"),
        &format!(
            r#"{{"id":"RNULL","pid":999999,"started_at":{},"base_sha":"{base_sha}","task_file":"{}","research":false,"surface":null}}"#,
            t - 40,
            d.path().join("tasks/rnull.md").display()
        ),
    );

    let (surface, _) = make_surface(repo.clone(), Vec::new());

    let dec_bb = route(
        &surface,
        &post_json(
            &surface,
            "/api/runs/RBB/decision",
            &serde_json::json!({ "action": "accept" }),
        ),
        t,
    );
    assert_eq!(dec_bb.status, 403);
    assert_eq!(
        parse_json(&dec_bb)["error"],
        "run RBB belongs to another surface"
    );
    assert!(!repo.join(".wt/RBB.verdict.json").exists());
    assert!(!repo.join(".wt/RBB.hold.json").exists());

    let hold_bb = route(
        &surface,
        &post_json(
            &surface,
            "/api/runs/RBB/decision",
            &serde_json::json!({ "action": "hold" }),
        ),
        t,
    );
    assert_eq!(hold_bb.status, 403);
    assert!(!repo.join(".wt/RBB.verdict.json").exists());
    assert!(!repo.join(".wt/RBB.hold.json").exists());

    let diff_bb = route(&surface, &get(&surface, "/api/runs/RBB/diff", vec![]), t);
    assert_eq!(diff_bb.status, 403);
    assert_eq!(
        parse_json(&diff_bb)["error"],
        "run RBB belongs to another surface"
    );

    let dec_null = route(
        &surface,
        &post_json(
            &surface,
            "/api/runs/RNULL/decision",
            &serde_json::json!({ "action": "accept" }),
        ),
        t,
    );
    assert_eq!(dec_null.status, 403);
    assert_eq!(
        parse_json(&dec_null)["error"],
        "run RNULL was not started by a surface"
    );
    assert!(!repo.join(".wt/RNULL.verdict.json").exists());
    assert!(!repo.join(".wt/RNULL.hold.json").exists());

    let diff_null = route(&surface, &get(&surface, "/api/runs/RNULL/diff", vec![]), t);
    assert_eq!(diff_null.status, 403);
    assert_eq!(
        parse_json(&diff_null)["error"],
        "run RNULL was not started by a surface"
    );
}

#[test]
fn reconcile_runs_writes_checks_only_for_owned_runs() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("reconcile-owned-repo");
    let base_sha = init_repo_with_commit(&repo);
    let t = now_secs();

    add_run_worktree(&repo, "RAA");
    add_run_worktree(&repo, "RBB");
    write_finished_run_summary(
        &repo,
        &d.path().join("tasks/raa.md"),
        "RAA",
        &base_sha,
        t - 60,
    );
    write_finished_run_summary(
        &repo,
        &d.path().join("tasks/rbb.md"),
        "RBB",
        &base_sha,
        t - 50,
    );
    write_file(
        &repo.join(".wt/RBB.start.json"),
        &format!(
            r#"{{"id":"RBB","pid":999999,"started_at":{},"base_sha":"{base_sha}","task_file":"{}","research":false,"surface":"s00000000000000bb"}}"#,
            t - 50,
            d.path().join("tasks/rbb.md").display()
        ),
    );

    let (surface, _) = make_surface(repo.clone(), Vec::new());
    blyte_cli::serve::reconcile_runs(&surface, t).unwrap();

    assert!(repo.join(".wt/RAA.checks.json").is_file());
    assert!(!repo.join(".wt/RBB.checks.json").exists());
}

fn assert_no_file_under_contains(dir: &Path, needle: &str) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_no_file_under_contains(&path, needle);
        } else if path.is_file() {
            let bytes = std::fs::read(&path).unwrap_or_default();
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !text.contains(needle),
                "token leaked into {}",
                path.display()
            );
        }
    }
}

struct ChildKillGuard(std::process::Child);

impl Drop for ChildKillGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn surface_keeps_token_out_of_wt_and_rejects_unauthenticated_actions() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("e2e-repo");
    let fake_home = d.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();
    init_git_repo(&repo);

    let child = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["--no-open", "--port", "0"])
        .current_dir(&repo)
        .env("HOME", &fake_home)
        .env("XDG_CONFIG_HOME", fake_home.join(".config"))
        .env_remove("XDG_STATE_HOME")
        .env_remove("BLYTE_STATE_DIR")
        .env("BLYTE_ENV_FILE", "")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_NO_KEEP_AWAKE", "1")
        .env("BLYTE_USAGE_LOG", "off")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = ChildKillGuard(child);

    let stdout = guard.0.stdout.take().unwrap();
    let mut stderr = guard.0.stderr.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let (_name, port, token) = loop {
        line.clear();
        let n = reader.read_line(&mut line).unwrap();
        if n == 0 {
            let mut err_text = String::new();
            let _ = stderr.read_to_string(&mut err_text);
            panic!("blyte exited before printing URL; stderr={err_text}");
        }
        if let Some(parsed) = parse_surface_ready_line(&line) {
            break parsed;
        }
    };

    assert!(!repo.join(".wt/surface.json").exists());
    assert!(!repo.join(".wt/surface.token").exists());
    assert!(!repo.join(".wt/sessions").exists());
    assert_eq!(list_records(&repo).unwrap().len(), 1);
    assert_no_file_under_contains(&repo.join(".wt"), &token);

    let token_dir = fake_home.join(".local/state/blyte/surface");
    let mut state_files: Vec<PathBuf> = std::fs::read_dir(&token_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    state_files.sort();
    assert_eq!(state_files.len(), 2, "{state_files:?}");
    let token_files: Vec<&PathBuf> = state_files
        .iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("token"))
        .collect();
    let message_files: Vec<&PathBuf> = state_files
        .iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("message"))
        .collect();
    assert_eq!(token_files.len(), 1, "{state_files:?}");
    assert_eq!(message_files.len(), 1, "{state_files:?}");
    assert_eq!(
        std::fs::read_to_string(token_files[0]).unwrap().trim(),
        token
    );
    let msg_cred = std::fs::read_to_string(message_files[0])
        .unwrap()
        .trim()
        .to_string();
    assert!(!msg_cred.is_empty());
    assert_ne!(msg_cred, token);
    assert_no_file_under_contains(&repo.join(".wt"), &msg_cred);
    #[cfg(unix)]
    {
        let dir_mode = std::fs::metadata(&token_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "dir mode {:o}", dir_mode);
        for file in &state_files {
            let file_mode = std::fs::metadata(file).unwrap().permissions().mode() & 0o777;
            assert_eq!(file_mode, 0o600, "file mode {:o}", file_mode);
        }
    }

    for path in [
        "/api/requests/x/push",
        "/api/requests/x/pull-request",
        "/api/session/approvals/x",
        "/api/runs/x/decision",
    ] {
        let resp = send_http(
            port,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            ),
        );
        assert!(
            resp.starts_with("HTTP/1.1 403"),
            "POST {path} without token must return 403, got: {resp}"
        );
        assert!(
            resp.contains("missing or invalid surface token"),
            "POST {path} without token must report missing or invalid surface token, got: {resp}"
        );
    }
}

fn test_registry_dir() -> PathBuf {
    ensure_isolated_state_home();
    registry_dir().unwrap()
}

fn read_registry_entry(dir: &Path, id: &str) -> RegistryEntry {
    let raw = std::fs::read_to_string(entry_path(dir, id)).unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn start_stoppable_surface_with_runner(
    repo: &Path,
    args: SurfaceArgs,
    runner: Box<dyn PromptRunner>,
) -> RunningSurface {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let repo_bg = repo.to_path_buf();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let cfg = default_repl_config(repo_bg);
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = |_: &str| {};
        run_with_runner(&args, cfg, &mut out, &mut err, &mut opener, stop_bg, runner)
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let (name, port, token) = loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for printed surface URL; stderr={}",
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        let stdout = String::from_utf8(out_buf.lock().unwrap().clone()).unwrap();
        if let Some(parsed) = stdout.lines().find_map(parse_surface_ready_line) {
            break parsed;
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    RunningSurface {
        name,
        port,
        token,
        stop,
        err_buf,
        handle,
    }
}

struct DoingBlockingRunner {
    started: Arc<(Mutex<bool>, Condvar)>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

impl PromptRunner for DoingBlockingRunner {
    fn run(
        &mut self,
        _prompt: &str,
        _model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        {
            let (lock, cv) = &*self.started;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
        {
            let (lock, cv) = &*self.release;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
        }
        emit(EntryBody::Turn {
            n: 1,
            text: "done".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: None,
        });
        Ok(())
    }
}

fn init_repo_on_branch(dir: &Path, branch: &str) {
    init_git_repo(dir);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    let run_git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run_git(&["config", "user.email", "test@example.com"]);
    run_git(&["config", "user.name", "Test"]);
    run_git(&["checkout", "-b", branch]);
    run_git(&["add", "README.md"]);
    run_git(&["commit", "-m", "initial"]);
}

#[test]
fn started_surface_writes_registry_entry_with_metadata_and_idle() {
    let reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("reg-entry-repo");
    init_repo_on_branch(&repo, "feat/registry-entry");

    let running = start_stoppable_surface_with_args(
        &repo,
        SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            name: None,
            resume: None,
        },
        None,
    );

    let records = list_records(&repo).unwrap();
    assert_eq!(records.len(), 1);
    let rec = &records[0];

    let entry = read_registry_entry(&reg_dir, &rec.id);
    assert_eq!(entry.id, rec.id);
    assert_eq!(entry.name, running.name);
    assert_eq!(entry.port, running.port);
    assert_eq!(
        entry.root.canonicalize().unwrap(),
        repo.canonicalize().unwrap()
    );
    assert_eq!(entry.branch.as_deref(), Some("feat/registry-entry"));
    assert_eq!(entry.doing, "idle");

    running.stop.store(true, Ordering::Relaxed);
    assert_eq!(running.handle.join().unwrap(), 0);
}

#[test]
fn stopped_surface_removes_registry_entry_and_keeps_surface_record() {
    let reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("reg-stop-repo");
    init_git_repo(&repo);

    let running = start_stoppable_surface_with_args(
        &repo,
        SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            name: None,
            resume: None,
        },
        None,
    );

    let records = list_records(&repo).unwrap();
    assert_eq!(records.len(), 1);
    let surface_id = records[0].id.clone();
    assert!(entry_path(&reg_dir, &surface_id).exists());

    running.stop.store(true, Ordering::Relaxed);
    assert_eq!(running.handle.join().unwrap(), 0);

    assert!(!entry_path(&reg_dir, &surface_id).exists());
    let records_after = list_records(&repo).unwrap();
    assert_eq!(records_after.len(), 1);
    assert_eq!(records_after[0].id, surface_id);
}

#[test]
fn second_surface_in_different_repo_with_same_dir_name_gets_dash_2() {
    let _reg_dir = test_registry_dir();
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let repo1 = d1.path().join("reg-shared-dir");
    let repo2 = d2.path().join("reg-shared-dir");
    init_git_repo(&repo1);
    init_git_repo(&repo2);

    let first = start_stoppable_surface_with_args(
        &repo1,
        SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            name: None,
            resume: None,
        },
        None,
    );
    let second = start_stoppable_surface_with_args(
        &repo2,
        SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            name: None,
            resume: None,
        },
        None,
    );

    assert_eq!(first.name, "reg-shared-dir");
    assert_eq!(second.name, "reg-shared-dir-2");

    second.stop.store(true, Ordering::Relaxed);
    first.stop.store(true, Ordering::Relaxed);
    assert_eq!(second.handle.join().unwrap(), 0);
    assert_eq!(first.handle.join().unwrap(), 0);
}

#[test]
fn explicit_name_used_in_different_repo_fails_and_writes_no_registry_entry() {
    let reg_dir = test_registry_dir();
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let repo1 = d1.path().join("reg-name-owner");
    let repo2 = d2.path().join("reg-name-challenger");
    init_git_repo(&repo1);
    init_git_repo(&repo2);

    let first = start_stoppable_surface_with_args(
        &repo1,
        SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            name: Some("reg-cross-repo-name".to_string()),
            resume: None,
        },
        None,
    );

    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut opener = |_: &str| {};
    let stop = Arc::new(AtomicBool::new(false));
    let code = run_with_hooks(
        &SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            name: Some("reg-cross-repo-name".to_string()),
            resume: None,
        },
        default_repl_config(repo2.clone()),
        &mut out,
        &mut err,
        &mut opener,
        stop,
    );

    assert_eq!(code, 1);
    let err_text = String::from_utf8(err).unwrap();
    assert!(
        err_text.contains("a running surface is already named reg-cross-repo-name"),
        "expected existing pick_name duplicate error, got: {err_text}"
    );
    assert!(list_records(&repo2).unwrap().is_empty());
    let first_records = list_records(&repo1).unwrap();
    assert_eq!(first_records.len(), 1);
    let first_id = &first_records[0].id;
    assert!(entry_path(&reg_dir, first_id).exists());
    let repo2_canon = repo2.canonicalize().unwrap();
    let mut stray = Vec::new();
    for item in std::fs::read_dir(&reg_dir).unwrap() {
        let path = item.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => panic!("failed to read {}: {e}", path.display()),
        };
        let entry: RegistryEntry = serde_json::from_str(&raw).unwrap();
        let points_to_repo2 =
            entry.root == repo2 || entry.root.canonicalize().ok().as_ref() == Some(&repo2_canon);
        let duplicate_name = entry.name == "reg-cross-repo-name" && entry.id != *first_id;
        if points_to_repo2 || duplicate_name {
            stray.push(entry);
        }
    }
    assert!(
        stray.is_empty(),
        "expected no registry entry for second repo; stray={stray:?}"
    );

    first.stop.store(true, Ordering::Relaxed);
    assert_eq!(first.handle.join().unwrap(), 0);
}

#[test]
fn stale_registry_entry_does_not_block_name_and_is_removed() {
    let reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("reg-stale-repo");
    init_git_repo(&repo);

    let dead_port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let stale_id = "s00000000000000f1";
    let stale_entry = RegistryEntry {
        id: stale_id.to_string(),
        name: "reg-stale-repo".to_string(),
        root: d.path().join("ghost-repo"),
        branch: Some("main".to_string()),
        pid: 999_999,
        port: dead_port,
        started_at: 1_700_000_000,
        doing: "idle".to_string(),
    };
    write_entry(&reg_dir, &stale_entry).unwrap();
    assert!(entry_path(&reg_dir, stale_id).exists());

    let running = start_stoppable_surface_with_args(
        &repo,
        SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            name: None,
            resume: None,
        },
        None,
    );

    assert_eq!(running.name, "reg-stale-repo");
    assert!(!entry_path(&reg_dir, stale_id).exists());

    running.stop.store(true, Ordering::Relaxed);
    assert_eq!(running.handle.join().unwrap(), 0);
}

#[test]
fn resume_fails_when_name_is_running_in_another_repository() {
    let _reg_dir = test_registry_dir();
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let repo1 = d1.path().join("reg-resume-first");
    let repo2 = d2.path().join("reg-resume-second");
    init_git_repo(&repo1);
    init_git_repo(&repo2);

    let first = start_stoppable_surface_with_args(
        &repo1,
        SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            name: Some("reg-resume-taken".to_string()),
            resume: None,
        },
        None,
    );
    first.stop.store(true, Ordering::Relaxed);
    assert_eq!(first.handle.join().unwrap(), 0);

    let second = start_stoppable_surface_with_args(
        &repo2,
        SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            name: Some("reg-resume-taken".to_string()),
            resume: None,
        },
        None,
    );

    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut opener = |_: &str| {};
    let stop = Arc::new(AtomicBool::new(false));
    let watchdog_done = Arc::new((Mutex::new(false), Condvar::new()));
    let watchdog_stop = Arc::clone(&stop);
    let watchdog_signal = Arc::clone(&watchdog_done);
    let watchdog = std::thread::spawn(move || {
        let (lock, cv) = &*watchdog_signal;
        let guard = lock.lock().unwrap();
        let (guard, _) = cv
            .wait_timeout_while(guard, Duration::from_secs(60), |done| !*done)
            .unwrap();
        if !*guard {
            watchdog_stop.store(true, Ordering::Relaxed);
        }
    });
    let code = run_with_hooks(
        &SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            name: None,
            resume: Some("reg-resume-taken".to_string()),
        },
        default_repl_config(repo1),
        &mut out,
        &mut err,
        &mut opener,
        stop,
    );
    {
        let (lock, cv) = &*watchdog_done;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }
    watchdog.join().unwrap();

    assert_eq!(code, 1);
    let err_text = String::from_utf8(err).unwrap();
    assert!(
        err_text.contains("a running surface is already named reg-resume-taken"),
        "expected running surface name conflict, got: {err_text}"
    );

    second.stop.store(true, Ordering::Relaxed);
    assert_eq!(second.handle.join().unwrap(), 0);
}

#[test]
fn registry_doing_tracks_running_turn_first_line_and_returns_to_idle() {
    let reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("reg-doing-repo");
    init_git_repo(&repo);

    let started = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let runner = DoingBlockingRunner {
        started: Arc::clone(&started),
        release: Arc::clone(&release),
    };

    let running = start_stoppable_surface_with_runner(
        &repo,
        SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            name: None,
            resume: None,
        },
        Box::new(runner),
    );

    let records = list_records(&repo).unwrap();
    assert_eq!(records.len(), 1);
    let surface_id = records[0].id.clone();

    assert_eq!(read_registry_entry(&reg_dir, &surface_id).doing, "idle");

    let body = serde_json::json!({
        "text": "investigate flaky parser\nwith extra details on line two"
    })
    .to_string();
    let post_resp = send_http(
        running.port,
        &format!(
            "POST /api/session/messages HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: http://127.0.0.1:{}\r\n{}: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            running.port,
            running.port,
            TOKEN_HEADER,
            running.token,
            body.len(),
            body
        ),
    );
    assert!(
        post_resp.starts_with("HTTP/1.1 200"),
        "POST /api/session/messages failed: {post_resp}"
    );

    {
        let (lock, cv) = &*started;
        let mut is_started = lock.lock().unwrap();
        while !*is_started {
            is_started = cv.wait(is_started).unwrap();
        }
    }

    assert_eq!(
        read_registry_entry(&reg_dir, &surface_id).doing,
        "investigate flaky parser"
    );

    {
        let (lock, cv) = &*release;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for turn to finish"
        );
        let ev_resp = send_http(
            running.port,
            &format!(
                "GET /api/session/events?after=0&wait_ms=100 HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{}: {}\r\nConnection: close\r\n\r\n",
                running.port, TOKEN_HEADER, running.token
            ),
        );
        if ev_resp.contains("\"kind\":\"idle\"") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(read_registry_entry(&reg_dir, &surface_id).doing, "idle");

    running.stop.store(true, Ordering::Relaxed);
    assert_eq!(running.handle.join().unwrap(), 0);
}

struct PeerRecordingRunner {
    prompts: Arc<Mutex<Vec<String>>>,
}

impl PromptRunner for PeerRecordingRunner {
    fn run(
        &mut self,
        prompt: &str,
        _model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        emit(EntryBody::Turn {
            n: 1,
            text: "ack".to_string(),
            input_tokens: 1,
            output_tokens: 1,
            cost_usd: None,
        });
        Ok(())
    }
}

fn start_recording_surface(
    repo: &Path,
    name: Option<&str>,
) -> (RunningSurface, String, Arc<Mutex<Vec<String>>>) {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let runner = PeerRecordingRunner {
        prompts: Arc::clone(&prompts),
    };
    let running = start_stoppable_surface_with_runner(
        repo,
        SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            name: name.map(String::from),
            resume: None,
        },
        Box::new(runner),
    );
    let records = list_records(repo).unwrap();
    assert_eq!(records.len(), 1);
    let surface_id = records[0].id.clone();
    (running, surface_id, prompts)
}

fn parse_http_status_and_body(raw: &str) -> (u16, String) {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("malformed HTTP response: {raw:?}"));
    let status = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap();
    (status, body.to_string())
}

fn post_peer_raw(port: u16, headers: &[(&str, &str)], body: &str) -> (u16, String) {
    let mut req = format!("POST /api/peer/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str(&format!(
        "Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    let raw = send_http(port, &req);
    parse_http_status_and_body(&raw)
}

fn fetch_events_json(port: u16, token: &str) -> serde_json::Value {
    let raw = send_http(
        port,
        &format!(
            "GET /api/session/events?after=0&wait_ms=0 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{TOKEN_HEADER}: {token}\r\nConnection: close\r\n\r\n"
        ),
    );
    let (status, body) = parse_http_status_and_body(&raw);
    assert_eq!(status, 200, "events fetch failed: {body}");
    serde_json::from_str(&body).unwrap()
}

#[test]
fn the_message_credential_file_is_written_and_removed_on_exit() {
    let _reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("peer-cred-repo");
    init_git_repo(&repo);

    let (running, surface_id, _prompts) = start_recording_surface(&repo, Some("peer-cred-file"));
    let token_path = surface_token_path_for_id(&surface_id).unwrap();
    let cred_path = surface_message_path_for_id(&surface_id).unwrap();
    assert!(cred_path.exists(), "missing {}", cred_path.display());

    #[cfg(unix)]
    {
        let mode = std::fs::metadata(&cred_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    let token_val = std::fs::read_to_string(&token_path).unwrap();
    let cred_val = std::fs::read_to_string(&cred_path).unwrap();
    assert_eq!(token_val.trim(), running.token);
    assert!(!cred_val.trim().is_empty());
    assert_ne!(cred_val.trim(), running.token);

    running.stop.store(true, Ordering::Relaxed);
    assert_eq!(running.handle.join().unwrap(), 0);
    assert!(!cred_path.exists());
    assert!(!token_path.exists());
}

#[test]
fn the_peer_endpoint_takes_only_the_message_credential() {
    let _reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("peer-auth-repo");
    init_git_repo(&repo);

    let (running, surface_id, _prompts) = start_recording_surface(&repo, Some("peer-auth"));
    let cred_path = surface_message_path_for_id(&surface_id).unwrap();
    let cred = std::fs::read_to_string(&cred_path)
        .unwrap()
        .trim()
        .to_string();
    let payload = serde_json::json!({
        "from_name": "sender",
        "from_repository": "shop",
        "text": "hello",
    })
    .to_string();

    let (status, body) = post_peer_raw(running.port, &[(MESSAGE_HEADER, &cred)], &payload);
    assert_eq!(status, 200, "body={body}");
    let val: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(val["status"], "delivered");

    let (status, body) = post_peer_raw(running.port, &[(MESSAGE_HEADER, &running.token)], &payload);
    assert_eq!(status, 403, "body={body}");
    assert!(body.contains("missing or invalid message credential"));

    let (status, body) = post_peer_raw(running.port, &[(TOKEN_HEADER, &running.token)], &payload);
    assert_eq!(status, 403, "body={body}");
    assert!(body.contains("missing or invalid message credential"));

    let (status, body) = post_peer_raw(running.port, &[], &payload);
    assert_eq!(status, 403, "body={body}");
    assert!(body.contains("missing or invalid message credential"));

    let (status, body) = post_peer_raw(
        running.port,
        &[(MESSAGE_HEADER, &cred), (MESSAGE_HEADER, &cred)],
        &payload,
    );
    assert_eq!(status, 403, "body={body}");
    assert!(body.contains("missing or invalid message credential"));

    running.stop.store(true, Ordering::Relaxed);
    assert_eq!(running.handle.join().unwrap(), 0);
}

#[test]
fn the_message_credential_opens_nothing_else() {
    let _reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("peer-isolated-repo");
    init_git_repo(&repo);

    let (running, surface_id, _prompts) = start_recording_surface(&repo, Some("peer-isolated"));
    let cred_path = surface_message_path_for_id(&surface_id).unwrap();
    let cred = std::fs::read_to_string(&cred_path)
        .unwrap()
        .trim()
        .to_string();

    let get_runs = send_http(
        running.port,
        &format!(
            "GET /api/runs HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{TOKEN_HEADER}: {cred}\r\nConnection: close\r\n\r\n",
            running.port
        ),
    );
    let (status, _) = parse_http_status_and_body(&get_runs);
    assert_eq!(status, 403);

    for (path, body) in [
        ("/api/session/messages", r#"{"text":"hi"}"#),
        ("/api/requests/req-1/push", "{}"),
        ("/api/session/approvals/appr-1", r#"{"answer":"yes"}"#),
    ] {
        let raw = send_http(
            running.port,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{TOKEN_HEADER}: {cred}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                running.port,
                body.len()
            ),
        );
        let (status, resp_body) = parse_http_status_and_body(&raw);
        assert_eq!(status, 403, "POST {path} must reject cred: {resp_body}");
    }

    running.stop.store(true, Ordering::Relaxed);
    assert_eq!(running.handle.join().unwrap(), 0);
}

#[test]
fn the_peer_endpoint_rejects_a_bad_body() {
    let _reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("peer-bad-body-repo");
    init_git_repo(&repo);

    let (running, surface_id, prompts) = start_recording_surface(&repo, Some("peer-bad-body"));
    let cred_path = surface_message_path_for_id(&surface_id).unwrap();
    let cred = std::fs::read_to_string(&cred_path)
        .unwrap()
        .trim()
        .to_string();

    let too_long = "a".repeat(16_385);
    let cases = [
        (
            serde_json::json!({
                "from_name": "sender",
                "from_repository": "shop",
            }),
            "text",
        ),
        (
            serde_json::json!({
                "from_name": "sender",
                "from_repository": "shop",
                "text": "   \n\t ",
            }),
            "text",
        ),
        (
            serde_json::json!({
                "from_name": "sender",
                "from_repository": "shop",
                "text": too_long,
            }),
            "text",
        ),
        (
            serde_json::json!({
                "from_name": "../x",
                "from_repository": "shop",
                "text": "hello",
            }),
            "from_name",
        ),
        (
            serde_json::json!({
                "from_name": "sender",
                "from_repository": "shop\nevil",
                "text": "hello",
            }),
            "from_repository",
        ),
    ];

    for (payload, field) in cases {
        let (status, body) = post_peer_raw(
            running.port,
            &[(MESSAGE_HEADER, &cred)],
            &payload.to_string(),
        );
        assert_eq!(status, 400, "field={field}, body={body}");
        assert!(
            body.contains(field),
            "expected error naming {field}, got: {body}"
        );
    }

    let events = fetch_events_json(running.port, &running.token);
    assert!(events["entries"].as_array().unwrap().is_empty());
    assert!(prompts.lock().unwrap().is_empty());

    running.stop.store(true, Ordering::Relaxed);
    assert_eq!(running.handle.join().unwrap(), 0);
}

#[test]
fn a_peer_message_reaches_the_runner_marked() {
    let _reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("peer-marked-repo");
    init_git_repo(&repo);

    let (running, surface_id, prompts) = start_recording_surface(&repo, Some("peer-marked"));
    let cred_path = surface_message_path_for_id(&surface_id).unwrap();
    let cred = std::fs::read_to_string(&cred_path)
        .unwrap()
        .trim()
        .to_string();

    let payload = serde_json::json!({
        "from_name": "api",
        "from_repository": "shop",
        "text": "schema changed",
    })
    .to_string();
    let (status, body) = post_peer_raw(running.port, &[(MESSAGE_HEADER, &cred)], &payload);
    assert_eq!(status, 200, "body={body}");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for runner");
        if !prompts.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        prompts.lock().unwrap()[0],
        peer_prompt("api", "shop", "schema changed")
    );

    let events = fetch_events_json(running.port, &running.token);
    let entries = events["entries"].as_array().unwrap();
    let peer_entry = entries
        .iter()
        .find(|e| e["kind"] == "peer_message")
        .expect("expected peer_message entry");
    assert_eq!(peer_entry["from_name"], "api");
    assert_eq!(peer_entry["from_repository"], "shop");
    assert_eq!(peer_entry["text"], "schema changed");

    running.stop.store(true, Ordering::Relaxed);
    assert_eq!(running.handle.join().unwrap(), 0);
}

#[test]
fn the_sixth_peer_message_is_answered_held() {
    let _reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("peer-held-repo");
    init_git_repo(&repo);

    let (running, surface_id, _prompts) = start_recording_surface(&repo, Some("peer-held"));
    let cred_path = surface_message_path_for_id(&surface_id).unwrap();
    let cred = std::fs::read_to_string(&cred_path)
        .unwrap()
        .trim()
        .to_string();

    for i in 1..=5 {
        let payload = serde_json::json!({
            "from_name": "api",
            "from_repository": "shop",
            "text": format!("msg {i}"),
        })
        .to_string();
        let (status, body) = post_peer_raw(running.port, &[(MESSAGE_HEADER, &cred)], &payload);
        assert_eq!(status, 200, "i={i} body={body}");
        let val: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(val["status"], "delivered", "i={i}");
    }

    let payload = serde_json::json!({
        "from_name": "api",
        "from_repository": "shop",
        "text": "msg 6",
    })
    .to_string();
    let (status, body) = post_peer_raw(running.port, &[(MESSAGE_HEADER, &cred)], &payload);
    assert_eq!(status, 200, "body={body}");
    let val: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(val["status"], "held");

    running.stop.store(true, Ordering::Relaxed);
    assert_eq!(running.handle.join().unwrap(), 0);
}

#[test]
fn health_does_not_show_the_message_credential() {
    let _reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("peer-health-repo");
    init_git_repo(&repo);

    let (running, surface_id, _prompts) = start_recording_surface(&repo, Some("peer-health"));
    let cred_path = surface_message_path_for_id(&surface_id).unwrap();
    let cred = std::fs::read_to_string(&cred_path)
        .unwrap()
        .trim()
        .to_string();

    let raw = send_http(
        running.port,
        &format!(
            "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            running.port
        ),
    );
    let (status, body) = parse_http_status_and_body(&raw);
    assert_eq!(status, 200);
    assert!(!body.contains(&cred));
    assert!(!body.contains(&running.token));

    running.stop.store(true, Ordering::Relaxed);
    assert_eq!(running.handle.join().unwrap(), 0);
}
