use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use blyte_agent::tools::PeerTools;
use blyte_cli::cli::SurfaceArgs;
use blyte_cli::repl::ReplConfig;
use blyte_cli::serve::{check_linux_non_repo_root, run_with_hooks, run_with_runner};
use blyte_cli::session::{EntryBody, PromptRunner};
use blyte_cli::surface_dir::{folder_store, list_records_for_root};
use blyte_cli::surface_registry::{entry_path, registry_dir, state_dir, RegistryEntry};
use blyte_cli::surfaces_cmd::run_surfaces;
use blyte_cli::tell::SurfacePeerTools;

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

fn ensure_isolated_state_home() {
    static ISOLATED_STATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    let _ = ISOLATED_STATE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
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

fn parse_surface_ready_line(line: &str) -> Option<(String, u16, String)> {
    let rest = line.strip_prefix("blyte: surface ")?;
    let (name, url) = rest.split_once(" at http://127.0.0.1:")?;
    let (port_str, token) = url.split_once("/#token=")?;
    let port: u16 = port_str.parse().ok()?;
    Some((name.to_string(), port, token.to_string()))
}

struct RunningSurface {
    name: String,
    port: u16,
    token: String,
    stop: Arc<AtomicBool>,
    err_buf: Arc<Mutex<Vec<u8>>>,
    handle: std::thread::JoinHandle<i32>,
}

impl RunningSurface {
    fn stop_and_join(self) -> (i32, String) {
        self.stop.store(true, Ordering::Relaxed);
        let code = self.handle.join().unwrap();
        let stderr = String::from_utf8_lossy(&self.err_buf.lock().unwrap()).into_owned();
        (code, stderr)
    }
}

fn start_with_hooks(dir: &Path, args: SurfaceArgs) -> RunningSurface {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let dir_bg = dir.to_path_buf();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let cfg = default_repl_config(dir_bg);
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = |_: &str| {};
        run_with_hooks(&args, cfg, &mut out, &mut err, &mut opener, stop_bg)
    });
    wait_for_surface(out_buf, err_buf, stop, handle)
}

fn start_with_runner(
    dir: &Path,
    args: SurfaceArgs,
    runner: Box<dyn PromptRunner>,
) -> RunningSurface {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let dir_bg = dir.to_path_buf();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let cfg = default_repl_config(dir_bg);
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = |_: &str| {};
        run_with_runner(&args, cfg, &mut out, &mut err, &mut opener, stop_bg, runner)
    });
    wait_for_surface(out_buf, err_buf, stop, handle)
}

