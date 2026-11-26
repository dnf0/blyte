use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use blyte_agent::approval::{Approval, ApprovalConfig, Approver, PublishDetails};
use blyte_agent::fs::LocalFs;
use blyte_agent::shell::{Output, Shell};
use blyte_agent::tools::{execute, set_approval, set_publish_tool, PublishTool};
use blyte_cli::combine::{CombineCheck, CombineRecord, CombineState};
use blyte_cli::http::{Request, Response};
use blyte_cli::requests::{
    load_published_record, maybe_combine, Publisher, RequestsState, SurfacePublishTool,
};
use blyte_cli::serve::{route, Surface, MESSAGE_HEADER, TOKEN_HEADER};
use blyte_cli::session::{Entry, EntryBody, PromptRunner, Session};
use blyte_cli::tty_approver::TtyApprover;
use serde_json::json;

static GLOBAL_TOOL_LOCK: Mutex<()> = Mutex::new(());

struct ToolLockGuard {
    _lock: MutexGuard<'static, ()>,
}

impl ToolLockGuard {
    fn acquire() -> Self {
        let lock = GLOBAL_TOOL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_publish_tool(None);
        set_approval(None);
        Self { _lock: lock }
    }
}

impl Drop for ToolLockGuard {
    fn drop(&mut self) {
        set_publish_tool(None);
        set_approval(None);
    }
}

struct UnusedShell;

impl Shell for UnusedShell {
    fn exec(&self, _: &str, _: &Path, _: Duration) -> Result<Output, String> {
        panic!("shell must not be called by publish");
    }

    fn describe(&self) -> String {
        String::new()
    }
}

struct NoopRunner;

