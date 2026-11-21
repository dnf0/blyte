use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use blyte_cli::cli::ReviewVerdict;
use blyte_cli::combine::{CombineRecord, CombineState};
use blyte_cli::http::{Request, Response};
use blyte_cli::queue_actions::{record_verdict, set_hold};
use blyte_cli::requests::{
    maybe_combine, request_readiness, Publisher, Readiness, RealPublisher, RequestsState,
};
use blyte_cli::serve::{generate_token, route, Surface, TOKEN_HEADER};
use blyte_cli::session::{EntryBody, PromptRunner, Session};
use serde_json::{json, Value};

struct RecordingRunner {
    prompts: Arc<Mutex<Vec<String>>>,
}

impl RecordingRunner {
    fn new() -> (Self, Arc<Mutex<Vec<String>>>) {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                prompts: Arc::clone(&prompts),
            },
            prompts,
        )
    }
}

impl PromptRunner for RecordingRunner {
    fn run(
        &mut self,
        prompt: &str,
        _model: &str,
        _emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PushCall {
    branch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PrCall {
    branch: String,
    title: String,
    body: String,
}

struct FakePublisher {
    push_calls: Mutex<Vec<PushCall>>,
    pr_calls: Mutex<Vec<PrCall>>,
    push_result: Mutex<Result<(), String>>,
    pr_result: Mutex<Result<String, String>>,
}

impl FakePublisher {
    fn ok(pr_url: &str) -> Arc<Self> {
        Arc::new(Self {
            push_calls: Mutex::new(Vec::new()),
            pr_calls: Mutex::new(Vec::new()),
            push_result: Mutex::new(Ok(())),
            pr_result: Mutex::new(Ok(pr_url.to_string())),
        })
    }

    fn set_push_error(&self, msg: &str) {
        *self.push_result.lock().unwrap() = Err(msg.to_string());
    }

    fn set_pr_error(&self, msg: &str) {
        *self.pr_result.lock().unwrap() = Err(msg.to_string());
    }
}

impl Publisher for FakePublisher {
    fn push_branch(&self, _root: &Path, branch: &str) -> Result<(), String> {
        self.push_calls.lock().unwrap().push(PushCall {
            branch: branch.to_string(),
        });
        self.push_result.lock().unwrap().clone()
    }

    fn open_pull_request(
        &self,
        _root: &Path,
        branch: &str,
        title: &str,
        body: &str,
    ) -> Result<String, String> {
        self.pr_calls.lock().unwrap().push(PrCall {
            branch: branch.to_string(),
            title: title.to_string(),
            body: body.to_string(),
        });
        self.pr_result.lock().unwrap().clone()
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {e}", args));
    assert!(
        out.status.success(),
        "git {:?} failed:\nstdout: {}\nstderr: {}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo_with_gate(gate: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "Test User"]);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    std::fs::write(
        root.join("blyte.toml"),
        format!("[dispatch]\ngate = {gate:?}\n"),
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial commit"]);
    let base_sha = git(root, &["rev-parse", "HEAD"]);
    std::fs::create_dir_all(root.join(".wt")).unwrap();
    (dir, base_sha)
}

fn create_run_branch(
    root: &Path,
    id: &str,
    base_sha: &str,
    file: &str,
    content: &str,
    subject: &str,
) -> String {
    let wt = root.join(".wt").join(id);
    let branch = format!("wip/{id}");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            wt.to_str().unwrap(),
            base_sha,
        ],
    );
    let target = wt.join(file);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&target, content).unwrap();
    git(&wt, &["add", "."]);
    git(&wt, &["commit", "-m", subject]);
    git(&wt, &["rev-parse", "HEAD"])
}

fn write_start_file(
    root: &Path,
    id: &str,
    request: Option<&str>,
    base_sha: &str,
    started_at: u64,
    research: bool,
) {
    let mut obj = json!({
        "id": id,
        "base": "main",
        "base_sha": base_sha,
        "started_at": started_at,
        "research": research,
    });
    if let Some(req) = request {
        obj["request"] = Value::String(req.to_string());
    }
    std::fs::write(
        root.join(".wt").join(format!("{id}.start.json")),
        serde_json::to_string_pretty(&obj).unwrap() + "\n",
    )
    .unwrap();
}

fn write_summary(root: &Path, id: &str, outcome: &str, branch: Option<&str>) {
    let mut obj = json!({
        "id": id,
        "outcome": outcome,
        "seconds": 5,
    });
    if let Some(b) = branch {
        obj["branch"] = Value::String(b.to_string());
    }
    std::fs::write(
        root.join(".wt").join(format!("{id}.json")),
        serde_json::to_string_pretty(&obj).unwrap() + "\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".wt").join(format!("{id}.report.json")),
        "{\"turns\":1}\n",
    )
    .unwrap();
}

fn write_checks(root: &Path, id: &str, flags: &[&str]) {
    let obj = json!({
        "flags": flags,
        "fallbacks": [],
    });
    std::fs::write(
        root.join(".wt").join(format!("{id}.checks.json")),
        serde_json::to_string_pretty(&obj).unwrap() + "\n",
    )
    .unwrap();
}

fn make_surface(
    root: PathBuf,
    publisher: Arc<dyn Publisher>,
) -> (Surface, Arc<Mutex<Vec<String>>>) {
    let (runner, prompts) = RecordingRunner::new();
    let session = Session::start(Box::new(runner), "gemini-4-neon".to_string());
    let repo = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string();
    let surface = Surface {
        root,
        repo,
        session,
        session_id: "sess-req-test".to_string(),
        started_at: 1_700_000_000,
        days: 14,
        token: generate_token().unwrap(),
        requests: Arc::new(RequestsState::new()),
        publisher,
    };
    (surface, prompts)
}

fn get(path: &str) -> Request {
    Request {
        method: "GET".to_string(),
        path: path.to_string(),
        query: Vec::new(),
        headers: Vec::new(),
        body: Vec::new(),
    }
}

fn post_json_with_token(path: &str, token: Option<&str>, body: &Value) -> Request {
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    if let Some(t) = token {
        headers.push((TOKEN_HEADER.to_string(), t.to_string()));
    }
    Request {
        method: "POST".to_string(),
        path: path.to_string(),
        query: Vec::new(),
        headers,
        body: serde_json::to_vec(body).unwrap(),
    }
}

fn parse_json(resp: &Response) -> Value {
    serde_json::from_slice(&resp.body).unwrap_or_else(|e| {
        panic!(
            "invalid JSON (status {}): {e}: {}",
            resp.status,
            String::from_utf8_lossy(&resp.body)
        )
    })
}

fn wait_for_idle_entries(session: &Session, expected_count: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let entries = session.entries_after(0, Duration::from_millis(50));
        let count = entries
            .iter()
            .filter(|e| matches!(e.body, EntryBody::Idle { .. }))
            .count();
        if count >= expected_count {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected_count} idle entries (saw {count})"
        );
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn readiness_cases() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();

    write_start_file(root, "T1", Some("req-ready"), &base, 100, false);
    write_summary(root, "T1", "verified", Some("wip/T1"));
    write_checks(root, "T1", &[]);
    write_start_file(root, "T2", Some("req-ready"), &base, 101, false);
    write_summary(root, "T2", "verified", Some("wip/T2"));
    write_checks(root, "T2", &[]);
    write_start_file(root, "R1", Some("req-ready"), &base, 102, true);
    assert_eq!(
        request_readiness(root, "req-ready").unwrap(),
        Readiness::Ready
    );

    write_start_file(root, "T3", Some("req-flagged"), &base, 103, false);
    write_summary(root, "T3", "verified", Some("wip/T3"));
    write_checks(root, "T3", &["diff touched risky file"]);
    let flagged = request_readiness(root, "req-flagged").unwrap();
    match flagged {
        Readiness::Waiting { reasons } => {
            assert!(reasons.iter().any(|r| r.contains("flagged")), "{reasons:?}");
        }
        Readiness::Ready => panic!("expected waiting for flagged run"),
    }

    record_verdict(root, "T3", ReviewVerdict::Accepted, "ok", 200).unwrap();
    assert_eq!(
        request_readiness(root, "req-flagged").unwrap(),
        Readiness::Ready
    );

    write_start_file(root, "T4", Some("req-hold"), &base, 104, false);
    write_summary(root, "T4", "verified", Some("wip/T4"));
    write_checks(root, "T4", &[]);
    set_hold(root, "T4", "wait for signoff", 200).unwrap();
    match request_readiness(root, "req-hold").unwrap() {
        Readiness::Waiting { reasons } => {
            assert!(reasons.iter().any(|r| r.contains("hold")), "{reasons:?}");
        }
        Readiness::Ready => panic!("expected waiting for held run"),
    }

    write_start_file(root, "T5", Some("req-gate"), &base, 105, false);
    write_summary(root, "T5", "gate_failed", Some("wip/T5"));
    write_checks(root, "T5", &[]);
    match request_readiness(root, "req-gate").unwrap() {
        Readiness::Waiting { reasons } => {
            assert!(
                reasons.iter().any(|r| r.contains("gate_failed")),
                "{reasons:?}"
            );
        }
        Readiness::Ready => panic!("expected waiting for gate_failed run"),
    }

    write_start_file(root, "T6", Some("req-nochecks"), &base, 106, false);
    write_summary(root, "T6", "verified", Some("wip/T6"));
    match request_readiness(root, "req-nochecks").unwrap() {
        Readiness::Waiting { reasons } => {
            assert!(
                reasons.iter().any(|r| r.contains("no checks file")),
                "{reasons:?}"
            );
        }
        Readiness::Ready => panic!("expected waiting for run without checks file"),
    }

    write_start_file(root, "T7", Some("req-broken"), &base, 107, false);
    std::fs::write(root.join(".wt/T7.json"), "{not json").unwrap();
    let err = request_readiness(root, "req-broken").unwrap_err();
    assert!(err.contains("T7.json"), "{err}");

    write_start_file(root, "T8", Some("req-redone"), &base, 108, false);
    write_summary(root, "T8", "verified", Some("wip/T8"));
    write_checks(root, "T8", &[]);
    record_verdict(root, "T8", ReviewVerdict::Redone, "redo", 200).unwrap();
    match request_readiness(root, "req-redone").unwrap() {
        Readiness::Waiting { reasons } => {
            assert!(reasons.iter().any(|r| r.contains("redone")), "{reasons:?}");
        }
        Readiness::Ready => panic!("expected waiting for redone run"),
    }
}

#[test]
fn trigger_combines_ready_and_skips_waiting_or_unrequested() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "A1", &base, "a.txt", "a\n", "feat: add a");
    write_start_file(root, "A1", Some("req-a"), &base, now, false);
    write_summary(root, "A1", "verified", Some("wip/A1"));
    write_checks(root, "A1", &[]);

    create_run_branch(root, "B1", &base, "b.txt", "b\n", "feat: add b");
    write_start_file(root, "B1", Some("req-b"), &base, now, false);
    write_summary(root, "B1", "verified", Some("wip/B1"));
    write_checks(root, "B1", &["needs review"]);

    create_run_branch(root, "C1", &base, "c.txt", "c\n", "feat: add c");
    write_start_file(root, "C1", None, &base, now, false);
    write_summary(root, "C1", "verified", Some("wip/C1"));
    write_checks(root, "C1", &[]);

    create_run_branch(root, "D1", &base, "d.txt", "d\n", "feat: add d");
    write_start_file(root, "D1", Some("req-poll"), &base, now + 5, false);
    write_summary(root, "D1", "verified", Some("wip/D1"));
    write_checks(root, "D1", &[]);

    let (surface, prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));

    assert!(maybe_combine(&surface, "req-a").unwrap());
    surface.requests.wait_for_idle("req-a");

    let rec_path = root.join(".wt/combined/req-a.json");
    let rec: CombineRecord =
        serde_json::from_str(&std::fs::read_to_string(&rec_path).unwrap()).unwrap();
    assert_eq!(rec.state, CombineState::Combined);
    assert_eq!(rec.branch, "blyte/req-a");
    let tip = git(root, &["rev-parse", "--verify", "refs/heads/blyte/req-a"]);
    assert!(!tip.is_empty());

    assert!(!maybe_combine(&surface, "req-a").unwrap());
    assert!(!maybe_combine(&surface, "req-b").unwrap());
    assert!(!root.join(".wt/combined/req-b.json").exists());

    let runs_resp = route(&surface, &get("/api/runs"), now + 6);
    assert_eq!(runs_resp.status, 200);
    surface.requests.wait_all_idle();
    assert!(!root.join(".wt/combined/req-b.json").exists());
    assert!(root.join(".wt/combined/req-poll.json").exists());
    let poll_rec: CombineRecord = serde_json::from_str(
        &std::fs::read_to_string(root.join(".wt/combined/req-poll.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(poll_rec.state, CombineState::Combined);

    let reqs = parse_json(&route(&surface, &get("/api/requests"), now + 7));
    let ids: Vec<&str> = reqs
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["request"].as_str().unwrap())
        .collect();
    assert_eq!(ids[0], "req-poll");

    std::thread::sleep(Duration::from_millis(100));
    assert!(prompts.lock().unwrap().is_empty());
}

#[test]
fn conflict_queues_once_and_success_queues_none() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();

    create_run_branch(root, "X1", &base, "shared.txt", "from x1\n", "feat: x1");
    write_start_file(root, "X1", Some("req-conflict"), &base, 100, false);
    write_summary(root, "X1", "verified", Some("wip/X1"));
    write_checks(root, "X1", &[]);

    create_run_branch(root, "X2", &base, "shared.txt", "from x2\n", "feat: x2");
    write_start_file(root, "X2", Some("req-conflict"), &base, 101, false);
    write_summary(root, "X2", "verified", Some("wip/X2"));
    write_checks(root, "X2", &[]);

    let (surface, prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));

    assert!(maybe_combine(&surface, "req-conflict").unwrap());
    surface.requests.wait_for_idle("req-conflict");
    wait_for_idle_entries(&surface.session, 1);

    {
        let recorded = prompts.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].contains("req-conflict"), "{}", recorded[0]);
        assert!(recorded[0].contains("conflict"), "{}", recorded[0]);
        assert!(recorded[0].contains("X2"), "{}", recorded[0]);
        assert!(recorded[0].contains("shared.txt"), "{}", recorded[0]);
    }

    let triggered_again = maybe_combine(&surface, "req-conflict").unwrap();
    if triggered_again {
        surface.requests.wait_for_idle("req-conflict");
    }
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(prompts.lock().unwrap().len(), 1);

    create_run_branch(root, "OK1", &base, "ok.txt", "ok\n", "feat: ok");
    write_start_file(root, "OK1", Some("req-ok"), &base, 102, false);
    write_summary(root, "OK1", "verified", Some("wip/OK1"));
    write_checks(root, "OK1", &[]);

    assert!(maybe_combine(&surface, "req-ok").unwrap());
    surface.requests.wait_for_idle("req-ok");
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(prompts.lock().unwrap().len(), 1);
}

