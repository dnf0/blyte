use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use blyte_agent::sandbox::surface_message_path_for_id;
use blyte_agent::tools::PeerTools;
use blyte_cli::cli::SurfaceArgs;
use blyte_cli::repl::ReplConfig;
use blyte_cli::serve::run_with_runner;
use blyte_cli::session::{peer_prompt, EntryBody, PromptRunner};
use blyte_cli::surface_dir::{list_records_in, surfaces_dir};
use blyte_cli::surface_registry::{
    entry_path, registry_dir, write_entry, RegistryEntry, SurfaceState,
};
use blyte_cli::tell::{run_tell, tell, SurfacePeerTools, TellOutcome};

const WAIT: Duration = Duration::from_secs(30);

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

struct RecordingRunner {
    prompts: Arc<Mutex<Vec<String>>>,
}

impl PromptRunner for RecordingRunner {
    fn run(
        &mut self,
        prompt: &str,
        _model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        self.prompts.lock().unwrap().push(prompt.to_string());
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

struct RunningSurface {
    id: String,
    name: String,
    prompts: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<i32>,
}

impl RunningSurface {
    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        assert_eq!(self.handle.join().unwrap(), 0);
    }
}

fn test_registry_dir() -> PathBuf {
    static STATE_HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    STATE_HOME.get_or_init(|| {
        let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
        std::fs::create_dir_all(base).unwrap();
        let dir = tempfile::tempdir_in(base).unwrap();
        std::env::set_var("XDG_STATE_HOME", dir.path());
        std::env::set_var("BLYTE_STATE_DIR", dir.path().join("blyte"));
        dir
    });
    registry_dir().unwrap()
}

fn init_git_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let out = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn default_repl_config(repo_root: PathBuf) -> ReplConfig {
    let _ = test_registry_dir();
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

fn parse_surface_ready_line(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("blyte: surface ")?;
    let (name, url) = rest.split_once(" at ")?;
    let after_scheme = url.trim().strip_prefix("http://127.0.0.1:")?;
    let _ = after_scheme.split_once("/#token=")?;
    Some(name.to_string())
}

fn start_surface(repo: &Path, name: &str) -> RunningSurface {
    let _ = test_registry_dir();
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let runner = RecordingRunner {
        prompts: Arc::clone(&prompts),
    };
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let repo_bg = repo.to_path_buf();
    let out_bg = Arc::clone(&out_buf);
    let err_bg = Arc::clone(&err_buf);
    let stop_bg = Arc::clone(&stop);
    let args = SurfaceArgs {
        port: 0,
        no_open: true,
        days: 14,
        name: Some(name.to_string()),
        resume: None,
    };
    let handle = std::thread::spawn(move || {
        let cfg = default_repl_config(repo_bg);
        let mut out = SharedBuf(out_bg);
        let mut err = SharedBuf(err_bg);
        let mut opener = |_: &str| {};
        run_with_runner(
            &args,
            cfg,
            &mut out,
            &mut err,
            &mut opener,
            stop_bg,
            Box::new(runner),
        )
    });

    let deadline = Instant::now() + WAIT;
    let ready_name = loop {
        let stdout = String::from_utf8(out_buf.lock().unwrap().clone()).unwrap();
        if let Some(parsed) = stdout.lines().find_map(parse_surface_ready_line) {
            break parsed;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for printed surface URL for {name}; stdout={stdout:?} stderr={:?}",
            String::from_utf8_lossy(&err_buf.lock().unwrap())
        );
        std::thread::sleep(Duration::from_millis(5));
    };

    let records = list_records_in(&surfaces_dir(repo)).unwrap();
    assert_eq!(records.len(), 1);
    let id = records[0].record.id.clone();

    RunningSurface {
        id,
        name: ready_name,
        prompts,
        stop,
        handle,
    }
}

fn wait_for_prompt_count(prompts: &Arc<Mutex<Vec<String>>>, expected: usize) -> Vec<String> {
    let deadline = Instant::now() + WAIT;
    loop {
        let current = prompts.lock().unwrap().clone();
        if current.len() >= expected {
            return current;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} prompts, got {} ({current:?})",
            current.len()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn tell_delivers_between_two_surfaces() {
    let reg_dir = test_registry_dir();
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let repo1 = d1.path().join("repo-one");
    let repo2 = d2.path().join("repo-two");
    init_git_repo(&repo1);
    init_git_repo(&repo2);

    let s1 = start_surface(&repo1, "one");
    let s2 = start_surface(&repo2, "two");

    let outcome = tell(&reg_dir, "one", "repo-one", "two", "hello").unwrap();
    assert_eq!(outcome, TellOutcome::Delivered);

    let seen = wait_for_prompt_count(&s2.prompts, 1);
    assert_eq!(seen[0], peer_prompt("one", "repo-one", "hello"));
    assert!(seen[0].contains("one (repo-one)"));

    s2.stop();
    s1.stop();
}

#[test]
fn tell_reports_held() {
    let reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("held-repo");
    init_git_repo(&repo);

    let target = start_surface(&repo, "tell-held-target");
    for i in 1..=5 {
        let outcome = tell(
            &reg_dir,
            "sender",
            "repo-a",
            "tell-held-target",
            &format!("msg {i}"),
        )
        .unwrap();
        assert_eq!(outcome, TellOutcome::Delivered);
    }
    let sixth = tell(&reg_dir, "sender", "repo-a", "tell-held-target", "msg 6").unwrap();
    assert_eq!(sixth, TellOutcome::Held);

    target.stop();
}

#[test]
fn tell_an_unknown_name_is_an_error() {
    let reg_dir = test_registry_dir();
    let err = tell(&reg_dir, "sender", "repo-a", "nosuch", "hello").unwrap_err();
    assert_eq!(err, "no running surface named nosuch");
}

#[test]
fn tell_a_stale_entry_is_an_error() {
    let reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let dead_port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let stale_id = "s00000000000000e1";
    let entry = RegistryEntry {
        id: stale_id.to_string(),
        name: "stale-tell-target".to_string(),
        root: d.path().join("ghost"),
        branch: Some("main".to_string()),
        pid: 999_999,
        port: dead_port,
        started_at: 1_700_000_000,
        doing: "idle".to_string(),
        state: SurfaceState::Idle,
    };
    write_entry(&reg_dir, &entry).unwrap();
    assert!(entry_path(&reg_dir, stale_id).exists());

    let err = tell(&reg_dir, "sender", "repo-a", "stale-tell-target", "hello").unwrap_err();
    assert_eq!(err, "no running surface named stale-tell-target");
    assert!(!entry_path(&reg_dir, stale_id).exists());
}

#[test]
fn tell_without_a_credential_file_is_an_error() {
    let reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("no-cred-repo");
    init_git_repo(&repo);

    let target = start_surface(&repo, "tell-no-cred");
    let cred_path = surface_message_path_for_id(&target.id).unwrap();
    std::fs::remove_file(&cred_path).unwrap();

    let err = tell(&reg_dir, "sender", "repo-a", "tell-no-cred", "hello").unwrap_err();
    assert!(
        err.contains(&cred_path.display().to_string()),
        "expected path {} in error: {err}",
        cred_path.display()
    );
    assert!(
        err.contains("surface tell-no-cred has no usable message credential at"),
        "unexpected error: {err}"
    );

    target.stop();
}

#[test]
fn run_tell_prints_the_outcome_and_exit_code() {
    let reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("run-tell-repo");
    init_git_repo(&repo);

    let target = start_surface(&repo, "run-tell-target");

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_tell(
        &reg_dir,
        "run-tell-target",
        "msg 1",
        "terminal",
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "delivered to run-tell-target\n"
    );
    assert!(err.is_empty());

    for i in 2..=5 {
        let mut o = Vec::new();
        let mut e = Vec::new();
        assert_eq!(
            run_tell(
                &reg_dir,
                "run-tell-target",
                &format!("msg {i}"),
                "terminal",
                &mut o,
                &mut e,
            ),
            0
        );
    }

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_tell(
        &reg_dir,
        "run-tell-target",
        "msg 6",
        "terminal",
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "held by run-tell-target: it has run five turns from other surfaces since its user last wrote; the message waits for its user's next message\n"
    );
    assert!(err.is_empty());

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_tell(
        &reg_dir,
        "missing-target",
        "hi",
        "terminal",
        &mut out,
        &mut err,
    );
    assert_eq!(code, 1);
    assert!(out.is_empty());
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "blyte: no running surface named missing-target\n"
    );

    target.stop();
}

#[test]
fn a_surface_cannot_tell_itself() {
    let reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("self-repo");
    init_git_repo(&repo);

    let s = start_surface(&repo, "self-surface");
    let tools = SurfacePeerTools {
        registry_dir: reg_dir,
        surface_id: s.id.clone(),
        surface_name: s.name.clone(),
        repository: "self-repo".to_string(),
    };

    assert_eq!(
        tools.tell("self-surface", "hello"),
        Err("a surface cannot tell itself".to_string())
    );
    assert_eq!(
        tools.tell(&s.id, "hello"),
        Err("a surface cannot tell itself".to_string())
    );

    s.stop();
}

#[test]
fn the_surfaces_tool_marks_this_surface() {
    let reg_dir = test_registry_dir();
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let repo1 = d1.path().join("mark-repo-a");
    let repo2 = d2.path().join("mark-repo-b");
    init_git_repo(&repo1);
    init_git_repo(&repo2);

    let s1 = start_surface(&repo1, "mark-alpha");
    let s2 = start_surface(&repo2, "mark-beta");

    let tools = SurfacePeerTools {
        registry_dir: reg_dir,
        surface_id: s1.id.clone(),
        surface_name: s1.name.clone(),
        repository: "mark-repo-a".to_string(),
    };
    let table = tools.surfaces().unwrap();
    assert!(
        table.contains("mark-alpha (this surface)"),
        "expected marked self in table:\n{table}"
    );
    assert!(
        table.contains("mark-beta"),
        "expected peer in table:\n{table}"
    );
    assert!(
        !table.contains("mark-beta (this surface)"),
        "peer must not be marked as this surface:\n{table}"
    );

    s2.stop();
    s1.stop();
}

#[test]
fn tell_never_prints_the_credential() {
    let reg_dir = test_registry_dir();
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("secret-repo");
    init_git_repo(&repo);

    let target = start_surface(&repo, "secret-target");
    let cred_path = surface_message_path_for_id(&target.id).unwrap();
    let cred = std::fs::read_to_string(&cred_path)
        .unwrap()
        .trim()
        .to_string();

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_tell(
        &reg_dir,
        "secret-target",
        "hello",
        "terminal",
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);
    let out_s = String::from_utf8(out).unwrap();
    let err_s = String::from_utf8(err).unwrap();
    assert!(!out_s.contains(&cred));
    assert!(!err_s.contains(&cred));

    let err_blank = tell(&reg_dir, "sender", "repo-a", "secret-target", "   ").unwrap_err();
    assert!(!err_blank.contains(&cred));

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run_tell(
        &reg_dir,
        "secret-target",
        "   ",
        "terminal",
        &mut out,
        &mut err,
    );
    assert_eq!(code, 1);
    let out_s = String::from_utf8(out).unwrap();
    let err_s = String::from_utf8(err).unwrap();
    assert!(!out_s.contains(&cred));
    assert!(!err_s.contains(&cred));

    std::fs::write(&cred_path, format!("{cred}\nextra\n")).unwrap();
    let err_bad_cred = tell(&reg_dir, "sender", "repo-a", "secret-target", "hello").unwrap_err();
    assert!(!err_bad_cred.contains(&cred));

    target.stop();
}