impl PromptRunner for NoopRunner {
    fn run(
        &mut self,
        _prompt: &str,
        _model: &str,
        _emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
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

    fn set_pr_error(&self, msg: &str) {
        *self.pr_result.lock().unwrap() = Err(msg.to_string());
    }

    fn set_pr_ok(&self, url: &str) {
        *self.pr_result.lock().unwrap() = Ok(url.to_string());
    }

    fn push_calls(&self) -> Vec<PushCall> {
        self.push_calls.lock().unwrap().clone()
    }

    fn pr_calls(&self) -> Vec<PrCall> {
        self.pr_calls.lock().unwrap().clone()
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

fn init_repo_with_origin(gate: &str) -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    let cfg_path = root.join(".git/config");
    let mut cfg = std::fs::read_to_string(&cfg_path).unwrap();
    cfg.push_str("\n[user]\n\temail = test@example.com\n\tname = Test User\n");
    std::fs::write(&cfg_path, cfg).unwrap();
    let toml = format!("[dispatch]\ngate = {gate:?}\n");
    let base_sha = fast_import_commit(
        root,
        "main",
        None,
        &[("README.md", "hello\n"), ("blyte.toml", &toml)],
        "initial commit",
    );
    git(root, &["reset", "--hard", "main"]);
    std::fs::create_dir_all(root.join(".wt")).unwrap();

    let remote_url = "https://github.com/example/blyte-test.git".to_string();
    git(root, &["remote", "add", "origin", &remote_url]);
    git(root, &["update-ref", "refs/remotes/origin/main", &base_sha]);
    git(
        root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    (dir, base_sha, remote_url)
}

#[allow(clippy::too_many_arguments)]
fn write_completed_run(
    root: &Path,
    run_id: &str,
    request: &str,
    surface_id: Option<&str>,
    base_sha: &str,
    file: &str,
    content: &str,
    subject: &str,
) {
    let _ = fast_import_commit(
        root,
        &format!("wip/{run_id}"),
        Some(base_sha),
        &[(file, content)],
        subject,
    );
    let wt_dir = root.join(".wt");
    let mut start = json!({
        "id": run_id,
        "request": request,
        "started_at": 1700000000u64,
        "base": "main",
        "base_sha": base_sha,
        "research": false,
    });
    if let Some(sid) = surface_id {
        start["surface"] = json!(sid);
    }
    std::fs::write(
        wt_dir.join(format!("{run_id}.start.json")),
        serde_json::to_string_pretty(&start).unwrap(),
    )
    .unwrap();
    let summary = json!({
        "id": run_id,
        "branch": format!("wip/{run_id}"),
        "outcome": "verified",
        "seconds": 5,
    });
    std::fs::write(
        wt_dir.join(format!("{run_id}.json")),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .unwrap();
    std::fs::write(
        wt_dir.join(format!("{run_id}.report.json")),
        "{\"turns\":1}\n",
    )
    .unwrap();
    std::fs::write(
        wt_dir.join(format!("{run_id}.checks.json")),
        "{\"flags\":[],\"fallbacks\":[]}\n",
    )
    .unwrap();
}

fn make_surface(
    root: &Path,
    surface_id: &str,
    publisher: Arc<dyn Publisher>,
    session: Session,
) -> Arc<Surface> {
    Arc::new(Surface {
        surface_id: surface_id.to_string(),
        surface_name: "main-surface".to_string(),
        root: root.to_path_buf(),
        repo: "example/blyte-test".to_string(),
        session,
        session_id: "sess-1".to_string(),
        started_at: 1700000000,
        days: 7,
        token: "surface-secret-token".to_string(),
        message_credential: "peer-msg-credential".to_string(),
        requests: Arc::new(RequestsState::new()),
        publisher,
        reconcile_lock: Mutex::new(()),
        reconcile_requested: AtomicBool::new(false),
    })
}

fn wait_for_publish_approval(session: &Session) -> (String, PublishDetails) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let entries = session.entries_after(0, Duration::ZERO);
        for entry in entries {
            if let EntryBody::PublishApprovalRequested { id, publish, .. } = entry.body {
                return (id, publish);
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for PublishApprovalRequested entry"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_nth_publish_approval(session: &Session, n: usize) -> (String, PublishDetails) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let matches: Vec<(String, PublishDetails)> = session
            .entries_after(0, Duration::ZERO)
            .into_iter()
            .filter_map(|e| match e.body {
                EntryBody::PublishApprovalRequested { id, publish, .. } => Some((id, publish)),
                _ => None,
            })
            .collect();
        if matches.len() >= n {
            return matches[n - 1].clone();
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for publish approval #{n}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn post_approval(surface: &Surface, id: &str, answer: &str, header: (&str, &str)) -> Response {
    let req = Request {
        method: "POST".to_string(),
        path: format!("/api/session/approvals/{id}"),
        query: Vec::new(),
        headers: vec![(header.0.to_ascii_lowercase(), header.1.to_string())],
        body: serde_json::to_vec(&json!({ "answer": answer })).unwrap(),
    };
    route(surface, &req, 1700000100)
}

#[test]
fn tty_approver_returns_unavailable_for_ask_publish() {
    let details = PublishDetails {
        request: "req-1".to_string(),
        remote_url: "https://github.com/example/blyte-test.git".to_string(),
        branch: "blyte/req-1".to_string(),
        base: "main".to_string(),
        commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        commits: vec!["Add feature".to_string()],
        title: "Add feature".to_string(),
        body: "Body".to_string(),
    };
    assert_eq!(TtyApprover.ask_publish(&details), Approval::Unavailable);
}

#[test]
fn approval_requested_wire_format_reads_with_and_without_publish() {
    let plain = Entry {
        seq: 1,
        ts: 1700000000,
        body: EntryBody::ApprovalRequested {
            id: "approval-1".to_string(),
            command: "npm install".to_string(),
            reason: "install deps".to_string(),
        },
    };
    let plain_json = serde_json::to_string(&plain).unwrap();
    let plain_val: serde_json::Value = serde_json::from_str(&plain_json).unwrap();
    assert_eq!(plain_val["kind"], "approval_requested");
    assert!(plain_val.get("publish").is_none());
    let plain_roundtrip: Entry = serde_json::from_str(&plain_json).unwrap();
    assert_eq!(plain_roundtrip, plain);

    let details = PublishDetails {
        request: "req-1".to_string(),
        remote_url: "https://github.com/example/blyte-test.git".to_string(),
        branch: "blyte/req-1".to_string(),
        base: "main".to_string(),
        commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        commits: vec!["First commit".to_string(), "Second commit".to_string()],
        title: "Ship feature".to_string(),
        body: "Full description".to_string(),
    };
    let pub_entry = Entry {
        seq: 2,
        ts: 1700000001,
        body: EntryBody::PublishApprovalRequested {
            id: "approval-2".to_string(),
            command: "publish req-1".to_string(),
            reason: "Ship feature".to_string(),
            publish: details,
        },
    };
    let pub_json = serde_json::to_string(&pub_entry).unwrap();
    let pub_val: serde_json::Value = serde_json::from_str(&pub_json).unwrap();
    assert_eq!(pub_val["kind"], "approval_requested");
    assert_eq!(pub_val["publish"]["request"], "req-1");
    assert_eq!(pub_val["publish"]["branch"], "blyte/req-1");
    let pub_roundtrip: Entry = serde_json::from_str(&pub_json).unwrap();
    assert_eq!(pub_roundtrip, pub_entry);
}

#[test]
fn publish_tool_refuses_unknown_other_surface_unready_combining_and_duplicate_pending() {
    let _guard = ToolLockGuard::acquire();
    let (dir, base_sha, _) = init_repo_with_origin("true");
    let root = dir.path();

    let publisher = FakePublisher::ok("https://github.com/example/blyte-test/pull/1");
    let session = Session::start(Box::new(NoopRunner), "test-model".to_string());
    let surface = make_surface(root, "surf-a", publisher.clone(), session);
    let tool = SurfacePublishTool::new(Arc::clone(&surface));

    let err_unknown = tool.publish("req-missing", "Title", "Body").unwrap_err();
    assert!(err_unknown.contains("unknown"), "{err_unknown}");

    write_completed_run(
        root,
        "R-other",
        "req-other",
        Some("surf-b"),
        &base_sha,
        "other.txt",
        "other\n",
        "Other change",
    );
    let err_other = tool.publish("req-other", "Title", "Body").unwrap_err();
    assert!(err_other.contains("another surface"), "{err_other}");

    write_completed_run(
        root,
        "R-wait",
        "req-wait",
        Some("surf-a"),
        &base_sha,
        "wait.txt",
        "wait\n",
        "Waiting change",
    );
    let err_waiting = tool.publish("req-wait", "Title", "Body").unwrap_err();
    assert!(err_waiting.contains("waiting"), "{err_waiting}");

    write_completed_run(
        root,
        "R-bad",
        "req-failed",
        Some("surf-a"),
        &base_sha,
        "bad.txt",
        "bad\n",
        "Failed check change",
    );
    let failed_record = CombineRecord {
        request: "req-failed".to_string(),
        branch: "blyte/req-failed".to_string(),
        base: base_sha.clone(),
        runs: vec![],
        commits: vec![],
        state: CombineState::CheckFailed,
        check: CombineCheck::Ran,
        conflicting_run: None,
        conflicting_files: vec![],
        failing_step: Some("gate".to_string()),
        output_tail: Some("failed".to_string()),
    };
    std::fs::create_dir_all(root.join(".wt/combined")).unwrap();
    std::fs::write(
        root.join(".wt/combined/req-failed.json"),
        serde_json::to_string_pretty(&failed_record).unwrap(),
    )
    .unwrap();
    let err_failed = tool.publish("req-failed", "Title", "Body").unwrap_err();
    assert!(err_failed.contains("not ready to publish"), "{err_failed}");

    assert!(surface.session.entries_after(0, Duration::ZERO).is_empty());
    assert!(publisher.push_calls().is_empty());
    assert!(publisher.pr_calls().is_empty());

    let started_file = root.join(".wt/combine-started");
    let release_file = root.join(".wt/combine-release");
    let gate_cmd = format!(
        "touch {} && while [ ! -f {} ]; do sleep 0.02; done",
        started_file.display(),
        release_file.display()
    );
    let toml = format!("[dispatch]\ngate = {gate_cmd:?}\n");
    std::fs::write(root.join("blyte.toml"), &toml).unwrap();
    write_completed_run(
        root,
        "R-comb",
        "req-comb",
        Some("surf-a"),
        &base_sha,
        "comb.txt",
        "comb\n",
        "Combining change",
    );
    assert!(maybe_combine(&surface, "req-comb").unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started_file.exists() {
        assert!(Instant::now() < deadline, "combine gate never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    let err_combining = tool.publish("req-comb", "Title", "Body").unwrap_err();
    assert!(err_combining.contains("combining"), "{err_combining}");
    std::fs::write(&release_file, "go").unwrap();
    surface.requests.wait_for_idle("req-comb");

    let surface_for_thread = Arc::clone(&surface);
    let handle = std::thread::spawn(move || {
        let t = SurfacePublishTool::new(surface_for_thread);
        t.publish("req-comb", "First title", "First body")
    });
    let (approval_id, _) = wait_for_publish_approval(&surface.session);
    let err_dup = tool
        .publish("req-comb", "Second title", "Second body")
        .unwrap_err();
    assert!(err_dup.contains("already waiting"), "{err_dup}");

    let resp = post_approval(&surface, &approval_id, "no", (TOKEN_HEADER, &surface.token));
    assert_eq!(resp.status, 200);
    assert_eq!(handle.join().unwrap().unwrap_err(), "the user declined");
}

#[test]
fn publish_yes_no_moved_branch_and_retry_after_pr_failure() {
    let _guard = ToolLockGuard::acquire();
    let (dir, base_sha, remote_url) = init_repo_with_origin("true");
    let root = dir.path();

    write_completed_run(
        root,
        "R1",
        "req-pub",
        Some("surf-a"),
        &base_sha,
        "feat.txt",
        "feature\n",
        "Add feature commit",
    );

    let publisher = FakePublisher::ok("https://github.com/example/blyte-test/pull/42");
    let session = Session::start(Box::new(NoopRunner), "test-model".to_string());
    let surface = make_surface(root, "surf-a", publisher.clone(), session);
    assert!(maybe_combine(&surface, "req-pub").unwrap());
    surface.requests.wait_for_idle("req-pub");

    let expected_sha = git(root, &["rev-parse", "refs/heads/blyte/req-pub"]);

    let surface_no = Arc::clone(&surface);
    let h_no = std::thread::spawn(move || {
        let t = SurfacePublishTool::new(surface_no);
        t.publish("req-pub", "Add feature", "Detailed body")
    });
    let (id1, details1) = wait_for_nth_publish_approval(&surface.session, 1);
    assert_eq!(details1.request, "req-pub");
    assert_eq!(details1.remote_url, remote_url);
    assert_eq!(details1.branch, "blyte/req-pub");
    assert_eq!(details1.base, "main");
    assert_eq!(details1.commit, expected_sha);
    assert_eq!(details1.commits, vec!["Add feature commit".to_string()]);
    assert_eq!(details1.title, "Add feature");
    assert_eq!(details1.body, "Detailed body");

    let denied_by_msg_cred = post_approval(
        &surface,
        &id1,
        "yes",
        (MESSAGE_HEADER, &surface.message_credential),
    );
    assert_eq!(denied_by_msg_cred.status, 403);
    let denied_by_msg_cred_as_token = post_approval(
        &surface,
        &id1,
        "yes",
        (TOKEN_HEADER, &surface.message_credential),
    );
    assert_eq!(denied_by_msg_cred_as_token.status, 403);

    let resp_no = post_approval(&surface, &id1, "no", (TOKEN_HEADER, &surface.token));
    assert_eq!(resp_no.status, 200);
    assert_eq!(h_no.join().unwrap().unwrap_err(), "the user declined");
    assert!(publisher.push_calls().is_empty());
    assert!(publisher.pr_calls().is_empty());

    let surface_moved = Arc::clone(&surface);
    let h_moved = std::thread::spawn(move || {
        let t = SurfacePublishTool::new(surface_moved);
        t.publish("req-pub", "Add feature", "Detailed body")
    });
    let (id2, _) = wait_for_nth_publish_approval(&surface.session, 2);
    let moved_sha = fast_import_commit(
        root,
        "blyte/req-pub",
        Some(&expected_sha),
        &[("extra.txt", "extra\n")],
        "Unexpected commit after approval",
    );
    let resp_moved = post_approval(&surface, &id2, "yes", (TOKEN_HEADER, &surface.token));
    assert_eq!(resp_moved.status, 200);
    let err_moved = h_moved.join().unwrap().unwrap_err();
    assert!(err_moved.contains("moved after approval"), "{err_moved}");
    assert!(publisher.push_calls().is_empty());
    assert!(publisher.pr_calls().is_empty());
    git(
        root,
        &["update-ref", "refs/heads/blyte/req-pub", &expected_sha],
    );
    assert_ne!(moved_sha, expected_sha);

    publisher.set_pr_error("gh returned 500");
    let surface_pr_fail = Arc::clone(&surface);
    let h_pr_fail = std::thread::spawn(move || {
        let t = SurfacePublishTool::new(surface_pr_fail);
        t.publish("req-pub", "Add feature", "Detailed body")
    });
    let (id3, _) = wait_for_nth_publish_approval(&surface.session, 3);
    let resp_pr_fail = post_approval(&surface, &id3, "yes", (TOKEN_HEADER, &surface.token));
    assert_eq!(resp_pr_fail.status, 200);
    let err_pr_fail = h_pr_fail.join().unwrap().unwrap_err();
    assert!(err_pr_fail.contains("push succeeded"), "{err_pr_fail}");
    assert!(err_pr_fail.contains("pull request failed"), "{err_pr_fail}");
    assert_eq!(publisher.push_calls().len(), 1);
    assert_eq!(publisher.push_calls()[0].branch, "blyte/req-pub");
    assert_eq!(publisher.pr_calls().len(), 1);
    let pub_rec_after_push = load_published_record(root, "req-pub").unwrap();
    assert_eq!(
        pub_rec_after_push.pushed_commit.as_deref(),
        Some(expected_sha.as_str())
    );
    assert_eq!(pub_rec_after_push.pull_request_url, None);

    publisher.set_pr_ok("https://github.com/example/blyte-test/pull/42");
    let surface_retry = Arc::clone(&surface);
    let h_retry = std::thread::spawn(move || {
        let t = SurfacePublishTool::new(surface_retry);
        t.publish("req-pub", "Add feature", "Detailed body")
    });
    let (id4, _) = wait_for_nth_publish_approval(&surface.session, 4);
    let resp_retry = post_approval(&surface, &id4, "yes", (TOKEN_HEADER, &surface.token));
    assert_eq!(resp_retry.status, 200);
    let pr_url = h_retry.join().unwrap().unwrap();
    assert_eq!(pr_url, "https://github.com/example/blyte-test/pull/42");
    assert_eq!(publisher.push_calls().len(), 1);
    assert_eq!(publisher.pr_calls().len(), 2);
    assert_eq!(publisher.pr_calls()[1].branch, "blyte/req-pub");
    assert_eq!(publisher.pr_calls()[1].title, "Add feature");
    assert_eq!(publisher.pr_calls()[1].body, "Detailed body");

    let pub_rec = load_published_record(root, "req-pub").unwrap();
    assert_eq!(
        pub_rec.pushed_commit.as_deref(),
        Some(expected_sha.as_str())
    );
    assert_eq!(
        pub_rec.pull_request_url.as_deref(),
        Some("https://github.com/example/blyte-test/pull/42")
    );
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
                && note.as_deref() == Some("https://github.com/example/blyte-test/pull/42")
    )));
}

#[test]
fn allow_unsandboxed_git_rule_does_not_skip_publish_approval() {
    let _guard = ToolLockGuard::acquire();
    let (dir, base_sha, _) = init_repo_with_origin("true");
    let root = dir.path();
    write_completed_run(
        root,
        "R1",
        "req-rule",
        Some("surf-a"),
        &base_sha,
        "rule.txt",
        "rule\n",
        "Rule test commit",
    );

    let publisher = FakePublisher::ok("https://github.com/example/blyte-test/pull/99");
    let session = Session::start(Box::new(NoopRunner), "test-model".to_string());
    let surface = make_surface(root, "surf-a", publisher.clone(), session);
    assert!(maybe_combine(&surface, "req-rule").unwrap());
    surface.requests.wait_for_idle("req-rule");

    set_approval(Some(ApprovalConfig {
        approver: surface.session.approver(),
        rules: vec!["git".to_string(), "gh".to_string()],
    }));
    set_publish_tool(Some(Arc::new(SurfacePublishTool::new(Arc::clone(
        &surface,
    )))));

    let root_for_thread = root.to_path_buf();
    let handle = std::thread::spawn(move || {
        execute(
            &root_for_thread,
            &UnusedShell,
            &LocalFs,
            "publish",
            &json!({
                "request": "req-rule",
                "title": "Rule test",
                "body": "Must still ask"
            }),
        )
    });

    let (approval_id, details) = wait_for_publish_approval(&surface.session);
    assert_eq!(details.request, "req-rule");
    assert!(publisher.push_calls().is_empty());

    let resp = post_approval(
        &surface,
        &approval_id,
        "yes",
        (TOKEN_HEADER, &surface.token),
    );
    assert_eq!(resp.status, 200);

    let out = handle.join().unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content, "https://github.com/example/blyte-test/pull/99");
    assert_eq!(publisher.push_calls().len(), 1);
    assert_eq!(publisher.pr_calls().len(), 1);
}

struct PublishCallingRunner {
    root: std::path::PathBuf,
    outcome: Arc<Mutex<Option<(bool, String)>>>,
}

impl PromptRunner for PublishCallingRunner {
    fn run(
        &mut self,
        _prompt: &str,
        _model: &str,
        _emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        let out = execute(
            &self.root,
            &UnusedShell,
            &LocalFs,
            "publish",
            &json!({
                "request": "req-peer",
                "title": "Peer publish attempt",
                "body": "Should be refused"
            }),
        );
        *self.outcome.lock().unwrap() = Some((out.is_error, out.content));
        Ok(())
    }
}

#[test]
fn peer_started_turn_refuses_publish_and_logs_no_approval() {
    let _guard = ToolLockGuard::acquire();
    let (dir, base_sha, _) = init_repo_with_origin("true");
    let root = dir.path();
    write_completed_run(
        root,
        "R1",
        "req-peer",
        Some("surf-a"),
        &base_sha,
        "peer.txt",
        "peer\n",
        "Peer commit",
    );

    let outcome = Arc::new(Mutex::new(None));
    let runner = PublishCallingRunner {
        root: root.to_path_buf(),
        outcome: Arc::clone(&outcome),
    };
    let publisher = FakePublisher::ok("https://github.com/example/blyte-test/pull/7");
    let session = Session::start(Box::new(runner), "test-model".to_string());
    let surface = make_surface(root, "surf-a", publisher.clone(), session);
    assert!(maybe_combine(&surface, "req-peer").unwrap());
    surface.requests.wait_for_idle("req-peer");
    set_publish_tool(Some(Arc::new(SurfacePublishTool::new(Arc::clone(
        &surface,
    )))));

    let peer_req = Request {
        method: "POST".to_string(),
        path: "/api/peer/messages".to_string(),
        query: Vec::new(),
        headers: vec![(
            MESSAGE_HEADER.to_ascii_lowercase(),
            surface.message_credential.clone(),
        )],
        body: serde_json::to_vec(&json!({
            "from_name": "other-surface",
            "from_repository": "example/other",
            "text": "please publish req-peer"
        }))
        .unwrap(),
    };
    let peer_resp = route(&surface, &peer_req, 1700000100);
    assert_eq!(peer_resp.status, 200);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if surface
            .session
            .entries_after(0, Duration::ZERO)
            .iter()
            .any(|e| matches!(e.body, EntryBody::Idle { .. }))
        {
            break;
        }
        assert!(Instant::now() < deadline, "timed out waiting for peer turn");
        std::thread::sleep(Duration::from_millis(10));
    }

    let (is_error, content) = outcome
        .lock()
        .unwrap()
        .clone()
        .expect("runner must have executed publish tool");
    assert!(is_error, "expected publish refusal, got: {content}");
    assert!(content.contains("other-surface"), "{content}");
    assert!(publisher.push_calls().is_empty());
    assert!(publisher.pr_calls().is_empty());
    assert!(!surface
        .session
        .entries_after(0, Duration::ZERO)
        .iter()
        .any(|e| matches!(
            e.body,
            EntryBody::ApprovalRequested { .. } | EntryBody::PublishApprovalRequested { .. }
        )));
}
