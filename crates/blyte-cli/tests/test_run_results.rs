use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use blyte_cli::http::Request;
use blyte_cli::requests::{RealPublisher, RequestsState};
use blyte_cli::run_results::report_finished_runs;
use blyte_cli::serve::{generate_token, route, Surface};
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

fn init_repo() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "Test User"]);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    std::fs::write(root.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n").unwrap();
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
) {
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

fn write_summary(root: &Path, id: &str, value: &Value) {
    std::fs::write(
        root.join(".wt").join(format!("{id}.json")),
        serde_json::to_string_pretty(value).unwrap() + "\n",
    )
    .unwrap();
}

fn make_surface(root: PathBuf) -> (Surface, Arc<Mutex<Vec<String>>>) {
    let (runner, prompts) = RecordingRunner::new();
    let session = Session::start(Box::new(runner), "test-model".to_string());
    let repo = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string();
    let surface = Surface {
        root,
        repo,
        session,
        session_id: "sess-run-results-test".to_string(),
        started_at: 1_700_000_000,
        days: 14,
        token: generate_token().unwrap(),
        requests: Arc::new(RequestsState::new()),
        publisher: Arc::new(RealPublisher),
    };
    (surface, prompts)
}

fn wait_for_prompts(prompts: &Arc<Mutex<Vec<String>>>, expected: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let current = prompts.lock().unwrap().clone();
        if current.len() >= expected {
            return current;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    prompts.lock().unwrap().clone()
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

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn verified_run_adds_notice_once_writes_marker_and_queues_nothing() {
    let (repo, base) = init_repo();
    let root = repo.path();
    let now = now_secs();

    write_start_file(root, "R1", Some("req-1"), &base, now, false);
    write_summary(
        root,
        "R1",
        &json!({
            "id": "R1",
            "outcome": "verified",
            "turns": 7,
            "cost_usd": 0.426,
            "seconds": 150,
            "final_message": "Implemented feature X and added unit tests.",
            "gate_failures": []
        }),
    );

    let (surface, prompts) = make_surface(root.to_path_buf());
    report_finished_runs(&surface).unwrap();

    let entries = surface.session.entries_after(0, Duration::ZERO);
    let decisions: Vec<_> = entries
        .iter()
        .filter_map(|e| match &e.body {
            EntryBody::Decision { run, action, note } => Some((run, action, note)),
            _ => None,
        })
        .collect();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].0, "R1");
    let note = decisions[0].2.as_deref().unwrap();
    let mut lines = note.lines();
    assert_eq!(
        lines.next().unwrap(),
        "Run R1: verified, 7 turns, USD 0.43, 2.5 min"
    );
    assert_eq!(
        lines.next().unwrap(),
        "Implemented feature X and added unit tests."
    );

    let marker_path = root.join(".wt/R1.reported.json");
    assert!(marker_path.exists());
    let marker: Value =
        serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    assert_eq!(marker["outcome"], "verified");
    assert!(marker["ts"].as_u64().is_some());

    std::thread::sleep(Duration::from_millis(100));
    assert!(prompts.lock().unwrap().is_empty());

    let seq_after_first = entries.last().unwrap().seq;
    report_finished_runs(&surface).unwrap();
    let extra = surface
        .session
        .entries_after(seq_after_first, Duration::ZERO);
    assert!(extra.is_empty());
}

#[test]
fn gate_failed_run_adds_notice_and_queues_one_message_with_last_20_failure_lines() {
    let (repo, base) = init_repo();
    let root = repo.path();
    let now = now_secs();

    let failure_lines: Vec<String> = (1..=25).map(|i| format!("failure line {i}")).collect();
    write_start_file(root, "F1", Some("req-fail"), &base, now, false);
    write_summary(
        root,
        "F1",
        &json!({
            "id": "F1",
            "outcome": "gate_failed",
            "turns": 4,
            "cost_usd": null,
            "seconds": 90,
            "final_message": "Tried fixing the test but clippy failed.",
            "gate_failures": [failure_lines.join("\n")]
        }),
    );

    let (surface, prompts) = make_surface(root.to_path_buf());
    report_finished_runs(&surface).unwrap();

    let entries = surface.session.entries_after(0, Duration::ZERO);
    let decisions: Vec<_> = entries
        .iter()
        .filter_map(|e| match &e.body {
            EntryBody::Decision { run, note, .. } => Some((run, note)),
            _ => None,
        })
        .collect();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].0, "F1");
    let note = decisions[0].1.as_deref().unwrap();
    assert_eq!(
        note.lines().next().unwrap(),
        "Run F1: gate_failed, 4 turns, 1.5 min"
    );
    assert!(note.contains("Tried fixing the test but clippy failed."));

    let queued = wait_for_prompts(&prompts, 1);
    assert_eq!(queued.len(), 1);
    let msg = &queued[0];
    assert!(msg.starts_with("Run F1 did not pass: gate_failed.\n"));
    assert!(!msg.contains("failure line 5\n"));
    assert!(msg.contains("failure line 6\n"));
    assert!(msg.contains("failure line 25\n"));
    assert!(msg.ends_with("Read blyte review F1 and tell the user, or dispatch it again."));

    let marker_path = root.join(".wt/F1.reported.json");
    assert!(marker_path.exists());
    let marker: Value =
        serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    assert_eq!(marker["outcome"], "gate_failed");

    let count_before = surface.session.entries_after(0, Duration::ZERO).len();
    report_finished_runs(&surface).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(prompts.lock().unwrap().len(), 1);
    let count_after = surface.session.entries_after(0, Duration::ZERO).len();
    assert_eq!(count_after, count_before);
}

