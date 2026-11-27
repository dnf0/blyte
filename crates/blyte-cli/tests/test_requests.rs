use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use blyte_cli::cli::{ReviewVerdict, SurfaceArgs};
use blyte_cli::combine::{CombineCheck, CombineRecord, CombineState, CombinedRun};
use blyte_cli::http::{Request, Response};
use blyte_cli::queue_actions::{decide, record_verdict, set_hold, Action};
use blyte_cli::repl::ReplConfig;
use blyte_cli::requests::{
    list_requests, load_attempt_record, load_published_record, maybe_combine,
    open_request_pull_request, push_request, request_readiness, write_attempt_record,
    AttemptRecord, AttemptResult, Publisher, Readiness, RealPublisher, RequestActionError,
    RequestsState,
};
use blyte_cli::serve::{generate_token, route, run_with_hooks, Surface, TOKEN_HEADER};
use blyte_cli::session::{EntryBody, PromptRunner, Session};
use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(30);

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

fn fast_import_commit(
    root: &Path,
    branch: &str,
    from: Option<&str>,
    files: &[(&str, &str)],
    subject: &str,
) -> String {
    use std::io::Write;
    let mut stream = String::new();
    stream.push_str(&format!("commit refs/heads/{branch}\n"));
    stream.push_str("mark :1\n");
    stream.push_str("committer Test User <test@example.com> 1700000000 +0000\n");
    stream.push_str(&format!("data {}\n{subject}\n", subject.len()));
    if let Some(parent) = from {
        stream.push_str(&format!("from {parent}\n"));
    }
    for (file, content) in files {
        stream.push_str(&format!("M 100644 inline {file}\n"));
        stream.push_str(&format!("data {}\n{content}\n", content.len()));
    }
    stream.push_str("get-mark :1\n");
    stream.push_str("done\n");
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git fast-import failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn copy_dir_all(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let ty = entry.file_type().unwrap();
        if ty.is_dir() {
            copy_dir_all(&src_path, &dst_path);
        } else {
            std::fs::copy(&src_path, &dst_path).unwrap();
        }
    }
}

fn template_repo() -> &'static (tempfile::TempDir, String) {
    static TEMPLATE: OnceLock<(tempfile::TempDir, String)> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        let cfg_path = root.join(".git/config");
        let mut cfg = std::fs::read_to_string(&cfg_path).unwrap();
        cfg.push_str("\n[user]\n\temail = test@example.com\n\tname = Test User\n");
        std::fs::write(&cfg_path, cfg).unwrap();
        let toml = "[dispatch]\ngate = \"true\"\n";
        let base_sha = fast_import_commit(
            root,
            "main",
            None,
            &[("README.md", "hello\n"), ("blyte.toml", toml)],
            "initial commit",
        );
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        std::fs::write(root.join("blyte.toml"), toml).unwrap();
        std::fs::create_dir_all(root.join(".wt")).unwrap();
        (dir, base_sha)
    })
}

fn init_repo_with_gate(gate: &str) -> (tempfile::TempDir, String) {
    let (tmpl, base_sha) = template_repo();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    copy_dir_all(tmpl.path(), root);
    if gate != "true" {
        let toml = format!("[dispatch]\ngate = {gate:?}\n");
        std::fs::write(root.join("blyte.toml"), &toml).unwrap();
    }
    (dir, base_sha.clone())
}