#[test]
fn accept_triggers_combine_and_does_not_queue_combine_message() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "F1", &base, "feature.txt", "hello\n", "feat: flagged");
    write_start_file(root, "F1", Some("req-accept"), &base, now, false);
    write_summary(root, "F1", "verified", Some("wip/F1"));
    write_checks(root, "F1", &["touched sensitive path"]);

    let (surface, prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));

    let resp = route(
        &surface,
        &post_json_with_token(
            "/api/runs/F1/decision",
            Some(&surface.token),
            &json!({ "action": "accept", "note": "looks good" }),
        ),
        now,
    );
    assert_eq!(resp.status, 200);
    let body = parse_json(&resp);
    assert!(body["decision"]["orchestrator_message"].is_null());

    surface.requests.wait_for_idle("req-accept");

    let rec_path = root.join(".wt/combined/req-accept.json");
    let rec: CombineRecord =
        serde_json::from_str(&std::fs::read_to_string(&rec_path).unwrap()).unwrap();
    assert_eq!(rec.state, CombineState::Combined);
    assert_eq!(rec.branch, "blyte/req-accept");

    std::thread::sleep(Duration::from_millis(150));
    assert!(prompts.lock().unwrap().is_empty());
}

#[test]
fn api_requests_shows_waiting_then_combined_with_suggested_pr_fields() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "R1", &base, "one.txt", "1\n", "feat: first change");
    write_start_file(root, "R1", Some("req-multi"), &base, now, false);
    write_summary(root, "R1", "verified", Some("wip/R1"));
    write_checks(root, "R1", &[]);

    create_run_branch(root, "R2", &base, "two.txt", "2\n", "feat: second change");
    write_start_file(root, "R2", Some("req-multi"), &base, now + 1, false);
    write_summary(root, "R2", "verified", Some("wip/R2"));
    write_checks(root, "R2", &["model review flagged"]);

    let (surface, _prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));

    let resp1 = route(&surface, &get("/api/requests"), now);
    assert_eq!(resp1.status, 200);
    let arr1 = parse_json(&resp1);
    let items1 = arr1.as_array().unwrap();
    assert_eq!(items1.len(), 1);
    assert_eq!(items1[0]["request"], "req-multi");
    assert_eq!(items1[0]["state"], "waiting");
    assert_eq!(items1[0]["runs"], json!(["R1", "R2"]));
    assert!(
        items1[0]["waiting_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("flagged")),
        "{}",
        items1[0]
    );
    assert_eq!(items1[0]["pushed"], false);
    assert!(items1[0]["pull_request_url"].is_null());

    record_verdict(root, "R2", ReviewVerdict::Accepted, "ok", now + 2).unwrap();
    assert!(maybe_combine(&surface, "req-multi").unwrap());
    surface.requests.wait_for_idle("req-multi");

    let resp2 = route(&surface, &get("/api/requests"), now + 3);
    assert_eq!(resp2.status, 200);
    let arr2 = parse_json(&resp2);
    let item = &arr2.as_array().unwrap()[0];
    assert_eq!(item["request"], "req-multi");
    assert_eq!(item["state"], "combined");
    assert_eq!(item["branch"], "blyte/req-multi");
    assert_eq!(item["base"], base);
    let commits = item["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0]["subject"], "feat: first change");
    assert_eq!(commits[1]["subject"], "feat: second change");
    assert!(!commits[0]["sha"].as_str().unwrap().is_empty());
    assert_eq!(item["suggested_title"], "feat: first change");
    assert_eq!(
        item["suggested_body"],
        "feat: first change\nfeat: second change"
    );
    assert_eq!(item["pushed"], false);
    assert!(item["pull_request_url"].is_null());
}

#[test]
fn push_and_pull_request_routes_and_error_handling() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "P1", &base, "p.txt", "p\n", "feat: publishable");
    write_start_file(root, "P1", Some("req-pub"), &base, now, false);
    write_summary(root, "P1", "verified", Some("wip/P1"));
    write_checks(root, "P1", &[]);

    let fake = FakePublisher::ok("https://github.com/example/repo/pull/42");
    let (surface, _prompts) = make_surface(root.to_path_buf(), fake.clone());

    let no_secret_push = route(
        &surface,
        &post_json_with_token("/api/requests/req-pub/push", None, &json!({})),
        now,
    );
    assert_eq!(no_secret_push.status, 403);

    let no_secret_pr = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-pub/pull-request",
            None,
            &json!({ "title": "t", "body": "b" }),
        ),
        now,
    );
    assert_eq!(no_secret_pr.status, 403);

    let invalid_id_push = route(
        &surface,
        &post_json_with_token(
            "/api/requests/bad..id/push",
            Some(&surface.token),
            &json!({}),
        ),
        now,
    );
    assert_eq!(invalid_id_push.status, 400);

    let invalid_id_pr = route(
        &surface,
        &post_json_with_token(
            "/api/requests/bad..id/pull-request",
            Some(&surface.token),
            &json!({ "title": "t", "body": "b" }),
        ),
        now,
    );
    assert_eq!(invalid_id_pr.status, 400);

    let uncombined_push = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-pub/push",
            Some(&surface.token),
            &json!({}),
        ),
        now,
    );
    assert_eq!(uncombined_push.status, 409);

    let uncombined_pr = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-pub/pull-request",
            Some(&surface.token),
            &json!({ "title": "My PR", "body": "Details" }),
        ),
        now,
    );
    assert_eq!(uncombined_pr.status, 409);

    assert!(maybe_combine(&surface, "req-pub").unwrap());
    surface.requests.wait_for_idle("req-pub");

    let empty_title_pr = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-pub/pull-request",
            Some(&surface.token),
            &json!({ "title": "   ", "body": "Details" }),
        ),
        now,
    );
    assert_eq!(empty_title_pr.status, 400);

    let unpushed_pr = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-pub/pull-request",
            Some(&surface.token),
            &json!({ "title": "My PR", "body": "Details" }),
        ),
        now,
    );
    assert_eq!(unpushed_pr.status, 409);

    let push_ok = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-pub/push",
            Some(&surface.token),
            &json!({}),
        ),
        now,
    );
    assert_eq!(push_ok.status, 200);
    assert_eq!(parse_json(&push_ok), json!({ "pushed": true }));
    assert_eq!(
        *fake.push_calls.lock().unwrap(),
        vec![PushCall {
            branch: "blyte/req-pub".to_string(),
        }]
    );

    let pr_ok = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-pub/pull-request",
            Some(&surface.token),
            &json!({ "title": "Publishable PR", "body": "Line 1\nLine 2" }),
        ),
        now,
    );
    assert_eq!(pr_ok.status, 200);
    assert_eq!(
        parse_json(&pr_ok),
        json!({ "url": "https://github.com/example/repo/pull/42" })
    );
    assert_eq!(
        *fake.pr_calls.lock().unwrap(),
        vec![PrCall {
            branch: "blyte/req-pub".to_string(),
            title: "Publishable PR".to_string(),
            body: "Line 1\nLine 2".to_string(),
        }]
    );

    let reqs = parse_json(&route(&surface, &get("/api/requests"), now));
    let item = &reqs.as_array().unwrap()[0];
    assert_eq!(item["pushed"], true);
    assert_eq!(
        item["pull_request_url"],
        "https://github.com/example/repo/pull/42"
    );

    let (restarted_surface, _) = make_surface(root.to_path_buf(), fake.clone());
    let restarted_reqs = parse_json(&route(&restarted_surface, &get("/api/requests"), now));
    let restarted_item = &restarted_reqs.as_array().unwrap()[0];
    assert_eq!(restarted_item["state"], "combined");
    assert_eq!(restarted_item["pushed"], true);
    assert_eq!(
        restarted_item["pull_request_url"],
        "https://github.com/example/repo/pull/42"
    );

    fake.set_push_error("remote rejected push");
    let push_err = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-pub/push",
            Some(&surface.token),
            &json!({}),
        ),
        now,
    );
    assert_eq!(push_err.status, 500);
    assert!(parse_json(&push_err)["error"]
        .as_str()
        .unwrap()
        .contains("remote rejected push"));

    fake.set_pr_error("gh is not signed in");
    let pr_err = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-pub/pull-request",
            Some(&surface.token),
            &json!({ "title": "Another", "body": "" }),
        ),
        now,
    );
    assert_eq!(pr_err.status, 500);
    assert!(parse_json(&pr_err)["error"]
        .as_str()
        .unwrap()
        .contains("gh is not signed in"));

    let entries = surface.session.entries_after(0, Duration::from_millis(50));
    assert!(entries.iter().any(|e| matches!(
        &e.body,
        EntryBody::Decision { run, action, .. } if run == "req-pub" && action == "push"
    )));
    assert!(entries.iter().any(|e| matches!(
        &e.body,
        EntryBody::Decision { run, action, note }
            if run == "req-pub"
                && action == "pull_request"
                && note.as_deref() == Some("https://github.com/example/repo/pull/42")
    )));
    assert!(entries.iter().any(|e| matches!(
        &e.body,
        EntryBody::Error { message } if message.contains("remote rejected push")
    )));
    assert!(entries.iter().any(|e| matches!(
        &e.body,
        EntryBody::Error { message } if message.contains("gh is not signed in")
    )));
}

