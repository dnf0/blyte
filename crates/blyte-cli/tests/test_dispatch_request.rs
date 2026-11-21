use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, set_blyte_tool};
use blyte_cli::dispatch::validate_request_id;
use blyte_cli::http::Request;
use blyte_cli::serve::{route, Surface};
use blyte_cli::session::{EntryBody, PromptRunner, Session};
use serde_json::json;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    std::fs::write(dir.join("f.txt"), "a\n").unwrap();
    std::fs::write(dir.join("task.md"), "do the task").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "init", "--no-gpg-sign"]);
}

fn run_dispatch_cli(repo: &Path, id: &str, extra: &[&str]) -> std::process::Output {
    let mut args = vec![
        "dispatch",
        id,
        "task.md",
        "--gate",
        "true",
        "--sandbox",
        "off",
    ];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(&args)
        .current_dir(repo)
        .env("BLYTE_STATE_DIR", repo.join(".state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

#[test]
fn dispatch_records_and_validates_request_flag() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path();
    init_repo(repo);

    let out = run_dispatch_cli(repo, "R1", &["--request", "r1700000000-1_aZ"]);
    assert!(
        !out.stdout.is_empty(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let start_r1: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/R1.start.json")).unwrap())
            .unwrap();
    assert_eq!(start_r1["request"], "r1700000000-1_aZ");

    let out_none = run_dispatch_cli(repo, "R2", &[]);
    assert!(
        !out_none.stdout.is_empty(),
        "stderr={}",
        String::from_utf8_lossy(&out_none.stderr)
    );
    let start_r2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/R2.start.json")).unwrap())
            .unwrap();
    assert!(start_r2["request"].is_null(), "{start_r2}");

    let max_len = "a".repeat(64);
    let out_max = run_dispatch_cli(repo, "R3", &["--request", &max_len]);
    assert!(
        !out_max.stdout.is_empty(),
        "stderr={}",
        String::from_utf8_lossy(&out_max.stderr)
    );
    let start_r3: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/R3.start.json")).unwrap())
            .unwrap();
    assert_eq!(start_r3["request"], max_len);

    let too_long = "a".repeat(65);
    for (idx, bad) in [
        "",
        too_long.as_str(),
        "bad/id",
        "bad id",
        "bad.id",
        "bad..id",
        "h\u{00e9}llo",
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("BAD{idx}");
        let res = run_dispatch_cli(repo, &id, &["--request", bad]);
        assert!(
            !res.status.success(),
            "expected failure for --request {bad:?}"
        );
        let stderr = String::from_utf8_lossy(&res.stderr);
        assert!(
            stderr.contains("--request"),
            "expected --request error for {bad:?}, got stderr={stderr:?}"
        );
        assert!(
            !repo.join(format!(".wt/{id}.start.json")).exists(),
            "start.json should not be written for invalid --request {bad:?}"
        );
    }
}

struct DispatchingRunner {
    repo: PathBuf,
    run_ids: Vec<String>,
    attempt_spoof_on_first: bool,
    spoof_error: Arc<Mutex<Option<String>>>,
}

impl PromptRunner for DispatchingRunner {
    fn run(
        &mut self,
        _prompt: &str,
        _model: &str,
        _emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        let id = self.run_ids.remove(0);
        if self.attempt_spoof_on_first {
            self.attempt_spoof_on_first = false;
            let spoof = execute(
                &self.repo,
                &LocalShell,
                &LocalFs,
                "blyte",
                &json!({
                    "args": ["dispatch", "SPOOF", "task.md", "--request", "model-chosen"]
                }),
            );
            assert!(spoof.is_error, "model-supplied --request must be refused");
            *self.spoof_error.lock().unwrap() = Some(spoof.content);
        }
        let _out = execute(
            &self.repo,
            &LocalShell,
            &LocalFs,
            "blyte",
            &json!({
                "args": [
                    "dispatch",
                    id,
                    "task.md",
                    "--gate",
                    "true",
                    "--sandbox",
                    "off"
                ]
            }),
        );
        Ok(())
    }
}

struct ToolReset;

impl Drop for ToolReset {
    fn drop(&mut self) {
        set_blyte_tool(None);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

const TEST_TOKEN: &str = "test-surface-token";

fn post_json(path: &str, body: serde_json::Value) -> Request {
    Request {
        method: "POST".to_string(),
        path: path.to_string(),
        query: Vec::new(),
        headers: vec![(
            blyte_cli::serve::TOKEN_HEADER.to_string(),
            TEST_TOKEN.to_string(),
        )],
        body: serde_json::to_vec(&body).unwrap(),
    }
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

fn wait_for_idle_count(session: &Session, expected_idles: usize) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let entries = session.entries_after(0, Duration::from_millis(50));
        let idles = entries
            .iter()
            .filter(|e| matches!(e.body, EntryBody::Idle { .. }))
            .count();
        if idles >= expected_idles {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected_idles} idles; saw {idles}"
        );
    }
}

#[test]
fn surface_dispatch_carries_current_request_and_two_messages_get_two_ids() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    init_repo(&repo);

    std::env::set_var("BLYTE_STATE_DIR", repo.join(".state"));
    std::env::set_var("BLYTE_NO_UPDATE_CHECK", "1");
    std::env::remove_var("GEMINI_API_KEY");
    std::env::remove_var("GOOGLE_API_KEY");
    std::env::remove_var("BLYTE_MODEL");

    let _tool_reset = ToolReset;
    set_blyte_tool(Some(PathBuf::from(env!("CARGO_BIN_EXE_blyte"))));

    let spoof_error = Arc::new(Mutex::new(None));
    let runner = DispatchingRunner {
        repo: repo.clone(),
        run_ids: vec!["M1".to_string(), "M2".to_string()],
        attempt_spoof_on_first: true,
        spoof_error: Arc::clone(&spoof_error),
    };
    let session = Session::start(Box::new(runner), "gemini-4-neon".to_string());
    let now = now_secs();
    let surface = Surface {
        root: repo.clone(),
        repo: "repo".to_string(),
        session: session.clone(),
        session_id: "sess-req-test".to_string(),
        started_at: now,
        days: 14,
        token: TEST_TOKEN.to_string(),
        requests: Arc::new(blyte_cli::requests::RequestsState::new()),
        publisher: Arc::new(blyte_cli::requests::RealPublisher),
    };

    let r1 = route(
        &surface,
        &post_json(
            "/api/session/messages",
            json!({"text": "first user message"}),
        ),
        now,
    );
    assert_eq!(r1.status, 200);
    wait_for_idle_count(&session, 1);

    let r2 = route(
        &surface,
        &post_json(
            "/api/session/messages",
            json!({"text": "second user message"}),
        ),
        now,
    );
    assert_eq!(r2.status, 200);
    wait_for_idle_count(&session, 2);

    let err_msg = spoof_error.lock().unwrap().clone().unwrap();
    assert!(err_msg.contains("--request"), "unexpected error: {err_msg}");
    assert!(!repo.join(".wt/SPOOF.start.json").exists());

    let m1_start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/M1.start.json")).unwrap())
            .unwrap();
    let m2_start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/M2.start.json")).unwrap())
            .unwrap();

    let req1 = m1_start["request"]
        .as_str()
        .expect("M1 start.json should have a string request id");
    let req2 = m2_start["request"]
        .as_str()
        .expect("M2 start.json should have a string request id");

    assert_ne!(req1, req2, "two messages must get two distinct request ids");
    validate_request_id(req1).expect("req1 must pass validation");
    validate_request_id(req2).expect("req2 must pass validation");

    for req in [req1, req2] {
        let valid_branch = Command::new("git")
            .args(["check-ref-format", "--branch", &format!("req/{req}")])
            .current_dir(&repo)
            .output()
            .unwrap()
            .status
            .success();
        assert!(
            valid_branch,
            "request id {req:?} is not safe as a branch-name component"
        );
    }

    let live_start = json!({
        "id": "LIVE1",
        "pid": std::process::id(),
        "started_at": now,
        "request": "r-live-1",
        "task_file": repo.join("task.md").display().to_string(),
        "branch": "wip/LIVE1",
    });
    std::fs::write(
        repo.join(".wt/LIVE1.start.json"),
        serde_json::to_string(&live_start).unwrap(),
    )
    .unwrap();

    let noreq_out = run_dispatch_cli(&repo, "NOREQ", &[]);
    assert!(
        !noreq_out.stdout.is_empty(),
        "stderr={}",
        String::from_utf8_lossy(&noreq_out.stderr)
    );

    let runs_resp = route(&surface, &get("/api/runs"), now);
    assert_eq!(
        runs_resp.status,
        200,
        "body={}",
        String::from_utf8_lossy(&runs_resp.body)
    );
    let payload: serde_json::Value = serde_json::from_slice(&runs_resp.body).unwrap();

    let running = payload["running"].as_array().unwrap();
    let live_run = running
        .iter()
        .find(|r| r["id"] == "LIVE1")
        .expect("LIVE1 should appear in running");
    assert_eq!(live_run["request"], "r-live-1");

    let runs = payload["runs"].as_array().unwrap();
    let m1 = runs
        .iter()
        .find(|r| r["id"] == "M1")
        .expect("M1 should appear in /api/runs");
    let m2 = runs
        .iter()
        .find(|r| r["id"] == "M2")
        .expect("M2 should appear in /api/runs");
    let noreq = runs
        .iter()
        .find(|r| r["id"] == "NOREQ")
        .expect("NOREQ should appear in /api/runs");
    assert_eq!(m1["request"], req1);
    assert_eq!(m2["request"], req2);
    assert!(noreq["request"].is_null(), "{noreq}");
}