fn create_run_branches(root: &Path, specs: &[(&str, &str, &str, &str, &str)]) -> Vec<String> {
    use std::io::Write;
    let mut stream = String::new();
    for (idx, (id, base_sha, file, content, subject)) in specs.iter().enumerate() {
        let mark = idx + 1;
        stream.push_str(&format!("commit refs/heads/wip/{id}\n"));
        stream.push_str(&format!("mark :{mark}\n"));
        stream.push_str("committer Test User <test@example.com> 1700000000 +0000\n");
        stream.push_str(&format!("data {}\n{subject}\n", subject.len()));
        stream.push_str(&format!("from {base_sha}\n"));
        stream.push_str(&format!("M 100644 inline {file}\n"));
        stream.push_str(&format!("data {}\n{content}\n", content.len()));
        stream.push_str(&format!("get-mark :{mark}\n"));
    }
    stream.push_str("done\n");
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git fast-import failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn create_run_branch(
    root: &Path,
    id: &str,
    base_sha: &str,
    file: &str,
    content: &str,
    subject: &str,
) -> String {
    fast_import_commit(
        root,
        &format!("wip/{id}"),
        Some(base_sha),
        &[(file, content)],
        subject,
    )
}

fn assert_session_idle(session: &Session) {
    let info = session.info();
    assert!(!info.busy, "expected idle session, got busy");
    assert_eq!(info.queued, 0, "expected empty queue, got {}", info.queued);
}

fn write_start_file_with_base(
    root: &Path,
    id: &str,
    request: Option<&str>,
    base_ref: &str,
    base_sha: &str,
    started_at: u64,
    research: bool,
) {
    write_start_file_with_surface(
        root,
        id,
        request,
        base_ref,
        base_sha,
        started_at,
        research,
        Value::String("s00000000000000aa".to_string()),
    );
}

#[allow(clippy::too_many_arguments)]
fn write_start_file_with_surface(
    root: &Path,
    id: &str,
    request: Option<&str>,
    base_ref: &str,
    base_sha: &str,
    started_at: u64,
    research: bool,
    surface: Value,
) {
    let mut obj = json!({
        "id": id,
        "base": base_ref,
        "base_sha": base_sha,
        "started_at": started_at,
        "research": research,
        "surface": surface,
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

fn write_start_file(
    root: &Path,
    id: &str,
    request: Option<&str>,
    base_sha: &str,
    started_at: u64,
    research: bool,
) {
    write_start_file_with_base(root, id, request, "main", base_sha, started_at, research);
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

fn make_surface_with_session(
    root: PathBuf,
    session: Session,
    publisher: Arc<dyn Publisher>,
) -> Surface {
    let repo = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string();
    Surface {
        surface_id: "s00000000000000aa".to_string(),
        surface_name: repo.clone(),
        root,
        repo,
        session,
        session_id: "sess-req-test".to_string(),
        started_at: 1_700_000_000,
        days: 14,
        token: generate_token().unwrap(),
        message_credential: generate_token().unwrap(),
        open_codes: blyte_cli::serve::OpenCodes::new(),
        requests: Arc::new(RequestsState::new()),
        publisher,
        reconcile_lock: Mutex::new(()),
        reconcile_requested: std::sync::atomic::AtomicBool::new(false),
    }
}

fn make_surface(
    root: PathBuf,
    publisher: Arc<dyn Publisher>,
) -> (Surface, Arc<Mutex<Vec<String>>>) {
    let (runner, prompts) = RecordingRunner::new();
    let session = Session::start(Box::new(runner), "gemini-4-neon".to_string());
    (make_surface_with_session(root, session, publisher), prompts)
}

fn get(surface: &Surface, path: &str) -> Request {
    Request {
        method: "GET".to_string(),
        path: path.to_string(),
        query: Vec::new(),
        headers: vec![(TOKEN_HEADER.to_string(), surface.token.clone())],
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
    let deadline = Instant::now() + WAIT;
    let mut after = 0;
    let mut count = 0;
    let mut seen = Vec::new();
    loop {
        let entries = session.entries_after(after, Duration::from_millis(50));
        for e in entries {
            after = after.max(e.seq);
            if matches!(e.body, EntryBody::Idle { .. }) {
                count += 1;
            }
            seen.push(e);
        }
        if count >= expected_count {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected_count} idle entries (saw {count}); entries={seen:?}"
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
    assert_eq!(
        request_readiness(root, "req-redone").unwrap(),
        Readiness::Waiting {
            reasons: vec!["every run was redone".to_string()],
        }
    );

    write_start_file(root, "T9", Some("req-redone"), &base, 109, false);
    write_summary(root, "T9", "verified", Some("wip/T9"));
    write_checks(root, "T9", &[]);
    assert_eq!(
        request_readiness(root, "req-redone").unwrap(),
        Readiness::Ready
    );
}

#[test]
fn trigger_combines_ready_and_skips_waiting_or_unrequested() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branches(
        root,
        &[
            ("A1", &base, "a.txt", "a\n", "feat: add a"),
            ("B1", &base, "b.txt", "b\n", "feat: add b"),
            ("C1", &base, "c.txt", "c\n", "feat: add c"),
            ("D1", &base, "d.txt", "d\n", "feat: add d"),
        ],
    );
    write_start_file(root, "A1", Some("req-a"), &base, now, false);
    write_summary(root, "A1", "verified", Some("wip/A1"));
    write_checks(root, "A1", &[]);

    write_start_file(root, "B1", Some("req-b"), &base, now, false);
    write_summary(root, "B1", "verified", Some("wip/B1"));
    write_checks(root, "B1", &["needs review"]);

    write_start_file(root, "C1", None, &base, now, false);
    write_summary(root, "C1", "verified", Some("wip/C1"));
    write_checks(root, "C1", &[]);

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

    let runs_resp = route(&surface, &get(&surface, "/api/runs"), now + 6);
    assert_eq!(runs_resp.status, 200);
    surface.requests.wait_all_idle();
    assert!(!root.join(".wt/combined/req-b.json").exists());
    assert!(root.join(".wt/combined/req-poll.json").exists());
    let poll_rec: CombineRecord = serde_json::from_str(
        &std::fs::read_to_string(root.join(".wt/combined/req-poll.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(poll_rec.state, CombineState::Combined);

    let reqs = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 7));
    let ids: Vec<&str> = reqs
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["request"].as_str().unwrap())
        .collect();
    assert_eq!(ids[0], "req-poll");

    assert_session_idle(&surface.session);
    assert!(prompts.lock().unwrap().is_empty());
}

#[test]
fn conflict_queues_once_and_success_queues_none() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();

    create_run_branches(
        root,
        &[
            ("X1", &base, "shared.txt", "from x1\n", "feat: x1"),
            ("X2", &base, "shared.txt", "from x2\n", "feat: x2"),
        ],
    );
    write_start_file(root, "X1", Some("req-conflict"), &base, 100, false);
    write_summary(root, "X1", "verified", Some("wip/X1"));
    write_checks(root, "X1", &[]);

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
    assert_session_idle(&surface.session);
    assert_eq!(prompts.lock().unwrap().len(), 1);

    create_run_branch(root, "OK1", &base, "ok.txt", "ok\n", "feat: ok");
    write_start_file(root, "OK1", Some("req-ok"), &base, 102, false);
    write_summary(root, "OK1", "verified", Some("wip/OK1"));
    write_checks(root, "OK1", &[]);

    assert!(maybe_combine(&surface, "req-ok").unwrap());
    surface.requests.wait_for_idle("req-ok");
    assert_session_idle(&surface.session);
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
    assert!(body["decision"]["conversation_message"].is_null());

    surface.requests.wait_for_idle("req-accept");

    let rec_path = root.join(".wt/combined/req-accept.json");
    let rec: CombineRecord =
        serde_json::from_str(&std::fs::read_to_string(&rec_path).unwrap()).unwrap();
    assert_eq!(rec.state, CombineState::Combined);
    assert_eq!(rec.branch, "blyte/req-accept");

    assert_session_idle(&surface.session);
    assert!(prompts.lock().unwrap().is_empty());
}

#[test]
fn api_requests_shows_waiting_then_combined_with_suggested_pr_fields() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branches(
        root,
        &[
            ("R1", &base, "one.txt", "1\n", "feat: first change"),
            ("R2", &base, "two.txt", "2\n", "feat: second change"),
        ],
    );
    write_start_file(root, "R1", Some("req-multi"), &base, now, false);
    write_summary(root, "R1", "verified", Some("wip/R1"));
    write_checks(root, "R1", &[]);

    write_start_file(root, "R2", Some("req-multi"), &base, now + 1, false);
    write_summary(root, "R2", "verified", Some("wip/R2"));
    write_checks(root, "R2", &["model review flagged"]);

    let (surface, _prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));

    let resp1 = route(&surface, &get(&surface, "/api/requests"), now);
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

    let resp2 = route(&surface, &get(&surface, "/api/requests"), now + 3);
    assert_eq!(resp2.status, 200);
    let arr2 = parse_json(&resp2);
    let item = &arr2.as_array().unwrap()[0];
    assert_eq!(item["request"], "req-multi");
    assert_eq!(item["state"], "combined");
    assert_eq!(item["check"], "ran");
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

    let reqs = parse_json(&route(&surface, &get(&surface, "/api/requests"), now));
    let item = &reqs.as_array().unwrap()[0];
    assert_eq!(item["pushed"], true);
    assert_eq!(
        item["pull_request_url"],
        "https://github.com/example/repo/pull/42"
    );

    let (restarted_surface, _) = make_surface(root.to_path_buf(), fake.clone());
    let restarted_reqs = parse_json(&route(
        &restarted_surface,
        &get(&restarted_surface, "/api/requests"),
        now,
    ));
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

    let entries = surface.session.entries_after(0, Duration::ZERO);
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
fn check_failed_queues_conversation_message_with_step_and_output() {
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

    let reqs = parse_json(&route(
        &surface,
        &get(&surface, "/api/requests"),
        now_secs(),
    ));
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

    let reqs = parse_json(&route(
        &surface,
        &get(&surface, "/api/requests"),
        now_secs(),
    ));
    let item = &reqs.as_array().unwrap()[0];
    assert_eq!(item["state"], "error");
    assert!(item["error"].as_str().unwrap().contains("blyte.toml"));

    if maybe_combine(&surface, "req-err").unwrap() {
        surface.requests.wait_for_idle("req-err");
    }
    assert_session_idle(&surface.session);
    assert_eq!(prompts.lock().unwrap().len(), 1);
}

#[test]
fn check_failed_does_not_rerun_on_second_trigger_or_restart_and_retries_unreported() {
    let marker_dir = tempfile::tempdir().unwrap();
    let marker = marker_dir.path().join("gate-runs.log");
    let gate = format!("echo run >> {} && false", marker.display());
    let (repo, base) = init_repo_with_gate(&gate);
    let root = repo.path();

    create_run_branch(root, "CF1", &base, "cf.txt", "v1\n", "feat: cf");
    write_start_file(root, "CF1", Some("req-cf"), &base, 100, false);
    write_summary(root, "CF1", "verified", Some("wip/CF1"));
    write_checks(root, "CF1", &[]);

    let (surface, prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));

    assert!(maybe_combine(&surface, "req-cf").unwrap());
    surface.requests.wait_for_idle("req-cf");
    wait_for_idle_entries(&surface.session, 1);
    assert_eq!(prompts.lock().unwrap().len(), 1);

    let attempt = load_attempt_record(root, "req-cf").unwrap().unwrap();
    assert_eq!(attempt.result, AttemptResult::CheckFailed);
    assert!(attempt.reported);
    assert!(attempt.error.is_none());

    assert!(!maybe_combine(&surface, "req-cf").unwrap());
    assert_session_idle(&surface.session);
    assert_eq!(prompts.lock().unwrap().len(), 1);

    let (restarted, restarted_prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));
    assert!(!maybe_combine(&restarted, "req-cf").unwrap());
    assert_session_idle(&restarted.session);
    assert!(restarted_prompts.lock().unwrap().is_empty());

    let gate_count = std::fs::read_to_string(&marker).unwrap().lines().count();
    assert_eq!(gate_count, 1);

    let mut unreported = attempt;
    unreported.reported = false;
    write_attempt_record(root, "req-cf", &unreported).unwrap();

    assert!(!maybe_combine(&restarted, "req-cf").unwrap());
    wait_for_idle_entries(&restarted.session, 1);
    assert_eq!(restarted_prompts.lock().unwrap().len(), 1);
    assert!(
        load_attempt_record(root, "req-cf")
            .unwrap()
            .unwrap()
            .reported
    );
    let gate_count_after_retry = std::fs::read_to_string(&marker).unwrap().lines().count();
    assert_eq!(gate_count_after_retry, 1);
}

#[test]
fn error_request_does_not_rerun_and_persists_error_state_after_restart() {
    let (repo, base1) = init_repo_with_gate("true");
    let root = repo.path();

    let base2 = fast_import_commit(
        root,
        "main",
        Some(&base1),
        &[("extra.txt", "second base\n")],
        "second base commit",
    );

    create_run_branch(root, "EB1", &base1, "b1.txt", "1\n", "feat: from base1");
    write_start_file(root, "EB1", Some("req-diffbase"), &base1, 100, false);
    write_summary(root, "EB1", "verified", Some("wip/EB1"));
    write_checks(root, "EB1", &[]);

    create_run_branch(root, "EB2", &base2, "b2.txt", "2\n", "feat: from base2");
    write_start_file(root, "EB2", Some("req-diffbase"), &base2, 101, false);
    write_summary(root, "EB2", "verified", Some("wip/EB2"));
    write_checks(root, "EB2", &[]);

    let (surface, prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));

    assert!(maybe_combine(&surface, "req-diffbase").unwrap());
    surface.requests.wait_for_idle("req-diffbase");
    wait_for_idle_entries(&surface.session, 1);
    assert_eq!(prompts.lock().unwrap().len(), 1);

    let attempt = load_attempt_record(root, "req-diffbase").unwrap().unwrap();
    assert_eq!(attempt.result, AttemptResult::Error);
    assert!(attempt.reported);
    assert!(attempt
        .error
        .as_deref()
        .unwrap()
        .contains("different base commits"));

    assert!(!maybe_combine(&surface, "req-diffbase").unwrap());
    assert_session_idle(&surface.session);
    assert_eq!(prompts.lock().unwrap().len(), 1);

    let (restarted, restarted_prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));
    assert!(!maybe_combine(&restarted, "req-diffbase").unwrap());
    assert_session_idle(&restarted.session);
    assert!(restarted_prompts.lock().unwrap().is_empty());

    let reqs = parse_json(&route(
        &restarted,
        &get(&restarted, "/api/requests"),
        now_secs(),
    ));
    let item = &reqs.as_array().unwrap()[0];
    assert_eq!(item["request"], "req-diffbase");
    assert_eq!(item["state"], "error");
    assert!(item["error"]
        .as_str()
        .unwrap()
        .contains("different base commits"));
}

