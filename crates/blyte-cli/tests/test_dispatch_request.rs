use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
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

fn copy_dir_all(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let ty = entry.file_type().unwrap();
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn template_repo() -> &'static Path {
    static TEMPLATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            git(root, &["init", "-q"]);
            std::fs::write(root.join("f.txt"), "a\n").unwrap();
            std::fs::write(root.join("task.md"), "do the task").unwrap();
            git(root, &["add", "."]);
            git(
                root,
                &[
                    "-c",
                    "maintenance.auto=false",
                    "-c",
                    "gc.auto=0",
                    "commit",
                    "-qm",
                    "init",
                    "--no-gpg-sign",
                ],
            );
            dir
        })
        .path()
}

fn init_repo(dir: &Path) {
    copy_dir_all(template_repo(), dir);
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
    let home = repo.join(".home");
    let _ = std::fs::create_dir_all(&home);
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(&args)
        .current_dir(repo)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("BLYTE_ENV_FILE", "")
        .env("BLYTE_STATE_DIR", repo.join(".state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_USAGE_LOG", "off")
        .env("BLYTE_NO_KEEP_AWAKE", "1")
        .env("BLYTE_GATE_SLOT_DIR", repo.join(".slots"))
        .env("BLYTE_GATE_SLOTS", "16")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
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
    let bad_ids = [
        "",
        too_long.as_str(),
        "bad/id",
        "bad id",
        "bad.id",
        "bad..id",
        "h\u{00e9}llo",
    ];
    std::thread::scope(|s| {
        for (idx, bad) in bad_ids.into_iter().enumerate() {
            s.spawn(move || {
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
            });
        }
    });
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
        headers: vec![(
            blyte_cli::serve::TOKEN_HEADER.to_string(),
            TEST_TOKEN.to_string(),
        )],
        body: Vec::new(),
    }
}