#[test]
fn ignores_runs_without_request_research_runs_and_unfinished_runs() {
    let (repo, base) = init_repo();
    let root = repo.path();
    let now = now_secs();

    write_start_file(root, "NOREQ", None, &base, now, false);
    write_summary(
        root,
        "NOREQ",
        &json!({
            "id": "NOREQ",
            "outcome": "verified",
            "turns": 3,
            "cost_usd": 0.10,
            "seconds": 60,
            "final_message": "no request"
        }),
    );

    write_start_file(root, "RES1", Some("req-res"), &base, now + 1, true);
    write_summary(
        root,
        "RES1",
        &json!({
            "id": "RES1",
            "outcome": "verified",
            "turns": 2,
            "cost_usd": 0.05,
            "seconds": 30,
            "final_message": "research report"
        }),
    );

    write_start_file(root, "UNFIN", Some("req-unfin"), &base, now + 2, false);

    let (surface, prompts) = make_surface(root.to_path_buf());
    report_finished_runs(&surface).unwrap();

    assert!(surface.session.entries_after(0, Duration::ZERO).is_empty());
    std::thread::sleep(Duration::from_millis(100));
    assert!(prompts.lock().unwrap().is_empty());
    assert!(!root.join(".wt/NOREQ.reported.json").exists());
    assert!(!root.join(".wt/RES1.reported.json").exists());
    assert!(!root.join(".wt/UNFIN.reported.json").exists());
}

#[test]
fn truncates_5000_char_final_message_at_4000_char_boundary() {
    let (repo, base) = init_repo();
    let root = repo.path();
    let now = now_secs();

    let long_message: String = std::iter::repeat_n('é', 5000).collect();
    write_start_file(root, "LONG1", Some("req-long"), &base, now, false);
    write_summary(
        root,
        "LONG1",
        &json!({
            "id": "LONG1",
            "outcome": "verified",
            "turns": 10,
            "cost_usd": 1.25,
            "seconds": 300,
            "final_message": long_message
        }),
    );

    let (surface, _prompts) = make_surface(root.to_path_buf());
    report_finished_runs(&surface).unwrap();

    let entries = surface.session.entries_after(0, Duration::ZERO);
    assert_eq!(entries.len(), 1);
    let EntryBody::Decision {
        note: Some(note), ..
    } = &entries[0].body
    else {
        panic!("expected Decision entry, got {:?}", entries[0].body);
    };
    let lines: Vec<&str> = note.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "Run LONG1: verified, 10 turns, USD 1.25, 5.0 min");
    assert_eq!(lines[1].chars().count(), 4000);
    assert_eq!(lines[2], "… (the rest: blyte review LONG1)");
}