#[test]
fn changing_run_tip_triggers_combine_again() {
    let (repo, base) = init_repo_with_gate("test -f pass.flag");
    let root = repo.path();

    let tip1 = create_run_branch(root, "TC1", &base, "work.txt", "v1\n", "feat: first try");
    write_start_file(root, "TC1", Some("req-tipchange"), &base, 100, false);
    write_summary(root, "TC1", "verified", Some("wip/TC1"));
    write_checks(root, "TC1", &[]);

    let (surface, prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));

    assert!(maybe_combine(&surface, "req-tipchange").unwrap());
    surface.requests.wait_for_idle("req-tipchange");
    wait_for_idle_entries(&surface.session, 1);
    assert_eq!(prompts.lock().unwrap().len(), 1);
    assert!(!maybe_combine(&surface, "req-tipchange").unwrap());

    let tip2 = fast_import_commit(
        root,
        "wip/TC1",
        Some(&tip1),
        &[("pass.flag", "ok\n")],
        "fix: make gate pass",
    );
    assert_ne!(tip1, tip2);

    assert!(maybe_combine(&surface, "req-tipchange").unwrap());
    surface.requests.wait_for_idle("req-tipchange");

    let attempt = load_attempt_record(root, "req-tipchange").unwrap().unwrap();
    assert_eq!(attempt.result, AttemptResult::Combined);
    assert_eq!(attempt.runs.len(), 1);
    assert_eq!(attempt.runs[0].tip, tip2);

    let reqs = parse_json(&route(
        &surface,
        &get(&surface, "/api/requests"),
        now_secs(),
    ));
    let item = &reqs.as_array().unwrap()[0];
    assert_eq!(item["state"], "combined");
    assert!(!maybe_combine(&surface, "req-tipchange").unwrap());
}

#[test]
fn push_then_rebuild_invalidates_pushed_and_blocks_pr_until_repushed() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "RP1", &base, "item.txt", "v1\n", "feat: v1");
    write_start_file(root, "RP1", Some("req-repush"), &base, now, false);
    write_summary(root, "RP1", "verified", Some("wip/RP1"));
    write_checks(root, "RP1", &[]);

    let fake = FakePublisher::ok("https://github.com/example/repo/pull/99");
    let (surface, _prompts) = make_surface(root.to_path_buf(), fake.clone());

    assert!(maybe_combine(&surface, "req-repush").unwrap());
    surface.requests.wait_for_idle("req-repush");

    let push1 = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-repush/push",
            Some(&surface.token),
            &json!({}),
        ),
        now,
    );
    assert_eq!(push1.status, 200);

    let pr1 = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-repush/pull-request",
            Some(&surface.token),
            &json!({ "title": "PR v1", "body": "initial" }),
        ),
        now,
    );
    assert_eq!(pr1.status, 200);

    fast_import_commit(
        root,
        "wip/RP1",
        Some("refs/heads/wip/RP1^0"),
        &[("item.txt", "v2\n")],
        "feat: v2",
    );

    assert!(maybe_combine(&surface, "req-repush").unwrap());
    surface.requests.wait_for_idle("req-repush");

    let reqs_after_rebuild = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 1));
    let item = &reqs_after_rebuild.as_array().unwrap()[0];
    assert_eq!(item["state"], "combined");
    assert_eq!(item["pushed"], false);
    assert_eq!(
        item["pull_request_url"],
        "https://github.com/example/repo/pull/99"
    );

    let pr_blocked = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-repush/pull-request",
            Some(&surface.token),
            &json!({ "title": "PR v2", "body": "updated" }),
        ),
        now + 2,
    );
    assert_eq!(pr_blocked.status, 409);

    let push2 = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-repush/push",
            Some(&surface.token),
            &json!({}),
        ),
        now + 3,
    );
    assert_eq!(push2.status, 200);

    let reqs_after_repush = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 4));
    let item2 = &reqs_after_repush.as_array().unwrap()[0];
    assert_eq!(item2["pushed"], true);
    assert_eq!(
        item2["pull_request_url"],
        "https://github.com/example/repo/pull/99"
    );

    let pr_ok = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-repush/pull-request",
            Some(&surface.token),
            &json!({ "title": "PR v2", "body": "updated" }),
        ),
        now + 5,
    );
    assert_eq!(pr_ok.status, 200);
}

#[test]
fn unparseable_attempt_and_old_shape_published_files_are_errors_naming_the_file() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branches(
        root,
        &[
            ("BA1", &base, "ba.txt", "a\n", "feat: ba"),
            ("BP1", &base, "bp.txt", "p\n", "feat: bp"),
        ],
    );
    write_start_file(root, "BA1", Some("req-bad-attempt"), &base, now, false);
    write_summary(root, "BA1", "verified", Some("wip/BA1"));
    write_checks(root, "BA1", &[]);

    write_start_file(root, "BP1", Some("req-old-pub"), &base, now + 1, false);
    write_summary(root, "BP1", "verified", Some("wip/BP1"));
    write_checks(root, "BP1", &[]);

    let combined_dir = root.join(".wt/combined");
    std::fs::create_dir_all(&combined_dir).unwrap();
    let bad_attempt_path = combined_dir.join("req-bad-attempt.attempt.json");
    std::fs::write(&bad_attempt_path, "{not valid json").unwrap();

    let (surface, _prompts) = make_surface(root.to_path_buf(), FakePublisher::ok("https://pr"));

    let attempt_err = load_attempt_record(root, "req-bad-attempt").unwrap_err();
    assert!(
        attempt_err.contains("req-bad-attempt.attempt.json"),
        "{attempt_err}"
    );
    let trigger_err = maybe_combine(&surface, "req-bad-attempt").unwrap_err();
    assert!(
        trigger_err.contains("req-bad-attempt.attempt.json"),
        "{trigger_err}"
    );

    assert!(maybe_combine(&surface, "req-old-pub").unwrap());
    surface.requests.wait_for_idle("req-old-pub");

    let old_pub_path = combined_dir.join("req-old-pub.published.json");
    std::fs::write(
        &old_pub_path,
        "{\"pushed\":true,\"pull_request_url\":null}\n",
    )
    .unwrap();

    let pub_err = load_published_record(root, "req-old-pub").unwrap_err();
    assert!(pub_err.contains("req-old-pub.published.json"), "{pub_err}");

    let runs_resp = route(&surface, &get(&surface, "/api/runs"), now + 2);
    assert_eq!(runs_resp.status, 200);
    let session_entries = surface.session.entries_after(0, Duration::ZERO);
    assert!(session_entries.iter().any(|e| matches!(
        &e.body,
        EntryBody::Error { message } if message.contains("req-bad-attempt.attempt.json")
    )));

    let reqs = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 3));
    let items = reqs.as_array().unwrap();
    let bad_attempt_item = items
        .iter()
        .find(|v| v["request"] == "req-bad-attempt")
        .unwrap();
    assert_eq!(bad_attempt_item["state"], "error");
    assert!(bad_attempt_item["error"]
        .as_str()
        .unwrap()
        .contains("req-bad-attempt.attempt.json"));

    let old_pub_item = items
        .iter()
        .find(|v| v["request"] == "req-old-pub")
        .unwrap();
    assert_eq!(old_pub_item["state"], "error");
    assert!(old_pub_item["error"]
        .as_str()
        .unwrap()
        .contains("req-old-pub.published.json"));

    let pr_resp = route(
        &surface,
        &post_json_with_token(
            "/api/requests/req-old-pub/pull-request",
            Some(&surface.token),
            &json!({ "title": "Old shape", "body": "" }),
        ),
        now + 4,
    );
    assert_eq!(pr_resp.status, 500);
    assert!(parse_json(&pr_resp)["error"]
        .as_str()
        .unwrap()
        .contains("req-old-pub.published.json"));
}