#[test]
fn check_failed_queues_orchestrator_message_with_step_and_output() {
    let (repo, base) = init_repo_with_gate("echo broken-gate-output && false");
    let root = repo.path();

    create_run_branch(root, "G1", &base, "g.txt", "g\n", "feat: g");
    write_start_file(root, "G1", Some("req-checkfail"), &base, 100, false);
    write_summary(root, "G1", "verified", Some("wip/G1"));
    write_checks(root, "G1", &[]);

    let (surface, prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));

    assert!(maybe_combine(&surface, "req-checkfail").unwrap());
    surface.requests.wait_for_idle("req-checkfail");
    wait_for_idle_entries(&surface.session, 1);

    let recorded = prompts.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains("req-checkfail"), "{}", recorded[0]);
    assert!(recorded[0].contains("check_failed"), "{}", recorded[0]);
    assert!(
        recorded[0].contains("broken-gate-output"),
        "{}",
        recorded[0]
    );

    let reqs = parse_json(&route(&surface, &get("/api/requests"), now_secs()));
    let item = &reqs.as_array().unwrap()[0];
    assert_eq!(item["state"], "check_failed");
    assert!(item["failing_step"].as_str().is_some());
    assert!(item["output_tail"]
        .as_str()
        .unwrap()
        .contains("broken-gate-output"));
}