fn wait_for_surface(
    out_buf: Arc<Mutex<Vec<u8>>>,
    err_buf: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<i32>,
) -> RunningSurface {
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

fn http_request(
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    if let Some(tok) = token {
        req.push_str(&format!("X-Blyte-Token: {tok}\r\n"));
    }
    if let Some(payload) = body {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    }
    req.push_str("Connection: close\r\n\r\n");
    if let Some(payload) = body {
        req.push_str(payload);
    }
    stream.write_all(req.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8(raw).unwrap();
    let (head, resp_body) = text.split_once("\r\n\r\n").unwrap();
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, resp_body.to_string())
}

fn default_surface_args() -> SurfaceArgs {
    SurfaceArgs {
        port: 0,
        no_open: true,
        days: 14,
        name: None,
        resume: None,
    }
}

fn git_init_with_commit(dir: &Path, branch: &str) {
    let run = |args: &[&str]| {
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
    run(&["init", "-b", branch]);
    run(&["config", "user.name", "Test"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["commit", "--allow-empty", "-m", "init"]);
}

struct ScriptedRunner;

impl PromptRunner for ScriptedRunner {
    fn run(
        &mut self,
        _prompt: &str,
        _model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        emit(EntryBody::Turn {
            n: 1,
            text: "ok".to_string(),
            input_tokens: 1,
            output_tokens: 1,
            cost_usd: None,
        });
        Ok(())
    }
}

#[test]
fn starts_in_a_plain_folder_and_writes_nothing_into_it() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("plain-notes");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("readme.txt"), "hello\n").unwrap();
    let canonical = folder.canonicalize().unwrap();

    let running = start_with_hooks(&folder, default_surface_args());

    let (health_status, health_body) = http_request(running.port, "GET", "/api/health", None, None);
    assert_eq!(health_status, 200, "body={health_body}");
    let health: serde_json::Value = serde_json::from_str(&health_body).unwrap();
    let surface_id = health["surface_id"].as_str().unwrap().to_string();

    let (sess_status, sess_body) = http_request(
        running.port,
        "GET",
        "/api/session",
        Some(&running.token),
        None,
    );
    assert_eq!(sess_status, 200, "body={sess_body}");
    let session: serde_json::Value = serde_json::from_str(&sess_body).unwrap();
    assert_eq!(session["repository"], serde_json::Value::Bool(false));

    let (code, stderr) = running.stop_and_join();
    assert_eq!(code, 0, "stderr={stderr}");
    let expected_notice = format!(
        "blyte: {} is not a git repository: runs, combining and publishing are off until it is one",
        canonical.display()
    );
    assert!(
        stderr.contains(&expected_notice),
        "expected notice `{expected_notice}` in stderr:\n{stderr}"
    );

    let entries: Vec<String> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["readme.txt".to_string()]);

    let sd = state_dir().unwrap();
    let store = folder_store(&sd, &folder).unwrap();
    assert!(store.starts_with(sd.join("folders")));
    let root_file = std::fs::read_to_string(store.join("root")).unwrap();
    assert_eq!(PathBuf::from(root_file.trim()), canonical);
    assert!(store
        .join("surfaces")
        .join(&surface_id)
        .join("surface.json")
        .is_file());
    assert!(store
        .join("surfaces")
        .join(&surface_id)
        .join("entries.jsonl")
        .is_file());
}

#[test]
fn registry_entry_has_no_branch_and_surfaces_prints_a_dash() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("no-branch-folder");
    std::fs::create_dir_all(&folder).unwrap();

    let running = start_with_hooks(&folder, default_surface_args());
    let (_, health_body) = http_request(running.port, "GET", "/api/health", None, None);
    let health: serde_json::Value = serde_json::from_str(&health_body).unwrap();
    let surface_id = health["surface_id"].as_str().unwrap().to_string();

    let reg_dir = registry_dir().unwrap();
    let raw = std::fs::read_to_string(entry_path(&reg_dir, &surface_id)).unwrap();
    let raw_val: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(raw_val["branch"].is_null(), "raw entry: {raw}");
    let entry: RegistryEntry = serde_json::from_str(&raw).unwrap();
    assert_eq!(entry.branch, None);

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_surfaces(&reg_dir, false, &mut out, &mut err);
    assert_eq!(code, 0, "stderr={}", String::from_utf8_lossy(&err));
    let table = String::from_utf8(out).unwrap();
    let row = table
        .lines()
        .find(|line| line.starts_with(&running.name))
        .unwrap_or_else(|| panic!("surface row not found in:\n{table}"));
    let cols: Vec<&str> = row.split_whitespace().collect();
    assert_eq!(cols[0], running.name);
    assert_eq!(cols[1], "no-branch-folder");
    assert_eq!(cols[2], "-");

    let peer_tools = SurfacePeerTools {
        registry_dir: reg_dir,
        surface_id: "s0000000000000099".to_string(),
        surface_name: "other".to_string(),
        repository: "other".to_string(),
    };
    let tool_table = peer_tools.surfaces().unwrap();
    let tool_row = tool_table
        .lines()
        .find(|line| line.starts_with(&running.name))
        .unwrap_or_else(|| panic!("surface row not found in tool output:\n{tool_table}"));
    let tool_cols: Vec<&str> = tool_row.split_whitespace().collect();
    assert_eq!(tool_cols[2], "-");

    let (stop_code, stderr) = running.stop_and_join();
    assert_eq!(stop_code, 0, "stderr={stderr}");
}