#[test]
fn redone_runs_left_out_regardless_of_outcome_and_reported_in_api() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branches(
        root,
        &[
            ("L1", &base, "a.txt", "a\n", "feat: keep a"),
            ("L2", &base, "b.txt", "b\n", "feat: drop verified"),
            ("L3", &base, "c.txt", "c\n", "feat: drop gate_failed"),
            ("L4", &base, "d.txt", "d\n", "feat: drop unfinished"),
            ("N1", &base, "n.txt", "n\n", "feat: normal"),
        ],
    );
    write_start_file(root, "L1", Some("req-leftout"), &base, now, false);
    write_summary(root, "L1", "verified", Some("wip/L1"));
    write_checks(root, "L1", &[]);

    write_start_file(root, "L2", Some("req-leftout"), &base, now + 1, false);
    write_summary(root, "L2", "verified", Some("wip/L2"));
    write_checks(root, "L2", &[]);
    record_verdict(root, "L2", ReviewVerdict::Redone, "replaced", now + 2).unwrap();

    write_start_file(root, "L3", Some("req-leftout"), &base, now + 3, false);
    write_summary(root, "L3", "gate_failed", Some("wip/L3"));
    record_verdict(root, "L3", ReviewVerdict::Redone, "replaced", now + 4).unwrap();

    write_start_file(root, "L4", Some("req-leftout"), &base, now + 5, false);
    record_verdict(root, "L4", ReviewVerdict::Redone, "replaced", now + 6).unwrap();

    write_start_file(root, "N1", Some("req-normal"), &base, now + 7, false);
    write_summary(root, "N1", "verified", Some("wip/N1"));
    write_checks(root, "N1", &[]);

    assert_eq!(
        request_readiness(root, "req-leftout").unwrap(),
        Readiness::Ready
    );

    let (surface, _prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));
    assert!(maybe_combine(&surface, "req-leftout").unwrap());
    surface.requests.wait_for_idle("req-leftout");

    let reqs = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 10));
    let items = reqs.as_array().unwrap();

    let leftout_item = items
        .iter()
        .find(|v| v["request"] == "req-leftout")
        .unwrap();
    assert_eq!(leftout_item["state"], "combined");
    assert_eq!(leftout_item["runs"], json!(["L1", "L2", "L3", "L4"]));
    assert_eq!(leftout_item["left_out"], json!(["L2", "L3", "L4"]));
    let commits = leftout_item["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0]["subject"], "feat: keep a");

    let combine_wt = root.join(".wt/combine-req-leftout");
    assert!(combine_wt.join("a.txt").exists());
    assert!(!combine_wt.join("b.txt").exists());
    assert!(!combine_wt.join("c.txt").exists());
    assert!(!combine_wt.join("d.txt").exists());

    let normal_item = items.iter().find(|v| v["request"] == "req-normal").unwrap();
    assert_eq!(normal_item["left_out"], json!([]));
}

#[test]
fn run_based_on_left_out_run_is_readiness_and_api_error() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    let t1_tip = create_run_branch(root, "D1", &base, "a.txt", "a\n", "feat: base run");
    write_start_file(root, "D1", Some("req-dep-redone"), &base, now, false);
    write_summary(root, "D1", "verified", Some("wip/D1"));
    write_checks(root, "D1", &[]);
    record_verdict(root, "D1", ReviewVerdict::Redone, "replaced", now + 1).unwrap();

    create_run_branch(root, "D2", &t1_tip, "b.txt", "b\n", "feat: child run");
    write_start_file_with_base(
        root,
        "D2",
        Some("req-dep-redone"),
        "wip/D1",
        &t1_tip,
        now + 2,
        false,
    );
    write_summary(root, "D2", "verified", Some("wip/D2"));
    write_checks(root, "D2", &[]);

    let err = request_readiness(root, "req-dep-redone").unwrap_err();
    assert!(err.contains("D1") && err.contains("D2"), "{err}");

    let (surface, _prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));
    let reqs = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 3));
    let item = &reqs.as_array().unwrap()[0];
    assert_eq!(item["request"], "req-dep-redone");
    assert_eq!(item["state"], "error");
    assert_eq!(item["left_out"], json!(["D1"]));
    let api_err = item["error"].as_str().unwrap();
    assert!(
        api_err.contains("D1") && api_err.contains("D2"),
        "{api_err}"
    );
}

#[test]
fn every_run_redone_is_waiting_in_readiness_and_api() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branches(
        root,
        &[
            ("AR1", &base, "a.txt", "a\n", "feat: a"),
            ("AR2", &base, "b.txt", "b\n", "feat: b"),
        ],
    );
    write_start_file(root, "AR1", Some("req-all-redone"), &base, now, false);
    write_summary(root, "AR1", "verified", Some("wip/AR1"));
    write_checks(root, "AR1", &[]);
    record_verdict(root, "AR1", ReviewVerdict::Redone, "redo", now + 1).unwrap();

    write_start_file(root, "AR2", Some("req-all-redone"), &base, now + 2, false);
    record_verdict(root, "AR2", ReviewVerdict::Redone, "redo", now + 3).unwrap();

    assert_eq!(
        request_readiness(root, "req-all-redone").unwrap(),
        Readiness::Waiting {
            reasons: vec!["every run was redone".to_string()],
        }
    );

    let (surface, _prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));
    assert!(!maybe_combine(&surface, "req-all-redone").unwrap());

    let reqs = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 4));
    let item = &reqs.as_array().unwrap()[0];
    assert_eq!(item["request"], "req-all-redone");
    assert_eq!(item["state"], "waiting");
    assert_eq!(item["waiting_reasons"], json!(["every run was redone"]));
    assert_eq!(item["runs"], json!(["AR1", "AR2"]));
    assert_eq!(item["left_out"], json!(["AR1", "AR2"]));
}

#[test]
fn recording_redone_on_combined_run_triggers_recombine() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branches(
        root,
        &[
            ("RC1", &base, "a.txt", "a\n", "feat: add a"),
            ("RC2", &base, "b.txt", "b\n", "feat: add b"),
        ],
    );
    write_start_file(root, "RC1", Some("req-recombine"), &base, now, false);
    write_summary(root, "RC1", "verified", Some("wip/RC1"));
    write_checks(root, "RC1", &[]);

    write_start_file(root, "RC2", Some("req-recombine"), &base, now + 1, false);
    write_summary(root, "RC2", "verified", Some("wip/RC2"));
    write_checks(root, "RC2", &[]);

    let (surface, _prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));
    assert!(maybe_combine(&surface, "req-recombine").unwrap());
    surface.requests.wait_for_idle("req-recombine");

    let reqs1 = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 2));
    let item1 = &reqs1.as_array().unwrap()[0];
    assert_eq!(item1["state"], "combined");
    assert_eq!(item1["left_out"], json!([]));
    assert_eq!(item1["commits"].as_array().unwrap().len(), 2);

    record_verdict(root, "RC2", ReviewVerdict::Redone, "replaced", now + 3).unwrap();

    let reqs_stale = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 4));
    let item_stale = &reqs_stale.as_array().unwrap()[0];
    assert_eq!(item_stale["state"], "waiting");
    assert_eq!(item_stale["left_out"], json!(["RC2"]));

    assert!(maybe_combine(&surface, "req-recombine").unwrap());
    surface.requests.wait_for_idle("req-recombine");

    let reqs2 = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 5));
    let item2 = &reqs2.as_array().unwrap()[0];
    assert_eq!(item2["state"], "combined");
    assert_eq!(item2["runs"], json!(["RC1", "RC2"]));
    assert_eq!(item2["left_out"], json!(["RC2"]));
    let commits = item2["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0]["subject"], "feat: add a");

    let combine_wt = root.join(".wt/combine-req-recombine");
    assert!(combine_wt.join("a.txt").exists());
    assert!(!combine_wt.join("b.txt").exists());

    assert!(!maybe_combine(&surface, "req-recombine").unwrap());
}

#[test]
fn unreadable_verdict_file_is_error_in_readiness_and_api() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "BV1", &base, "a.txt", "a\n", "feat: a");
    write_start_file(root, "BV1", Some("req-bad-verdict"), &base, now, false);
    write_summary(root, "BV1", "verified", Some("wip/BV1"));
    write_checks(root, "BV1", &[]);
    std::fs::write(root.join(".wt/BV1.verdict.json"), "{broken json").unwrap();

    let err = request_readiness(root, "req-bad-verdict").unwrap_err();
    assert!(err.contains("BV1.verdict.json"), "{err}");

    let (surface, _prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));
    let reqs = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 1));
    let item = &reqs.as_array().unwrap()[0];
    assert_eq!(item["request"], "req-bad-verdict");
    assert_eq!(item["state"], "error");
    assert!(item["error"].as_str().unwrap().contains("BV1.verdict.json"));
}

struct BlockingFirstHandle {
    prompts: Arc<Mutex<Vec<String>>>,
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}

struct BlockingFirstRunner {
    prompts: Arc<Mutex<Vec<String>>>,
    entered: Option<mpsc::Sender<()>>,
    release: Option<mpsc::Receiver<()>>,
}

impl BlockingFirstRunner {
    fn new() -> (Self, BlockingFirstHandle) {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        (
            Self {
                prompts: Arc::clone(&prompts),
                entered: Some(entered_tx),
                release: Some(release_rx),
            },
            BlockingFirstHandle {
                prompts,
                entered: entered_rx,
                release: release_tx,
            },
        )
    }
}

impl PromptRunner for BlockingFirstRunner {
    fn run(
        &mut self,
        prompt: &str,
        _model: &str,
        _emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        if let Some(tx) = self.entered.take() {
            let _ = tx.send(());
        }
        if let Some(rx) = self.release.take() {
            let _ = rx.recv_timeout(Duration::from_secs(10));
        }
        Ok(())
    }
}

fn branch_exists(root: &Path, branch: &str) -> bool {
    Command::new("git")
        .args(["rev-parse", "--verify", &format!("refs/heads/{branch}")])
        .current_dir(root)
        .output()
        .expect("git rev-parse")
        .status
        .success()
}

