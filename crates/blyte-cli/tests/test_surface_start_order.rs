use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use blyte_agent::approval::Approver;
use blyte_cli::cli::SurfaceArgs;
use blyte_cli::repl::ReplConfig;
use blyte_cli::serve::{run_with_runner_factory, RunnerFactory};
use blyte_cli::session::{EntryBody, PromptRunner};
use blyte_cli::surface_dir::{list_records_in, surfaces_dir};
use blyte_cli::surface_registry::{live_entries, registry_dir};

const WAIT_DEADLINE: Duration = Duration::from_secs(30);

static STATE_DIR_LOCK: Mutex<()> = Mutex::new(());

struct IsolatedStateDir {
    _dir: tempfile::TempDir,
    reg_dir: PathBuf,
    prev_xdg_state: Option<std::ffi::OsString>,
    prev_xdg_config: Option<std::ffi::OsString>,
    prev_blyte_state: Option<std::ffi::OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl IsolatedStateDir {
    fn new() -> Self {
        let lock = STATE_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev_xdg_state = std::env::var_os("XDG_STATE_HOME");
        let prev_xdg_config = std::env::var_os("XDG_CONFIG_HOME");
        let prev_blyte_state = std::env::var_os("BLYTE_STATE_DIR");
        let blyte_state = dir.path().join("blyte");
        std::env::set_var("XDG_STATE_HOME", dir.path());
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("BLYTE_STATE_DIR", &blyte_state);
        let reg_dir = registry_dir().unwrap();
        Self {
            _dir: dir,
            reg_dir,
            prev_xdg_state,
            prev_xdg_config,
            prev_blyte_state,
            _lock: lock,
        }
    }

    fn registry_dir(&self) -> &Path {
        &self.reg_dir
    }
}

impl Drop for IsolatedStateDir {
    fn drop(&mut self) {
        match &self.prev_xdg_state {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
        match &self.prev_xdg_config {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match &self.prev_blyte_state {
            Some(v) => std::env::set_var("BLYTE_STATE_DIR", v),
            None => std::env::remove_var("BLYTE_STATE_DIR"),
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

struct NoopRunner;

impl PromptRunner for NoopRunner {
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

fn default_surface_args() -> SurfaceArgs {
    SurfaceArgs {
        no_open: true,
        ..SurfaceArgs::default()
    }
}

fn parse_surface_ready_line(line: &str) -> Option<(String, u16, String)> {
    let rest = line.strip_prefix("blyte: surface ")?;
    let (name, url) = rest.split_once(" at http://127.0.0.1:")?;
    let (port_str, token) = url.split_once("/#token=")?;
    let port: u16 = port_str.parse().ok()?;
    Some((name.to_string(), port, token.to_string()))
}

fn git_init_with_commit(dir: &Path) {
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
    run(&["init", "-b", "main"]);
    run(&["config", "user.name", "Test"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["commit", "--allow-empty", "-m", "init"]);
}

fn registry_files(reg_dir: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(reg_dir) {
        Ok(rd) => rd.map(|e| e.unwrap().path()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => panic!("failed to read {}: {e}", reg_dir.display()),
    }
}

struct RunningSurface {
    name: String,
    port: u16,
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

fn start_with_factory<F>(repo: &Path, args: SurfaceArgs, factory: F) -> RunningSurface
where
    F: FnOnce(
            &ReplConfig,
            Arc<dyn Approver>,
        ) -> Result<(Box<dyn PromptRunner>, Option<PathBuf>), String>
        + Send
        + 'static,
{
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
        run_with_runner_factory(
            &args,
            cfg,
            &mut out,
            &mut err,
            &mut opener,
            stop_bg,
            Box::new(factory),
        )
    });

    let deadline = Instant::now() + WAIT_DEADLINE;
    let (name, port, _token) = loop {
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
        stop,
        err_buf,
        handle,
    }
}

struct StartSnapshot {
    reg_files: Vec<PathBuf>,
    live_ids: Vec<String>,
    records: Result<Vec<String>, String>,
    allow_read: Vec<PathBuf>,
    read_roots: Vec<PathBuf>,
    attachments_exists: bool,
    log_exists: bool,
}

#[test]
fn nothing_advertises_the_surface_while_its_session_is_starting() {
    let state = IsolatedStateDir::new();
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("start-order-repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_init_with_commit(&repo);

    let observed: Arc<Mutex<Option<StartSnapshot>>> = Arc::new(Mutex::new(None));
    let reg_dir_bg = state.registry_dir().to_path_buf();
    let surfaces_root_bg = surfaces_dir(&repo);
    let observed_bg = Arc::clone(&observed);

    let running = start_with_factory(
        &repo,
        SurfaceArgs {
            name: Some("starting-surface".to_string()),
            ..default_surface_args()
        },
        move |cfg, _approver| {
            let reg_files = registry_files(&reg_dir_bg);
            let live_ids = live_entries(&reg_dir_bg)
                .unwrap()
                .into_iter()
                .map(|e| e.id)
                .collect();
            let records = list_records_in(&surfaces_root_bg)
                .map(|list| list.into_iter().map(|r| r.record.id).collect());
            let allow_read = cfg.allow_read.clone();
            let read_roots = blyte_agent::tools::read_roots();
            let attachments_exists = allow_read
                .iter()
                .any(|p| p.ends_with("attachments") && p.is_dir());
            let log_exists = allow_read
                .iter()
                .find(|p| p.ends_with("attachments"))
                .and_then(|p| p.parent())
                .map(|dir| dir.join("entries.jsonl").is_file())
                .unwrap_or(false);
            *observed_bg.lock().unwrap() = Some(StartSnapshot {
                reg_files,
                live_ids,
                records,
                allow_read,
                read_roots,
                attachments_exists,
                log_exists,
            });
            Ok((Box::new(NoopRunner) as Box<dyn PromptRunner>, None))
        },
    );

    let snapshot = observed
        .lock()
        .unwrap()
        .take()
        .expect("runner factory did not run");
    let during_records = snapshot
        .records
        .expect("list_records_in failed while session was starting");

    assert!(
        snapshot.attachments_exists,
        "attachments dir in config.allow_read must exist on disk before runner starts; allow_read={:?}",
        snapshot.allow_read
    );
    assert!(
        snapshot.log_exists,
        "entries.jsonl must be opened before runner starts; allow_read={:?}",
        snapshot.allow_read
    );
    let attachments_path = snapshot
        .allow_read
        .iter()
        .find(|p| p.ends_with("attachments"))
        .expect("attachments dir missing from config.allow_read");
    assert!(
        snapshot.read_roots.iter().any(|p| p == attachments_path),
        "read_roots must contain attachments dir before runner starts; read_roots={:?}",
        snapshot.read_roots
    );
    assert!(
        snapshot.reg_files.is_empty(),
        "registry directory must be empty while session is starting; found {:?}",
        snapshot.reg_files
    );
    assert!(
        snapshot.live_ids.is_empty(),
        "live_entries must be empty while session is starting; found {:?}",
        snapshot.live_ids
    );
    assert!(
        during_records.is_empty(),
        "list_records_in must be empty while session is starting; found {during_records:?}"
    );

    let after_live = live_entries(state.registry_dir()).unwrap();
    assert_eq!(after_live.len(), 1, "after_live={after_live:?}");
    assert_eq!(after_live[0].name, running.name);
    assert_eq!(after_live[0].port, running.port);

    let after_records = list_records_in(&surfaces_dir(&repo)).unwrap();
    assert_eq!(after_records.len(), 1, "after_records={after_records:?}");
    assert_eq!(after_records[0].record.name, running.name);
    assert_eq!(after_records[0].record.port, running.port);
    assert_eq!(after_records[0].record.id, after_live[0].id);
    assert!(
        !after_records[0].dir.join(".starting").exists(),
        ".starting marker must be removed once surface.json is written"
    );

    let (code, stderr) = running.stop_and_join();
    assert_eq!(code, 0, "stderr={stderr}");
}

#[test]
fn a_failed_session_start_leaves_no_record_and_no_registry_entry() {
    let state = IsolatedStateDir::new();
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("failed-start-repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_init_with_commit(&repo);

    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut opener = |_: &str| {};
    let stop = Arc::new(AtomicBool::new(false));
    let factory: RunnerFactory =
        Box::new(|_cfg, _approver| Err("simulated session startup failure".to_string()));

    let code = run_with_runner_factory(
        &SurfaceArgs {
            name: Some("doomed-surface".to_string()),
            ..default_surface_args()
        },
        default_repl_config(repo.clone()),
        &mut out,
        &mut err,
        &mut opener,
        stop,
        factory,
    );

    assert_eq!(code, 1);
    assert!(
        out.is_empty(),
        "no URL should be printed on failed start; stdout={}",
        String::from_utf8_lossy(&out)
    );
    let stderr = String::from_utf8(err).unwrap();
    assert!(
        stderr.contains("blyte: simulated session startup failure"),
        "stderr={stderr}"
    );
    assert!(
        registry_files(state.registry_dir()).is_empty(),
        "registry must hold no entry after failed start; found {:?}",
        registry_files(state.registry_dir())
    );
    let records = list_records_in(&surfaces_dir(&repo)).unwrap();
    assert!(
        records.is_empty(),
        "list_records_in must be empty after failed start; found {records:?}"
    );
    if let Ok(entries) = std::fs::read_dir(surfaces_dir(&repo)) {
        let leftover: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        assert!(
            leftover.is_empty(),
            "surfaces dir must hold no leftover directory after failed start; found {leftover:?}"
        );
    }
}

#[test]
fn a_failed_resume_leaves_the_stopped_record_unchanged() {
    let state = IsolatedStateDir::new();
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("failed-resume-repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_init_with_commit(&repo);

    let initial = start_with_factory(
        &repo,
        SurfaceArgs {
            name: Some("resumable-surface".to_string()),
            ..default_surface_args()
        },
        |_cfg, _approver| Ok((Box::new(NoopRunner) as Box<dyn PromptRunner>, None)),
    );
    let (stop_code, stop_stderr) = initial.stop_and_join();
    assert_eq!(stop_code, 0, "stderr={stop_stderr}");

    let records_before = list_records_in(&surfaces_dir(&repo)).unwrap();
    assert_eq!(records_before.len(), 1);
    let record_path = records_before[0].dir.join("surface.json");
    let bytes_before = std::fs::read(&record_path).unwrap();

    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut opener = |_: &str| {};
    let stop = Arc::new(AtomicBool::new(false));
    let failing_factory: RunnerFactory =
        Box::new(|_cfg, _approver| Err("simulated resume failure".to_string()));

    let code = run_with_runner_factory(
        &SurfaceArgs {
            resume: Some("resumable-surface".to_string()),
            ..default_surface_args()
        },
        default_repl_config(repo.clone()),
        &mut out,
        &mut err,
        &mut opener,
        stop,
        failing_factory,
    );

    assert_eq!(code, 1);
    assert!(
        out.is_empty(),
        "no URL should be printed on failed resume; stdout={}",
        String::from_utf8_lossy(&out)
    );
    let stderr = String::from_utf8(err).unwrap();
    assert!(
        stderr.contains("blyte: simulated resume failure"),
        "stderr={stderr}"
    );
    let bytes_after = std::fs::read(&record_path).unwrap();
    assert_eq!(
        bytes_after, bytes_before,
        "stopped surface record must be byte-for-byte unchanged after failed resume"
    );
    assert!(
        registry_files(state.registry_dir()).is_empty(),
        "registry must hold no entry after failed resume; found {:?}",
        registry_files(state.registry_dir())
    );
}