#[test]
fn resume_finds_a_surface_started_without_a_repository() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("resume-plain");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), "one\n").unwrap();

    let first = start_with_hooks(
        &folder,
        SurfaceArgs {
            name: Some("plain-resume-one".to_string()),
            ..default_surface_args()
        },
    );
    let (_, health1_body) = http_request(first.port, "GET", "/api/health", None, None);
    let health1: serde_json::Value = serde_json::from_str(&health1_body).unwrap();
    let first_id = health1["surface_id"].as_str().unwrap().to_string();
    let (code1, stderr1) = first.stop_and_join();
    assert_eq!(code1, 0, "stderr={stderr1}");

    let sd = state_dir().unwrap();
    let store = folder_store(&sd, &folder).unwrap();
    let log_path = store.join("surfaces").join(&first_id).join("entries.jsonl");
    assert!(log_path.is_file());

    let resumed = start_with_hooks(
        &folder,
        SurfaceArgs {
            resume: Some("plain-resume-one".to_string()),
            ..default_surface_args()
        },
    );
    let (_, health2_body) = http_request(resumed.port, "GET", "/api/health", None, None);
    let health2: serde_json::Value = serde_json::from_str(&health2_body).unwrap();
    assert_eq!(health2["surface_id"].as_str().unwrap(), first_id);
    assert_eq!(resumed.name, "plain-resume-one");

    let located = list_records_for_root(&folder, Some(&sd)).unwrap();
    assert_eq!(located.len(), 1);
    assert_eq!(located[0].record.id, first_id);
    assert_eq!(located[0].dir, store.join("surfaces").join(&first_id));

    let (code2, stderr2) = resumed.stop_and_join();
    assert_eq!(code2, 0, "stderr={stderr2}");

    let entries: Vec<String> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["a.txt".to_string()]);
}

#[test]
fn resume_still_works_after_git_init() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("before-git-init");
    std::fs::create_dir_all(&folder).unwrap();

    let first = start_with_hooks(
        &folder,
        SurfaceArgs {
            name: Some("pre-git-surface".to_string()),
            ..default_surface_args()
        },
    );
    let (_, health1_body) = http_request(first.port, "GET", "/api/health", None, None);
    let health1: serde_json::Value = serde_json::from_str(&health1_body).unwrap();
    let first_id = health1["surface_id"].as_str().unwrap().to_string();
    let (code1, stderr1) = first.stop_and_join();
    assert_eq!(code1, 0, "stderr={stderr1}");

    git_init_with_commit(&folder, "main");

    let resumed = start_with_hooks(
        &folder,
        SurfaceArgs {
            resume: Some("pre-git-surface".to_string()),
            ..default_surface_args()
        },
    );
    let (_, health2_body) = http_request(resumed.port, "GET", "/api/health", None, None);
    let health2: serde_json::Value = serde_json::from_str(&health2_body).unwrap();
    assert_eq!(health2["surface_id"].as_str().unwrap(), first_id);

    let (sess_status, sess_body) = http_request(
        resumed.port,
        "GET",
        "/api/session",
        Some(&resumed.token),
        None,
    );
    assert_eq!(sess_status, 200, "body={sess_body}");
    let session: serde_json::Value = serde_json::from_str(&sess_body).unwrap();
    assert_eq!(session["repository"], serde_json::Value::Bool(true));

    let sd = state_dir().unwrap();
    let store = folder_store(&sd, &folder).unwrap();
    let expected_dir = store.join("surfaces").join(&first_id);
    assert!(expected_dir.join("surface.json").is_file());
    assert!(expected_dir.join("entries.jsonl").is_file());
    assert!(!folder.join(".wt").join("surfaces").join(&first_id).exists());

    let (code2, stderr2) = resumed.stop_and_join();
    assert_eq!(code2, 0, "stderr={stderr2}");
}