#[test]
fn ready_request_waits_while_session_busy_then_combines_when_idle() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "BS1", &base, "busy.txt", "busy\n", "feat: busy");
    write_start_file(root, "BS1", Some("req-busy"), &base, now, false);
    write_summary(root, "BS1", "verified", Some("wip/BS1"));
    write_checks(root, "BS1", &[]);

    let (runner, handle) = BlockingFirstRunner::new();
    let session = Session::start(Box::new(runner), "gemini-4-neon".to_string());
    let surface = make_surface_with_session(root.to_path_buf(), session, Arc::new(RealPublisher));

    surface
        .session
        .submit("working on next dispatch", Vec::new())
        .unwrap();
    handle.entered.recv_timeout(WAIT).unwrap_or_else(|err| {
        panic!(
            "timed out waiting for BlockingFirstRunner to enter run: {err:?}; prompts={:?}",
            handle.prompts.lock().unwrap()
        )
    });

    let busy_info = surface.session.info();
    assert!(busy_info.busy);
    assert_eq!(busy_info.queued, 0);

    assert!(!maybe_combine(&surface, "req-busy").unwrap());
    assert!(!root.join(".wt/combined/req-busy.json").exists());
    assert!(!branch_exists(root, "blyte/req-busy"));

    let reqs_busy = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 1));
    let item_busy = &reqs_busy.as_array().unwrap()[0];
    assert_eq!(item_busy["request"], "req-busy");
    assert_eq!(item_busy["state"], "waiting");
    assert_eq!(
        item_busy["waiting_reasons"],
        json!(["the conversation model is still working"])
    );

    handle.release.send(()).unwrap();
    wait_for_idle_entries(&surface.session, 1);

    let idle_info = surface.session.info();
    assert!(!idle_info.busy);
    assert_eq!(idle_info.queued, 0);

    assert!(maybe_combine(&surface, "req-busy").unwrap());
    surface.requests.wait_for_idle("req-busy");

    let rec: CombineRecord = serde_json::from_str(
        &std::fs::read_to_string(root.join(".wt/combined/req-busy.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(rec.state, CombineState::Combined);
    assert_eq!(rec.branch, "blyte/req-busy");
    assert!(branch_exists(root, "blyte/req-busy"));
}

#[test]
fn ready_request_waits_while_session_has_queued_message_then_combines_when_idle() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "QS1", &base, "queued.txt", "queued\n", "feat: queued");
    write_start_file(root, "QS1", Some("req-queued"), &base, now, false);
    write_summary(root, "QS1", "verified", Some("wip/QS1"));
    write_checks(root, "QS1", &[]);

    let (runner, _prompts) = RecordingRunner::new();
    let (session, start_worker) =
        Session::start_paused(Box::new(runner), "gemini-4-neon".to_string());
    let surface = make_surface_with_session(root.to_path_buf(), session, Arc::new(RealPublisher));

    surface
        .session
        .submit("queued follow-up", Vec::new())
        .unwrap();

    let queued_info = surface.session.info();
    assert!(!queued_info.busy);
    assert_eq!(queued_info.queued, 1);

    assert!(!maybe_combine(&surface, "req-queued").unwrap());
    assert!(!root.join(".wt/combined/req-queued.json").exists());
    assert!(!branch_exists(root, "blyte/req-queued"));

    let reqs_queued = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 1));
    let item_queued = &reqs_queued.as_array().unwrap()[0];
    assert_eq!(item_queued["request"], "req-queued");
    assert_eq!(item_queued["state"], "waiting");
    assert_eq!(
        item_queued["waiting_reasons"],
        json!(["the conversation model is still working"])
    );

    start_worker.send(()).unwrap();
    wait_for_idle_entries(&surface.session, 1);

    let idle_info = surface.session.info();
    assert!(!idle_info.busy);
    assert_eq!(idle_info.queued, 0);

    assert!(maybe_combine(&surface, "req-queued").unwrap());
    surface.requests.wait_for_idle("req-queued");

    let rec: CombineRecord = serde_json::from_str(
        &std::fs::read_to_string(root.join(".wt/combined/req-queued.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(rec.state, CombineState::Combined);
    assert_eq!(rec.branch, "blyte/req-queued");
    assert!(branch_exists(root, "blyte/req-queued"));
}

#[test]
fn conflict_failure_message_is_not_queued_while_busy_and_queued_once_after() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branches(
        root,
        &[
            ("BC1", &base, "shared.txt", "from bc1\n", "feat: bc1"),
            ("BC2", &base, "shared.txt", "from bc2\n", "feat: bc2"),
        ],
    );
    write_start_file(root, "BC1", Some("req-busy-conflict"), &base, now, false);
    write_summary(root, "BC1", "verified", Some("wip/BC1"));
    write_checks(root, "BC1", &[]);

    write_start_file(
        root,
        "BC2",
        Some("req-busy-conflict"),
        &base,
        now + 1,
        false,
    );
    write_summary(root, "BC2", "verified", Some("wip/BC2"));
    write_checks(root, "BC2", &[]);

    let (runner, handle) = BlockingFirstRunner::new();
    let session = Session::start(Box::new(runner), "gemini-4-neon".to_string());
    let surface = make_surface_with_session(root.to_path_buf(), session, Arc::new(RealPublisher));

    surface
        .session
        .submit("active conversation model turn", Vec::new())
        .unwrap();
    handle.entered.recv_timeout(WAIT).unwrap_or_else(|err| {
        panic!(
            "timed out waiting for BlockingFirstRunner to enter run: {err:?}; prompts={:?}",
            handle.prompts.lock().unwrap()
        )
    });

    assert!(surface.session.info().busy);
    assert_eq!(surface.session.info().queued, 0);

    assert!(!maybe_combine(&surface, "req-busy-conflict").unwrap());
    assert!(!root.join(".wt/combined/req-busy-conflict.json").exists());
    assert_eq!(surface.session.info().queued, 0);
    assert_eq!(handle.prompts.lock().unwrap().len(), 1);

    handle.release.send(()).unwrap();
    wait_for_idle_entries(&surface.session, 1);

    assert!(!surface.session.info().busy);
    assert_eq!(surface.session.info().queued, 0);
    assert_eq!(handle.prompts.lock().unwrap().len(), 1);

    assert!(maybe_combine(&surface, "req-busy-conflict").unwrap());
    surface.requests.wait_for_idle("req-busy-conflict");
    wait_for_idle_entries(&surface.session, 2);

    {
        let recorded = handle.prompts.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        assert!(recorded[1].contains("req-busy-conflict"), "{}", recorded[1]);
        assert!(recorded[1].contains("conflict"), "{}", recorded[1]);
        assert!(recorded[1].contains("BC2"), "{}", recorded[1]);
        assert!(recorded[1].contains("shared.txt"), "{}", recorded[1]);
    }

    assert!(!maybe_combine(&surface, "req-busy-conflict").unwrap());
    assert_session_idle(&surface.session);
    assert_eq!(handle.prompts.lock().unwrap().len(), 2);
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

fn ensure_isolated_state_home() {
    static STATE_HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    STATE_HOME.get_or_init(|| {
        let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("test-requests-state-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::env::set_var("XDG_STATE_HOME", &base);
        std::env::set_var("BLYTE_STATE_DIR", base.join("blyte"));
        base
    });
}

fn start_stoppable_surface(
    repo: &Path,
    surface_id: &str,
    surface_name: &str,
) -> (u16, String, Arc<AtomicBool>, std::thread::JoinHandle<i32>) {
    ensure_isolated_state_home();
    let rec = blyte_cli::surface_dir::SurfaceRecord {
        id: surface_id.to_string(),
        name: surface_name.to_string(),
        pid: 0,
        port: 0,
        session_id: format!("sess-{surface_name}"),
        started_at: 1_700_000_000,
    };
    blyte_cli::surface_dir::write_record_in(
        &blyte_cli::surface_dir::surface_dir(repo, &rec.id),
        &rec,
    )
    .unwrap();
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let repo_bg = repo.to_path_buf();
    let resume_name = surface_name.to_string();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let args = SurfaceArgs {
            port: 0,
            no_open: true,
            days: 14,
            resume: Some(resume_name),
            ..Default::default()
        };
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = |_url: &str| {};
        run_with_hooks(
            &args,
            default_repl_config(repo_bg),
            &mut out,
            &mut err,
            &mut opener,
            stop_bg,
        )
    });

    let deadline = Instant::now() + WAIT;
    let url = loop {
        let s = String::from_utf8_lossy(&out_buf.lock().unwrap()).to_string();
        if let Some(line) = s.lines().find(|l| l.starts_with("blyte: surface ")) {
            if let Some((_, url)) = line.split_once(" at ") {
                if !url.trim().is_empty() {
                    break url.trim().to_string();
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for printed surface URL; stdout={s:?} stderr={:?}",
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        std::thread::sleep(Duration::from_millis(20));
    };

    let after_scheme = url.strip_prefix("http://127.0.0.1:").expect("loopback url");
    let (port_str, token_part) = after_scheme.split_once("/#token=").expect("port and token");
    let port: u16 = port_str.parse().expect("port u16");
    (port, token_part.to_string(), stop, handle)
}

fn http_get_json(port: u16, token: &str, path: &str) -> (u16, Value) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect surface");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{TOKEN_HEADER}: {token}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").expect("http response split");
    let status: u16 = head
        .lines()
        .next()
        .expect("status line")
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse()
        .expect("status u16");
    let val: Value = serde_json::from_str(body).expect("json response body");
    (status, val)
}

fn write_reportable_summary(root: &Path, id: &str, outcome: &str, branch: Option<&str>) {
    let wt = root.join(".wt");
    std::fs::create_dir_all(&wt).unwrap();
    let mut obj = json!({
        "id": id,
        "outcome": outcome,
        "turns": 3,
        "seconds": 12,
        "cost_usd": 0.05,
        "final_message": "done"
    });
    if let Some(b) = branch {
        obj["branch"] = json!(b);
    }
    std::fs::write(
        wt.join(format!("{id}.json")),
        serde_json::to_string_pretty(&obj).unwrap() + "\n",
    )
    .unwrap();
    std::fs::write(wt.join(format!("{id}.report.json")), "{\"turns\":3}\n").unwrap();
}

#[test]
fn a_ready_request_is_combined_with_no_page_request() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "TM1", &base, "timer.txt", "timer\n", "feat: timer");
    write_start_file_with_surface(
        root,
        "TM1",
        Some("req-timer"),
        "main",
        &base,
        now,
        false,
        Value::String("s00000000000000a1".to_string()),
    );
    write_reportable_summary(root, "TM1", "verified", Some("wip/TM1"));
    write_checks(root, "TM1", &["needs review"]);
    decide(root, "TM1", Action::Accept, None, now + 1).unwrap();

    let (_port, _token, stop, handle) =
        start_stoppable_surface(root, "s00000000000000a1", "req-timer-surface");

    let rec_path = root.join(".wt/combined/req-timer.json");
    let attempt_path = root.join(".wt/combined/req-timer.attempt.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    while (!rec_path.is_file() || !attempt_path.is_file()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }

    stop.store(true, Ordering::Relaxed);
    assert_eq!(handle.join().unwrap(), 0);

    assert!(
        rec_path.is_file(),
        "expected {} to exist",
        rec_path.display()
    );
    let rec: CombineRecord =
        serde_json::from_str(&std::fs::read_to_string(&rec_path).unwrap()).unwrap();
    assert_eq!(rec.state, CombineState::Combined);
    assert_eq!(rec.branch, "blyte/req-timer");
}

#[test]
fn timer_and_page_requests_do_not_report_a_run_twice() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "RACE1", &base, "r1.txt", "r1\n", "feat: r1");
    create_run_branch(root, "RACE2", &base, "r2.txt", "r2\n", "feat: r2");
    write_start_file_with_surface(
        root,
        "RACE1",
        Some("req-race-1"),
        "main",
        &base,
        now,
        false,
        Value::String("s00000000000000a2".to_string()),
    );
    write_reportable_summary(root, "RACE1", "verified", Some("wip/RACE1"));
    write_checks(root, "RACE1", &["needs review"]);

    let (port, token, stop, handle) =
        start_stoppable_surface(root, "s00000000000000a2", "req-race-surface");

    let hammer_stop = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let tok = token.clone();
        let done = Arc::clone(&hammer_stop);
        workers.push(std::thread::spawn(move || {
            let mut calls = 0usize;
            while !done.load(Ordering::Relaxed) {
                let (status, _) = http_get_json(port, &tok, "/api/runs");
                assert_eq!(status, 200);
                calls += 1;
            }
            calls
        }));
    }

    std::thread::sleep(Duration::from_millis(1200));
    write_start_file_with_surface(
        root,
        "RACE2",
        Some("req-race-2"),
        "main",
        &base,
        now + 1,
        false,
        Value::String("s00000000000000a2".to_string()),
    );
    write_reportable_summary(root, "RACE2", "verified", Some("wip/RACE2"));
    write_checks(root, "RACE2", &["needs review"]);
    std::thread::sleep(Duration::from_millis(1300));

    hammer_stop.store(true, Ordering::Relaxed);
    for w in workers {
        assert!(w.join().unwrap() > 0);
    }

    let (status, events) = http_get_json(port, &token, "/api/session/events?after=0");
    assert_eq!(status, 200);

    stop.store(true, Ordering::Relaxed);
    assert_eq!(handle.join().unwrap(), 0);

    let entries = events["entries"].as_array().expect("entries array");
    let count_reports = |run_id: &str| {
        entries
            .iter()
            .filter(|e| e["kind"] == "decision" && e["run"] == run_id && e["action"] == "finished")
            .count()
    };
    assert_eq!(count_reports("RACE1"), 1, "entries: {entries:?}");
    assert_eq!(count_reports("RACE2"), 1, "entries: {entries:?}");
}