fn wait_for_idle_count(session: &Session, expected_idles: usize) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let entries = session.entries_after(0, Duration::from_millis(5));
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
    std::env::set_var("BLYTE_USAGE_LOG", "off");
    std::env::set_var("BLYTE_NO_KEEP_AWAKE", "1");
    std::env::set_var("BLYTE_GATE_SLOT_DIR", repo.join(".slots"));
    std::env::set_var("BLYTE_GATE_SLOTS", "16");
    std::env::set_var("BLYTE_ENV_FILE", "");
    std::env::remove_var("GEMINI_API_KEY");
    std::env::remove_var("GOOGLE_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("OPENROUTER_API_KEY");
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

fn make_test_surface(repo: &Path, now: u64) -> Surface {
    let session = Session::start(Box::new(NoopRunner), "gemini-4-neon".to_string());
    Surface {
        root: repo.to_path_buf(),
        repo: "repo".to_string(),
        session,
        session_id: "sess-test".to_string(),
        started_at: now,
        days: 14,
        token: TEST_TOKEN.to_string(),
        requests: Arc::new(blyte_cli::requests::RequestsState::new()),
        publisher: Arc::new(blyte_cli::requests::RealPublisher),
    }
}

#[test]
fn api_runs_lists_finished_run_after_worktree_removed_and_ensure_checks_skips_it() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    init_repo(&repo);
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

    let wt_m2 = repo.join(".wt/M2");
    assert!(Command::new("git")
        .args(["worktree", "add", "-q", "-b", "wip/M2"])
        .arg(&wt_m2)
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let now = now_secs();
    let start = json!({
        "id": "M2",
        "pid": 999999,
        "started_at": now - 60,
        "base_sha": base_sha,
        "research": false,
        "task_file": repo.join("task.md").display().to_string(),
        "branch": "wip/M2",
    });
    std::fs::write(
        repo.join(".wt/M2.start.json"),
        serde_json::to_string(&start).unwrap(),
    )
    .unwrap();
    std::fs::write(
        repo.join(".wt/M2.report.json"),
        json!({ "outcome": "error", "turns": 0 }).to_string(),
    )
    .unwrap();
    std::fs::write(
        repo.join(".wt/M2.json"),
        json!({
            "outcome": "error",
            "seconds": 2,
            "final_message": "",
            "commits": [],
            "changed_files": []
        })
        .to_string(),
    )
    .unwrap();

    assert!(Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&wt_m2)
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    assert!(!wt_m2.exists());

    assert!(!blyte_cli::queue_actions::ensure_checks(&repo, "M2").unwrap());
    assert!(!repo.join(".wt/M2.checks.json").exists());

    let surface = make_test_surface(&repo, now);
    let runs_resp = route(&surface, &get("/api/runs"), now);
    assert_eq!(
        runs_resp.status,
        200,
        "body={}",
        String::from_utf8_lossy(&runs_resp.body)
    );
    let payload: serde_json::Value = serde_json::from_slice(&runs_resp.body).unwrap();
    let runs = payload["runs"].as_array().unwrap();
    let m2 = runs
        .iter()
        .find(|r| r["id"] == "M2")
        .expect("M2 should appear in /api/runs");
    assert_eq!(m2["checks_recorded"], false);
    assert!(!repo.join(".wt/M2.checks.json").exists());
}

#[test]
fn api_runs_lists_run_with_partially_removed_worktree_directory() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    init_repo(&repo);
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

    let wt_m3 = repo.join(".wt/M3");
    assert!(Command::new("git")
        .args(["worktree", "add", "-q", "-b", "wip/M3"])
        .arg(&wt_m3)
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let now = now_secs();
    std::fs::write(
        repo.join(".wt/M3.start.json"),
        json!({
            "id": "M3",
            "pid": 999999,
            "started_at": now - 60,
            "base_sha": base_sha,
            "research": false,
            "task_file": repo.join("task.md").display().to_string(),
            "branch": "wip/M3",
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        repo.join(".wt/M3.report.json"),
        json!({ "outcome": "error", "turns": 0 }).to_string(),
    )
    .unwrap();
    std::fs::write(
        repo.join(".wt/M3.json"),
        json!({
            "outcome": "error",
            "seconds": 2,
            "final_message": "",
            "commits": [],
            "changed_files": []
        })
        .to_string(),
    )
    .unwrap();

    std::fs::remove_file(wt_m3.join(".git")).unwrap();
    assert!(wt_m3.is_dir());

    let surface = make_test_surface(&repo, now);
    let runs_resp = route(&surface, &get("/api/runs"), now);
    assert_eq!(
        runs_resp.status,
        200,
        "body={}",
        String::from_utf8_lossy(&runs_resp.body)
    );
    let payload: serde_json::Value = serde_json::from_slice(&runs_resp.body).unwrap();
    let runs = payload["runs"].as_array().unwrap();
    let m3 = runs
        .iter()
        .find(|r| r["id"] == "M3")
        .expect("M3 should appear in /api/runs");
    assert_eq!(m3["checks_recorded"], false);
    assert!(!repo.join(".wt/M3.checks.json").exists());
}

#[test]
fn api_runs_still_returns_500_when_start_base_sha_is_not_in_repo() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    init_repo(&repo);
    let missing_sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

    let wt_bad = repo.join(".wt/BADSHA");
    assert!(Command::new("git")
        .args(["worktree", "add", "-q", "-b", "wip/BADSHA"])
        .arg(&wt_bad)
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let now = now_secs();
    std::fs::write(
        repo.join(".wt/BADSHA.start.json"),
        json!({
            "id": "BADSHA",
            "pid": 999999,
            "started_at": now - 60,
            "base_sha": missing_sha,
            "research": false,
            "task_file": repo.join("task.md").display().to_string(),
            "branch": "wip/BADSHA",
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        repo.join(".wt/BADSHA.report.json"),
        json!({ "outcome": "error", "turns": 0 }).to_string(),
    )
    .unwrap();
    std::fs::write(
        repo.join(".wt/BADSHA.json"),
        json!({
            "outcome": "error",
            "seconds": 2,
            "final_message": "",
            "commits": [],
            "changed_files": []
        })
        .to_string(),
    )
    .unwrap();

    let surface = make_test_surface(&repo, now);
    let runs_resp = route(&surface, &get("/api/runs"), now);
    assert_eq!(runs_resp.status, 500);
    let body = String::from_utf8_lossy(&runs_resp.body);
    assert!(
        body.contains(&format!(
            "run BADSHA started from {missing_sha}, which is not in this repository"
        )),
        "body={body}"
    );
}