#[test]
fn branch_appears_after_git_init_without_restart() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("live-git-init");
    std::fs::create_dir_all(&folder).unwrap();

    let running = start_with_runner(&folder, default_surface_args(), Box::new(ScriptedRunner));
    let (_, health_body) = http_request(running.port, "GET", "/api/health", None, None);
    let health: serde_json::Value = serde_json::from_str(&health_body).unwrap();
    let surface_id = health["surface_id"].as_str().unwrap().to_string();

    let reg_dir = registry_dir().unwrap();
    let initial: RegistryEntry =
        serde_json::from_str(&std::fs::read_to_string(entry_path(&reg_dir, &surface_id)).unwrap())
            .unwrap();
    assert_eq!(initial.branch, None);

    let (_, sess_before_body) = http_request(
        running.port,
        "GET",
        "/api/session",
        Some(&running.token),
        None,
    );
    let sess_before: serde_json::Value = serde_json::from_str(&sess_before_body).unwrap();
    assert_eq!(sess_before["repository"], serde_json::Value::Bool(false));

    git_init_with_commit(&folder, "main");

    let (sess_after_status, sess_after_body) = http_request(
        running.port,
        "GET",
        "/api/session",
        Some(&running.token),
        None,
    );
    assert_eq!(sess_after_status, 200);
    let sess_after: serde_json::Value = serde_json::from_str(&sess_after_body).unwrap();
    assert_eq!(sess_after["repository"], serde_json::Value::Bool(true));

    let (msg_status, msg_body) = http_request(
        running.port,
        "POST",
        "/api/session/messages",
        Some(&running.token),
        Some(r#"{"text":"hello after git init"}"#),
    );
    assert_eq!(msg_status, 200, "body={msg_body}");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let updated: RegistryEntry = serde_json::from_str(
            &std::fs::read_to_string(entry_path(&reg_dir, &surface_id)).unwrap(),
        )
        .unwrap();
        if updated.branch.as_deref() == Some("main") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for branch to appear in registry entry; got {:?}",
            updated.branch
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let (code, stderr) = running.stop_and_join();
    assert_eq!(code, 0, "stderr={stderr}");
}

#[test]
fn push_route_answers_409_without_a_repository() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("no-git-routes");
    std::fs::create_dir_all(&folder).unwrap();

    let running = start_with_hooks(&folder, default_surface_args());
    let expected = serde_json::json!({ "error": "this folder is not a git repository" });

    let (push_status, push_body) = http_request(
        running.port,
        "POST",
        "/api/requests/req-1/push",
        Some(&running.token),
        Some("{}"),
    );
    assert_eq!(push_status, 409, "body={push_body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&push_body).unwrap(),
        expected
    );

    let (pr_status, pr_body) = http_request(
        running.port,
        "POST",
        "/api/requests/req-1/pull-request",
        Some(&running.token),
        Some(r#"{"title":"t","body":"b"}"#),
    );
    assert_eq!(pr_status, 409, "body={pr_body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&pr_body).unwrap(),
        expected
    );

    let (diff_status, diff_body) = http_request(
        running.port,
        "GET",
        "/api/runs/T1/diff",
        Some(&running.token),
        None,
    );
    assert_eq!(diff_status, 409, "body={diff_body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&diff_body).unwrap(),
        expected
    );

    let (dec_status, dec_body) = http_request(
        running.port,
        "POST",
        "/api/runs/T1/decision",
        Some(&running.token),
        Some(r#"{"action":"accept"}"#),
    );
    assert_eq!(dec_status, 409, "body={dec_body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&dec_body).unwrap(),
        expected
    );

    let (runs_status, runs_body) =
        http_request(running.port, "GET", "/api/runs", Some(&running.token), None);
    assert_eq!(runs_status, 200, "body={runs_body}");
    let runs_val: serde_json::Value = serde_json::from_str(&runs_body).unwrap();
    assert_eq!(runs_val["running"], serde_json::json!([]));
    assert_eq!(runs_val["runs"], serde_json::json!([]));

    let (reqs_status, reqs_body) = http_request(
        running.port,
        "GET",
        "/api/requests",
        Some(&running.token),
        None,
    );
    assert_eq!(reqs_status, 200, "body={reqs_body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&reqs_body).unwrap(),
        serde_json::json!([])
    );

    let (code, stderr) = running.stop_and_join();
    assert_eq!(code, 0, "stderr={stderr}");
}

#[test]
fn a_broken_repository_is_still_an_error() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("broken-git");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(".git"), "garbage not a gitdir\n").unwrap();

    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut opener = |_: &str| {};
    let stop = Arc::new(AtomicBool::new(false));
    let code = run_with_hooks(
        &default_surface_args(),
        default_repl_config(folder),
        &mut out,
        &mut err,
        &mut opener,
        stop,
    );
    assert_eq!(code, 1);
    let stderr = String::from_utf8(err).unwrap();
    assert!(
        !stderr.contains("runs, combining and publishing are off"),
        "broken repo must not start without a repository; stderr={stderr}"
    );
    assert!(
        stderr.contains("not inside a git repository"),
        "expected git error in stderr={stderr}"
    );
}