#[test]
fn unparseable_or_incomplete_summary_returns_err_naming_file_and_writes_no_marker() {
    let (repo, base) = init_repo();
    let root = repo.path();
    let now = now_secs();

    write_start_file(root, "BAD1", Some("req-bad"), &base, now, false);
    let bad_path = root.join(".wt/BAD1.json");
    std::fs::write(&bad_path, "{ not valid json").unwrap();

    let (surface, prompts) = make_surface(root.to_path_buf());
    let err = report_finished_runs(&surface).unwrap_err();
    assert!(err.contains("BAD1"), "{err}");
    assert!(err.contains("BAD1.json"), "{err}");
    assert!(!root.join(".wt/BAD1.reported.json").exists());

    write_summary(
        root,
        "BAD1",
        &json!({
            "id": "BAD1",
            "turns": 3,
            "seconds": 60
        }),
    );
    let err_missing_outcome = report_finished_runs(&surface).unwrap_err();
    assert!(
        err_missing_outcome.contains("BAD1.json"),
        "{err_missing_outcome}"
    );
    assert!(!root.join(".wt/BAD1.reported.json").exists());

    write_summary(
        root,
        "BAD1",
        &json!({
            "id": "BAD1",
            "outcome": "verified",
            "seconds": 60
        }),
    );
    let err_missing_turns = report_finished_runs(&surface).unwrap_err();
    assert!(
        err_missing_turns.contains("BAD1.json"),
        "{err_missing_turns}"
    );
    assert!(!root.join(".wt/BAD1.reported.json").exists());

    write_summary(
        root,
        "BAD1",
        &json!({
            "id": "BAD1",
            "outcome": "verified",
            "turns": 3
        }),
    );
    let err_missing_seconds = report_finished_runs(&surface).unwrap_err();
    assert!(
        err_missing_seconds.contains("BAD1.json"),
        "{err_missing_seconds}"
    );
    assert!(!root.join(".wt/BAD1.reported.json").exists());

    assert!(surface.session.entries_after(0, Duration::ZERO).is_empty());
    std::thread::sleep(Duration::from_millis(100));
    assert!(prompts.lock().unwrap().is_empty());
}

#[test]
fn router_get_api_runs_adds_entry_once_across_two_calls() {
    let (repo, base) = init_repo();
    let root = repo.path();
    let now = now_secs();

    create_run_branch(root, "RT1", &base, "rt1.txt", "hello\n", "feat: rt1");
    write_start_file(root, "RT1", Some("req-rt"), &base, now, false);
    write_summary(
        root,
        "RT1",
        &json!({
            "id": "RT1",
            "branch": "wip/RT1",
            "outcome": "verified",
            "turns": 5,
            "cost_usd": 0.19,
            "seconds": 120,
            "final_message": "Added rt1.txt.",
            "gate_failures": []
        }),
    );
    std::fs::write(root.join(".wt/RT1.report.json"), "{\"turns\":5}\n").unwrap();

    let (surface, prompts) = make_surface(root.to_path_buf());

    let resp1 = route(&surface, &get("/api/runs"), now + 1);
    assert_eq!(resp1.status, 200);
    surface.requests.wait_all_idle();

    let resp2 = route(&surface, &get("/api/runs"), now + 2);
    assert_eq!(resp2.status, 200);
    surface.requests.wait_all_idle();

    let entries = surface.session.entries_after(0, Duration::ZERO);
    let decisions: Vec<_> = entries
        .iter()
        .filter(|e| matches!(&e.body, EntryBody::Decision { run, .. } if run == "RT1"))
        .collect();
    assert_eq!(decisions.len(), 1);
    let EntryBody::Decision {
        note: Some(note), ..
    } = &decisions[0].body
    else {
        panic!("expected Decision with note");
    };
    assert!(note.starts_with("Run RT1: verified, 5 turns, USD 0.19, 2.0 min\nAdded rt1.txt."));
    assert!(root.join(".wt/RT1.reported.json").exists());

    std::thread::sleep(Duration::from_millis(100));
    assert!(prompts.lock().unwrap().is_empty());
}