#[test]
fn combine_exit_two_reports_error_once_and_surfaces_in_api() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    std::fs::remove_file(root.join("blyte.toml")).unwrap();

    create_run_branch(root, "E1", &base, "e.txt", "e\n", "feat: e");
    write_start_file(root, "E1", Some("req-err"), &base, 100, false);
    write_summary(root, "E1", "verified", Some("wip/E1"));
    write_checks(root, "E1", &[]);

    let (surface, prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));

    assert!(maybe_combine(&surface, "req-err").unwrap());
    surface.requests.wait_for_idle("req-err");
    wait_for_idle_entries(&surface.session, 1);

    {
        let recorded = prompts.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].contains("req-err"), "{}", recorded[0]);
        assert!(recorded[0].contains("error"), "{}", recorded[0]);
        assert!(recorded[0].contains("blyte.toml"), "{}", recorded[0]);
    }

    let reqs = parse_json(&route(&surface, &get("/api/requests"), now_secs()));
    let item = &reqs.as_array().unwrap()[0];
    assert_eq!(item["state"], "error");
    assert!(item["error"].as_str().unwrap().contains("blyte.toml"));

    if maybe_combine(&surface, "req-err").unwrap() {
        surface.requests.wait_for_idle("req-err");
    }
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(prompts.lock().unwrap().len(), 1);
}