#[test]
fn the_timer_thread_ends_with_the_server() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    let (_port, _token, stop, handle) =
        start_stoppable_surface(root, "s00000000000000a3", "req-stop-surface");
    stop.store(true, Ordering::Relaxed);
    assert_eq!(handle.join().unwrap(), 0);

    create_run_branch(root, "AFTER1", &base, "after.txt", "after\n", "feat: after");
    write_start_file(root, "AFTER1", Some("req-after-stop"), &base, now, false);
    write_reportable_summary(root, "AFTER1", "verified", Some("wip/AFTER1"));
    write_checks(root, "AFTER1", &["needs review"]);
    decide(root, "AFTER1", Action::Accept, None, now + 1).unwrap();

    std::thread::sleep(Duration::from_secs(5));

    assert!(!root.join(".wt/combined/req-after-stop.json").exists());
    assert!(!branch_exists(root, "blyte/req-after-stop"));
}

#[test]
fn a_conflict_record_without_files_is_reported_as_an_error_naming_the_field() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();

    let tip = create_run_branch(root, "CFN1", &base, "a.txt", "v1\n", "feat: cfn1");
    write_start_file(root, "CFN1", Some("req-bad-conflict"), &base, 100, false);
    write_summary(root, "CFN1", "verified", Some("wip/CFN1"));
    write_checks(root, "CFN1", &[]);

    let runs = vec![CombinedRun {
        id: "CFN1".to_string(),
        tip,
    }];
    let record = CombineRecord {
        request: "req-bad-conflict".to_string(),
        state: CombineState::Conflict,
        branch: "blyte/req-bad-conflict".to_string(),
        base,
        runs: runs.clone(),
        commits: Vec::new(),
        check: CombineCheck::Ran,
        conflicting_run: Some("CFN1".to_string()),
        conflicting_files: Vec::new(),
        failing_step: None,
        output_tail: None,
    };
    let rec_path = root.join(".wt/combined/req-bad-conflict.json");
    std::fs::create_dir_all(rec_path.parent().unwrap()).unwrap();
    std::fs::write(&rec_path, serde_json::to_string_pretty(&record).unwrap()).unwrap();

    write_attempt_record(
        root,
        "req-bad-conflict",
        &AttemptRecord {
            runs,
            result: AttemptResult::Conflict,
            error: None,
            reported: false,
        },
    )
    .unwrap();

    let (surface, prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));
    assert!(!maybe_combine(&surface, "req-bad-conflict").unwrap());
    wait_for_idle_entries(&surface.session, 1);

    let captured = prompts.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let msg = &captured[0];
    assert!(msg.contains("conflicting_files"), "msg: {msg}");
    assert!(
        msg.contains(".wt/combined/req-bad-conflict.json"),
        "msg: {msg}"
    );
    assert!(!msg.contains("(none listed)"), "msg: {msg}");

    let attempt = load_attempt_record(root, "req-bad-conflict")
        .unwrap()
        .unwrap();
    assert_eq!(attempt.result, AttemptResult::Error);
    assert!(attempt.reported);
    let err = attempt.error.as_deref().unwrap_or("");
    assert!(err.contains("conflicting_files"), "err: {err}");
    assert!(
        err.contains(".wt/combined/req-bad-conflict.json"),
        "err: {err}"
    );
    assert!(!err.contains("(none listed)"), "err: {err}");
}

#[test]
fn a_check_failed_record_without_output_is_reported_as_an_error_naming_the_field() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();

    let tip = create_run_branch(root, "CFO1", &base, "a.txt", "v1\n", "feat: cfo1");
    write_start_file(root, "CFO1", Some("req-bad-cf"), &base, 100, false);
    write_summary(root, "CFO1", "verified", Some("wip/CFO1"));
    write_checks(root, "CFO1", &[]);

    let runs = vec![CombinedRun {
        id: "CFO1".to_string(),
        tip,
    }];
    let record = CombineRecord {
        request: "req-bad-cf".to_string(),
        state: CombineState::CheckFailed,
        branch: "blyte/req-bad-cf".to_string(),
        base,
        runs: runs.clone(),
        commits: Vec::new(),
        check: CombineCheck::Ran,
        conflicting_run: None,
        conflicting_files: Vec::new(),
        failing_step: Some("cargo test".to_string()),
        output_tail: Some(String::new()),
    };
    let rec_path = root.join(".wt/combined/req-bad-cf.json");
    std::fs::create_dir_all(rec_path.parent().unwrap()).unwrap();
    std::fs::write(&rec_path, serde_json::to_string_pretty(&record).unwrap()).unwrap();

    write_attempt_record(
        root,
        "req-bad-cf",
        &AttemptRecord {
            runs,
            result: AttemptResult::CheckFailed,
            error: None,
            reported: false,
        },
    )
    .unwrap();

    let (surface, prompts) = make_surface(root.to_path_buf(), Arc::new(RealPublisher));
    assert!(!maybe_combine(&surface, "req-bad-cf").unwrap());
    wait_for_idle_entries(&surface.session, 1);

    let captured = prompts.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let msg = &captured[0];
    assert!(msg.contains("output_tail"), "msg: {msg}");
    assert!(msg.contains(".wt/combined/req-bad-cf.json"), "msg: {msg}");

    let attempt = load_attempt_record(root, "req-bad-cf").unwrap().unwrap();
    assert_eq!(attempt.result, AttemptResult::Error);
    assert!(attempt.reported);
    let err = attempt.error.as_deref().unwrap_or("");
    assert!(err.contains("output_tail"), "err: {err}");
    assert!(err.contains(".wt/combined/req-bad-cf.json"), "err: {err}");
}