#[test]
fn folder_store_refuses_a_key_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    let dir_a = tmp.path().join("folder-a");
    let dir_b = tmp.path().join("folder-b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    let store_a = folder_store(&state, &dir_a).unwrap();
    let canon_a = dir_a.canonicalize().unwrap();
    let canon_b = dir_b.canonicalize().unwrap();
    std::fs::write(store_a.join("root"), canon_b.to_string_lossy().as_bytes()).unwrap();

    let err = folder_store(&state, &dir_a).unwrap_err();
    assert!(
        err.contains(&canon_a.display().to_string()),
        "collision error should name requested path {}; got: {err}",
        canon_a.display()
    );
    assert!(
        err.contains(&canon_b.display().to_string()),
        "collision error should name recorded path {}; got: {err}",
        canon_b.display()
    );
}

#[test]
fn linux_root_check_refuses_denied_ancestors_and_allows_subfolders() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    let ssh_dir = fake_home.join(".ssh");
    let work_dir = fake_home.join("work");
    std::fs::create_dir_all(&ssh_dir).unwrap();
    std::fs::create_dir_all(&work_dir).unwrap();

    let err = check_linux_non_repo_root(&fake_home, &fake_home).unwrap_err();
    assert!(
        err.contains("on Linux a surface cannot start in"),
        "err={err}"
    );
    assert!(err.contains(".ssh"), "err={err}");
    assert!(
        err.contains("which the sandbox could not keep unreadable; start it in a subfolder"),
        "err={err}"
    );

    let ssh_err = check_linux_non_repo_root(&ssh_dir, &fake_home).unwrap_err();
    assert!(ssh_err.contains(".ssh"), "err={ssh_err}");

    check_linux_non_repo_root(&work_dir, &fake_home).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn linux_refuses_a_root_that_contains_ssh() {
    ensure_isolated_state_home();
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("fake-home");
    std::fs::create_dir_all(fake_home.join(".ssh")).unwrap();

    let prev_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &fake_home);

    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut opener = |_: &str| {};
    let stop = Arc::new(AtomicBool::new(false));
    let code = run_with_hooks(
        &default_surface_args(),
        default_repl_config(fake_home.clone()),
        &mut out,
        &mut err,
        &mut opener,
        stop,
    );

    if let Some(prev) = prev_home {
        std::env::set_var("HOME", prev);
    }

    assert_eq!(code, 1);
    let stderr = String::from_utf8(err).unwrap();
    let canon_home = fake_home.canonicalize().unwrap();
    let expected = format!(
        "blyte: on Linux a surface cannot start in {}: it contains {}, which the sandbox could not keep unreadable; start it in a subfolder",
        canon_home.display(),
        canon_home.join(".ssh").display()
    );
    assert!(
        stderr.contains(&expected),
        "expected `{expected}` in stderr:\n{stderr}"
    );
}