#[test]
fn a_combined_request_with_all_branches_deleted_is_done() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "DN1", &base, "dn1.txt", "one\n", "feat: dn1");
    write_start_file(root, "DN1", Some("req-done-all"), &base, now, false);
    write_summary(root, "DN1", "verified", Some("wip/DN1"));
    write_checks(root, "DN1", &[]);

    let (surface, _prompts) =
        make_surface(root.to_path_buf(), FakePublisher::ok("https://example/1"));
    assert!(maybe_combine(&surface, "req-done-all").unwrap());
    surface.requests.wait_for_idle("req-done-all");

    git(root, &["branch", "-D", "wip/DN1"]);

    let reqs = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 1));
    let arr = reqs.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["request"], "req-done-all");
    assert_eq!(arr[0]["state"], "done");
    assert!(arr[0]["error"].is_null());
    assert_eq!(arr[0]["branch"], "blyte/req-done-all");
    assert_eq!(arr[0]["base"], base);
    assert_eq!(arr[0]["check"], "ran");
    assert_eq!(arr[0]["commits"].as_array().unwrap().len(), 1);
    assert!(!maybe_combine(&surface, "req-done-all").unwrap());
}

#[test]
fn a_combined_request_with_one_of_two_branches_deleted_is_done() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "DP1", &base, "dp1.txt", "one\n", "feat: dp1");
    create_run_branch(root, "DP2", &base, "dp2.txt", "two\n", "feat: dp2");
    write_start_file(root, "DP1", Some("req-done-part"), &base, now, false);
    write_start_file(root, "DP2", Some("req-done-part"), &base, now + 1, false);
    write_summary(root, "DP1", "verified", Some("wip/DP1"));
    write_summary(root, "DP2", "verified", Some("wip/DP2"));
    write_checks(root, "DP1", &[]);
    write_checks(root, "DP2", &[]);

    let (surface, _prompts) =
        make_surface(root.to_path_buf(), FakePublisher::ok("https://example/2"));
    assert!(maybe_combine(&surface, "req-done-part").unwrap());
    surface.requests.wait_for_idle("req-done-part");

    git(root, &["branch", "-D", "wip/DP1"]);

    let reqs = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 2));
    let arr = reqs.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["request"], "req-done-part");
    assert_eq!(arr[0]["state"], "done");
    assert!(arr[0]["error"].is_null());
    assert_eq!(arr[0]["branch"], "blyte/req-done-part");
    assert!(!maybe_combine(&surface, "req-done-part").unwrap());
}

#[test]
fn a_combined_request_with_an_unlisted_runs_branch_deleted_is_an_error() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "UL1", &base, "ul1.txt", "one\n", "feat: ul1");
    write_start_file(root, "UL1", Some("req-unlisted"), &base, now, false);
    write_summary(root, "UL1", "verified", Some("wip/UL1"));
    write_checks(root, "UL1", &[]);

    let (surface, _prompts) =
        make_surface(root.to_path_buf(), FakePublisher::ok("https://example/3"));
    assert!(maybe_combine(&surface, "req-unlisted").unwrap());
    surface.requests.wait_for_idle("req-unlisted");

    create_run_branch(root, "UL2", &base, "ul2.txt", "two\n", "feat: ul2");
    write_start_file(root, "UL2", Some("req-unlisted"), &base, now + 1, false);
    write_summary(root, "UL2", "verified", Some("wip/UL2"));
    write_checks(root, "UL2", &[]);
    git(root, &["branch", "-D", "wip/UL2"]);

    let reqs = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 2));
    let arr = reqs.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["request"], "req-unlisted");
    assert_eq!(arr[0]["state"], "error");
    let err = arr[0]["error"].as_str().unwrap();
    assert!(err.contains("UL2"), "err: {err}");
    assert!(err.contains("wip/UL2"), "err: {err}");
}

#[test]
fn a_request_without_a_combine_record_and_a_deleted_branch_is_an_error() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "NR1", &base, "nr1.txt", "one\n", "feat: nr1");
    write_start_file(root, "NR1", Some("req-no-rec"), &base, now, false);
    write_summary(root, "NR1", "verified", Some("wip/NR1"));
    write_checks(root, "NR1", &[]);
    git(root, &["branch", "-D", "wip/NR1"]);

    let (surface, _prompts) =
        make_surface(root.to_path_buf(), FakePublisher::ok("https://example/4"));
    let reqs = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 1));
    let arr = reqs.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["request"], "req-no-rec");
    assert_eq!(arr[0]["state"], "error");
    let err = arr[0]["error"].as_str().unwrap();
    assert!(err.contains("NR1"), "err: {err}");
    assert!(err.contains("wip/NR1"), "err: {err}");
}

#[test]
fn a_combined_request_with_no_branch_deleted_stays_combined() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "KC1", &base, "kc1.txt", "one\n", "feat: kc1");
    write_start_file(root, "KC1", Some("req-keep-comb"), &base, now, false);
    write_summary(root, "KC1", "verified", Some("wip/KC1"));
    write_checks(root, "KC1", &[]);

    let (surface, _prompts) =
        make_surface(root.to_path_buf(), FakePublisher::ok("https://example/5"));
    assert!(maybe_combine(&surface, "req-keep-comb").unwrap());
    surface.requests.wait_for_idle("req-keep-comb");

    let reqs = parse_json(&route(&surface, &get(&surface, "/api/requests"), now + 1));
    let arr = reqs.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["request"], "req-keep-comb");
    assert_eq!(arr[0]["state"], "combined");
    assert!(arr[0]["error"].is_null());
}

#[test]
fn list_requests_returns_only_owned_requests() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "OAA", &base, "aa.txt", "aa\n", "feat: aa");
    create_run_branch(root, "OBB", &base, "bb.txt", "bb\n", "feat: bb");
    create_run_branch(root, "OUN", &base, "un.txt", "un\n", "feat: un");

    write_start_file_with_surface(
        root,
        "OAA",
        Some("req-aa"),
        "main",
        &base,
        now,
        false,
        Value::String("s00000000000000aa".to_string()),
    );
    write_start_file_with_surface(
        root,
        "OBB",
        Some("req-bb"),
        "main",
        &base,
        now + 1,
        false,
        Value::String("s00000000000000bb".to_string()),
    );
    write_start_file_with_surface(
        root,
        "OUN",
        Some("req-unowned"),
        "main",
        &base,
        now + 2,
        false,
        Value::Null,
    );
    for id in ["OAA", "OBB", "OUN"] {
        write_summary(root, id, "verified", Some(&format!("wip/{id}")));
        write_checks(root, id, &[]);
    }

    let (surface, _prompts) =
        make_surface(root.to_path_buf(), FakePublisher::ok("https://example/aa"));
    let reqs = list_requests(&surface).unwrap();
    let names: Vec<&str> = reqs.iter().map(|r| r.request.as_str()).collect();
    assert_eq!(names, vec!["req-aa"]);
}

#[test]
fn maybe_combine_skips_request_owned_by_another_surface() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "CBB", &base, "cbb.txt", "bb\n", "feat: bb");
    write_start_file_with_surface(
        root,
        "CBB",
        Some("req-bb-ready"),
        "main",
        &base,
        now,
        false,
        Value::String("s00000000000000bb".to_string()),
    );
    write_summary(root, "CBB", "verified", Some("wip/CBB"));
    write_checks(root, "CBB", &[]);

    let (surface, _prompts) =
        make_surface(root.to_path_buf(), FakePublisher::ok("https://example/bb"));
    assert!(!maybe_combine(&surface, "req-bb-ready").unwrap());
    surface.requests.wait_all_idle();
    assert!(!root.join(".wt/combined").exists());
}

#[test]
fn push_and_pr_refuse_other_surface_request_and_mixed_owner_request() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "PBB", &base, "pbb.txt", "bb\n", "feat: pbb");
    write_start_file_with_surface(
        root,
        "PBB",
        Some("req-bb-pub"),
        "main",
        &base,
        now,
        false,
        Value::String("s00000000000000bb".to_string()),
    );
    write_summary(root, "PBB", "verified", Some("wip/PBB"));
    write_checks(root, "PBB", &[]);

    let (mut surface_bb, _) =
        make_surface(root.to_path_buf(), FakePublisher::ok("https://example/bb"));
    surface_bb.surface_id = "s00000000000000bb".to_string();
    assert!(maybe_combine(&surface_bb, "req-bb-pub").unwrap());
    surface_bb.requests.wait_for_idle("req-bb-pub");

    let pub_aa = FakePublisher::ok("https://example/aa");
    let (surface_aa, _) = make_surface(
        root.to_path_buf(),
        Arc::clone(&pub_aa) as Arc<dyn Publisher>,
    );

    let push_err = push_request(&surface_aa, "req-bb-pub").unwrap_err();
    assert_eq!(
        push_err,
        RequestActionError::NotOwned("request req-bb-pub belongs to another surface".to_string())
    );
    assert!(pub_aa.push_calls.lock().unwrap().is_empty());

    let pr_err = open_request_pull_request(&surface_aa, "req-bb-pub", "Title", "Body").unwrap_err();
    assert_eq!(
        pr_err,
        RequestActionError::NotOwned("request req-bb-pub belongs to another surface".to_string())
    );
    assert!(pub_aa.pr_calls.lock().unwrap().is_empty());

    let push_http = route(
        &surface_aa,
        &post_json_with_token(
            "/api/requests/req-bb-pub/push",
            Some(&surface_aa.token),
            &json!({}),
        ),
        now + 1,
    );
    assert_eq!(push_http.status, 403);
    assert_eq!(
        parse_json(&push_http)["error"],
        "request req-bb-pub belongs to another surface"
    );

    let pr_http = route(
        &surface_aa,
        &post_json_with_token(
            "/api/requests/req-bb-pub/pull-request",
            Some(&surface_aa.token),
            &json!({ "title": "Title", "body": "Body" }),
        ),
        now + 1,
    );
    assert_eq!(pr_http.status, 403);
    assert_eq!(
        parse_json(&pr_http)["error"],
        "request req-bb-pub belongs to another surface"
    );

    create_run_branch(root, "MX1", &base, "mx1.txt", "1\n", "feat: mx1");
    create_run_branch(root, "MX2", &base, "mx2.txt", "2\n", "feat: mx2");
    write_start_file_with_surface(
        root,
        "MX1",
        Some("req-mixed"),
        "main",
        &base,
        now + 2,
        false,
        Value::String("s00000000000000aa".to_string()),
    );
    write_start_file_with_surface(
        root,
        "MX2",
        Some("req-mixed"),
        "main",
        &base,
        now + 3,
        false,
        Value::String("s00000000000000bb".to_string()),
    );

    let mixed_err = push_request(&surface_aa, "req-mixed").unwrap_err();
    match mixed_err {
        RequestActionError::BadRequest(msg) => {
            assert!(msg.contains("more than one surface"), "msg={msg}");
        }
        other => panic!("expected BadRequest, got {other:?}"),
    }
    assert!(pub_aa.push_calls.lock().unwrap().is_empty());
    assert!(pub_aa.pr_calls.lock().unwrap().is_empty());
}

#[test]
fn non_string_surface_in_start_record_returns_error_naming_file() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    write_start_file_with_surface(
        root,
        "BADSURF",
        Some("req-badsurf"),
        "main",
        &base,
        now,
        false,
        json!(5),
    );

    let (surface, _prompts) =
        make_surface(root.to_path_buf(), FakePublisher::ok("https://example/bad"));
    let err = list_requests(&surface).unwrap_err();
    assert!(err.contains("BADSURF.start.json"), "err={err}");
}

#[allow(clippy::too_many_arguments)]
fn write_start_file_with_started_by(
    root: &Path,
    id: &str,
    request: Option<&str>,
    base_sha: &str,
    started_at: u64,
    started_by: Option<Value>,
) {
    let mut obj = json!({
        "id": id,
        "base": "main",
        "base_sha": base_sha,
        "started_at": started_at,
        "research": false,
        "surface": "s00000000000000aa",
    });
    if let Some(req) = request {
        obj["request"] = Value::String(req.to_string());
    }
    if let Some(sb) = started_by {
        obj["started_by"] = sb;
    }
    std::fs::write(
        root.join(".wt").join(format!("{id}.start.json")),
        serde_json::to_string_pretty(&obj).unwrap() + "\n",
    )
    .unwrap();
}

#[test]
fn the_request_list_reports_started_by() {
    let (repo, base) = init_repo_with_gate("true");
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "SB1", &base, "sb1.txt", "1\n", "feat: sb1");
    create_run_branch(root, "SB2", &base, "sb2.txt", "2\n", "feat: sb2");
    write_start_file_with_started_by(
        root,
        "SB1",
        Some("req-from-api"),
        &base,
        now + 2,
        Some(Value::String("api-2".to_string())),
    );
    write_start_file_with_started_by(root, "SB2", Some("req-from-user"), &base, now + 1, None);
    for id in ["SB1", "SB2"] {
        write_summary(root, id, "verified", Some(&format!("wip/{id}")));
        write_checks(root, id, &[]);
    }

    let (surface, _prompts) =
        make_surface(root.to_path_buf(), FakePublisher::ok("https://example/sb"));
    let reqs = list_requests(&surface).unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].request, "req-from-api");
    assert_eq!(reqs[0].started_by.as_deref(), Some("api-2"));
    assert_eq!(reqs[1].request, "req-from-user");
    assert_eq!(reqs[1].started_by, None);

    let resp = route(&surface, &get(&surface, "/api/requests"), now + 3);
    assert_eq!(resp.status, 200);
    let body = parse_json(&resp);
    let list = body.as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["request"], "req-from-api");
    assert_eq!(list[0]["started_by"], "api-2");
    assert_eq!(list[1]["request"], "req-from-user");
    assert!(
        list[1].get("started_by").is_some_and(|v| v.is_null()),
        "expected started_by: null in {}",
        list[1]
    );
}

#[test]
fn a_request_with_mixed_started_by_is_an_error() {
    let (repo1, base1) = init_repo_with_gate("true");
    let root1 = repo1.path();
    let now = now_secs();

    create_run_branch(root1, "MS1", &base1, "ms1.txt", "1\n", "feat: ms1");
    create_run_branch(root1, "MS2", &base1, "ms2.txt", "2\n", "feat: ms2");
    write_start_file_with_started_by(
        root1,
        "MS1",
        Some("req-mix-none"),
        &base1,
        now,
        Some(Value::String("api".to_string())),
    );
    write_start_file_with_started_by(root1, "MS2", Some("req-mix-none"), &base1, now + 1, None);

    let (surface1, _) = make_surface(
        root1.to_path_buf(),
        FakePublisher::ok("https://example/mix1"),
    );
    let list_err1 = list_requests(&surface1).unwrap_err();
    assert_eq!(
        list_err1,
        "request req-mix-none has runs started by different surfaces"
    );
    let push_err1 = push_request(&surface1, "req-mix-none").unwrap_err();
    assert_eq!(
        push_err1,
        RequestActionError::BadRequest(
            "request req-mix-none has runs started by different surfaces".to_string()
        )
    );
    let combine_err1 = maybe_combine(&surface1, "req-mix-none").unwrap_err();
    assert_eq!(
        combine_err1,
        "request req-mix-none has runs started by different surfaces"
    );

    let (repo2, base2) = init_repo_with_gate("true");
    let root2 = repo2.path();
    create_run_branch(root2, "MD1", &base2, "md1.txt", "1\n", "feat: md1");
    create_run_branch(root2, "MD2", &base2, "md2.txt", "2\n", "feat: md2");
    write_start_file_with_started_by(
        root2,
        "MD1",
        Some("req-mix-names"),
        &base2,
        now,
        Some(Value::String("api".to_string())),
    );
    write_start_file_with_started_by(
        root2,
        "MD2",
        Some("req-mix-names"),
        &base2,
        now + 1,
        Some(Value::String("web".to_string())),
    );

    let (surface2, _) = make_surface(
        root2.to_path_buf(),
        FakePublisher::ok("https://example/mix2"),
    );
    let list_err2 = list_requests(&surface2).unwrap_err();
    assert_eq!(
        list_err2,
        "request req-mix-names has runs started by different surfaces"
    );
    let push_err2 = push_request(&surface2, "req-mix-names").unwrap_err();
    assert_eq!(
        push_err2,
        RequestActionError::BadRequest(
            "request req-mix-names has runs started by different surfaces".to_string()
        )
    );
}

#[test]
fn a_start_record_with_a_bad_started_by_is_an_error() {
    let now = now_secs();

    for (id, bad_val) in [("BADNUM", json!(3)), ("BADEMPTY", json!(""))] {
        let (repo, base) = init_repo_with_gate("true");
        let root = repo.path();
        write_start_file_with_started_by(root, id, Some("req-badsb"), &base, now, Some(bad_val));
        let start_path = root.join(".wt").join(format!("{id}.start.json"));
        let (surface, _) =
            make_surface(root.to_path_buf(), FakePublisher::ok("https://example/bad"));
        let err = list_requests(&surface).unwrap_err();
        assert_eq!(
            err,
            format!(
                "{}: started_by must be a surface name",
                start_path.display()
            )
        );
    }
}
