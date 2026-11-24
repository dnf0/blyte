use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::cli::EditFinishArgs;
use blyte_cli::combine::{CombineCheck, CombineRecord, CombineState};
use blyte_cli::direct_edit::direct_edit_scope;
use blyte_cli::report::RunRecord;
use blyte_cli::requests::{maybe_combine, Readiness, RealPublisher, RequestsState};
use blyte_cli::serve::{generate_token, Surface};
use blyte_cli::session::{EntryBody, PromptRunner, Session};
use tempfile::TempDir;

fn blyte_bin() -> &'static str {
    env!("CARGO_BIN_EXE_blyte")
}

enum MockReviewReply {
    Ok(String),
    HttpError(u16, String),
}

struct MockReviewServer {
    port: u16,
    calls: Arc<AtomicUsize>,
}

impl MockReviewServer {
    fn start(reply: MockReviewReply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_bg = Arc::clone(&calls);

        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let _ = read_http_body(&mut stream);
                calls_bg.fetch_add(1, Ordering::SeqCst);
                match &reply {
                    MockReviewReply::Ok(text) => {
                        let body = serde_json::json!({
                            "id": "rev-1",
                            "object": "chat.completion",
                            "choices": [{
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "content": text,
                                },
                                "finish_reason": "stop",
                            }],
                            "usage": {
                                "prompt_tokens": 50,
                                "completion_tokens": 20,
                                "total_tokens": 70,
                            }
                        })
                        .to_string();
                        write_http_response(&mut stream, 200, "OK", &body);
                    }
                    MockReviewReply::HttpError(code, msg) => {
                        write_http_response(&mut stream, *code, "Error", msg);
                    }
                }
            }
        });

        Self { port, calls }
    }
}

fn read_http_body(stream: &mut std::net::TcpStream) -> String {
    let mut buf = vec![0u8; 65536];
    let mut got = Vec::new();
    let mut header_end = None;
    let mut content_len = 0usize;
    loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
        if header_end.is_none() {
            let text = String::from_utf8_lossy(&got);
            if let Some(i) = text.find("\r\n\r\n") {
                header_end = Some(i + 4);
                content_len = text[..i]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
            }
        }
        if let Some(end) = header_end {
            if got.len() >= end + content_len {
                break;
            }
        }
    }
    match header_end {
        Some(end) => String::from_utf8_lossy(&got[end..]).into_owned(),
        None => String::new(),
    }
}

fn write_http_response(stream: &mut std::net::TcpStream, status: u16, reason: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn default_clean_review_port() -> u16 {
    static SERVER: OnceLock<MockReviewServer> = OnceLock::new();
    SERVER
        .get_or_init(|| {
            MockReviewServer::start(MockReviewReply::Ok(
                r#"{"concerns": [], "fallbacks": []}"#.to_string(),
            ))
        })
        .port
}

fn isolated_home() -> &'static Path {
    static HOME_DIR: OnceLock<TempDir> = OnceLock::new();
    HOME_DIR.get_or_init(|| TempDir::new().unwrap()).path()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git failed to launch");
    assert!(
        out.status.success(),
        "git {:?} failed in {}:\nstdout: {}\nstderr: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn run_blyte_with_review(dir: &Path, args: &[&str], review_port: u16) -> Output {
    let home = isolated_home();
    Command::new(blyte_bin())
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("BLYTE_STATE_DIR", home.join("state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_NO_KEEP_AWAKE", "1")
        .env("BLYTE_USAGE_LOG", "off")
        .env("BLYTE_ENV_FILE", "")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_HOST")
        .env_remove("OLLAMA_API_KEY")
        .env(
            "OPENAI_BASE_URL",
            format!("http://127.0.0.1:{review_port}/v1"),
        )
        .env("OPENAI_API_KEY", "test")
        .env("BLYTE_MODEL", "openai/gemini-3.8-flash")
        .output()
        .expect("blyte failed to launch")
}

fn run_blyte(dir: &Path, args: &[&str]) -> Output {
    run_blyte_with_review(dir, args, default_clean_review_port())
}

fn setup_repo(toml_body: &str) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    init_repo(dir.path(), toml_body);
    dir
}

fn init_repo(root: &Path, toml_body: &str) {
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Test User"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    std::fs::write(root.join("blyte.toml"), toml_body).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial commit"]);
}

fn branch_exists(root: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(root)
        .status()
        .expect("git show-ref")
        .success()
}

#[test]
fn edit_start_creates_worktree_branch_task_and_edit_record() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();
    let head_sha = git(root, &["rev-parse", "HEAD"]);

    let out = run_blyte(
        root,
        &["edit-start", "e-one", "--task", "Fix README greeting"],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("edit-start stdout json");
    assert_eq!(parsed["id"], "e-one");
    assert_eq!(parsed["base_sha"], head_sha);
    let printed_path = PathBuf::from(parsed["path"].as_str().expect("path string"));
    assert!(printed_path.is_absolute());
    assert_eq!(
        std::fs::canonicalize(&printed_path).unwrap(),
        std::fs::canonicalize(root.join(".wt/e-one")).unwrap()
    );

    assert!(root.join(".wt/e-one").is_dir());
    assert!(branch_exists(root, "wip/e-one"));
    assert_eq!(
        std::fs::read_to_string(root.join(".wt/e-one.task.md")).unwrap(),
        "Fix README greeting"
    );

    let edit_raw = std::fs::read_to_string(root.join(".wt/e-one.edit.json")).unwrap();
    let edit_val: serde_json::Value = serde_json::from_str(&edit_raw).unwrap();
    assert_eq!(edit_val["id"], "e-one");
    assert_eq!(edit_val["base"], "HEAD");
    assert_eq!(edit_val["base_sha"], head_sha);
    assert!(edit_val["started_at"].as_u64().unwrap() > 0);
    assert_eq!(git(root, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
}

#[test]
fn edit_start_refuses_existing_id_across_all_four_cases() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();
    std::fs::create_dir_all(root.join(".wt")).unwrap();

    std::fs::create_dir_all(root.join(".wt/has-dir")).unwrap();
    let out = run_blyte(root, &["edit-start", "has-dir", "--task", "x"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("has-dir"), "stderr: {err}");

    std::fs::write(root.join(".wt/has-start.start.json"), "{}\n").unwrap();
    let out = run_blyte(root, &["edit-start", "has-start", "--task", "x"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("has-start.start.json"), "stderr: {err}");

    std::fs::write(root.join(".wt/has-edit.edit.json"), "{}\n").unwrap();
    let out = run_blyte(root, &["edit-start", "has-edit", "--task", "x"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("has-edit.edit.json"), "stderr: {err}");

    git(root, &["branch", "wip/has-branch"]);
    let out = run_blyte(root, &["edit-start", "has-branch", "--task", "x"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("wip/has-branch"), "stderr: {err}");
}

#[test]
fn edit_start_refuses_empty_task_bad_base_and_cleans_up_failed_setup() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let out = run_blyte(root, &["edit-start", "empty-task", "--task", "   \n\t "]);
    assert!(!out.status.success());
    assert!(!root.join(".wt/empty-task").exists());
    assert!(!branch_exists(root, "wip/empty-task"));

    let out = run_blyte(
        root,
        &[
            "edit-start",
            "bad-base",
            "--task",
            "change something",
            "--base",
            "does-not-exist-ref",
        ],
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("does-not-exist-ref"), "stderr: {err}");
    assert!(!root.join(".wt/bad-base").exists());
    assert!(!branch_exists(root, "wip/bad-base"));

    let setup_repo_dir =
        setup_repo("[dispatch]\ngate = \"true\"\nsetup = \"echo setup-boom >&2; exit 1\"\n");
    let sroot = setup_repo_dir.path();
    let out = run_blyte(
        sroot,
        &["edit-start", "setup-fail", "--task", "change something"],
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("setup"), "stderr: {err}");
    assert!(!sroot.join(".wt/setup-fail").exists());
    assert!(!sroot.join(".wt/setup-fail.edit.json").exists());
    assert!(!branch_exists(sroot, "wip/setup-fail"));
}

#[test]
fn edit_finish_errors_on_missing_edit_dirty_worktree_and_no_commits() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let out = run_blyte(root, &["edit-finish", "never-started"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("never-started.edit.json"), "stderr: {err}");
    assert!(!root.join(".wt/never-started.start.json").exists());
    assert!(!root.join(".wt/never-started.json").exists());

    let start_out = run_blyte(root, &["edit-start", "e-checks", "--task", "edit readme"]);
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-checks");

    let out = run_blyte(root, &["edit-finish", "e-checks"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no commits"), "stderr: {err}");
    assert!(!root.join(".wt/e-checks.start.json").exists());
    assert!(!root.join(".wt/e-checks.json").exists());

    std::fs::write(wt.join("untracked.txt"), "hi\n").unwrap();
    let out = run_blyte(root, &["edit-finish", "e-checks"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("uncommitted") || err.contains("untracked"),
        "stderr: {err}"
    );
    assert!(!root.join(".wt/e-checks.start.json").exists());
    assert!(!root.join(".wt/e-checks.json").exists());

    std::fs::remove_file(wt.join("untracked.txt")).unwrap();
    std::fs::write(wt.join("README.md"), "modified\n").unwrap();
    let out = run_blyte(root, &["edit-finish", "e-checks"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("uncommitted") || err.contains("untracked"),
        "stderr: {err}"
    );
    assert!(!root.join(".wt/e-checks.start.json").exists());
    assert!(!root.join(".wt/e-checks.json").exists());
}

#[test]
fn edit_finish_gate_failure_and_resume_after_fix() {
    let repo = setup_repo("[dispatch]\ngate = \"test -f ok.txt\"\n");
    let root = repo.path();

    let start_out = run_blyte(root, &["edit-start", "e-resume", "--task", "Add ok.txt"]);
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-resume");

    std::fs::write(wt.join("wrong.txt"), "not ok yet\n").unwrap();
    git(&wt, &["add", "wrong.txt"]);
    git(&wt, &["commit", "-m", "add wrong file"]);

    let fail_out = run_blyte(root, &["edit-finish", "e-resume"]);
    assert_eq!(fail_out.status.code(), Some(1));
    let failed: serde_json::Value =
        serde_json::from_slice(&fail_out.stdout).expect("gate failure json");
    assert_eq!(failed["id"], "e-resume");
    assert_eq!(failed["outcome"], "gate_failed");
    assert_eq!(failed["failing_step"], "test -f ok.txt");
    assert!(failed["output_tail"].is_string());
    assert!(!root.join(".wt/e-resume.start.json").exists());
    assert!(!root.join(".wt/e-resume.json").exists());

    std::fs::write(wt.join("ok.txt"), "now passing\n").unwrap();
    git(&wt, &["add", "ok.txt"]);
    git(&wt, &["commit", "-m", "add ok.txt"]);

    let pass_out = run_blyte(root, &["edit-finish", "e-resume"]);
    assert_eq!(
        pass_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&pass_out.stderr)
    );
    let summary: serde_json::Value =
        serde_json::from_slice(&pass_out.stdout).expect("summary json");
    assert_eq!(summary["outcome"], "verified");
    assert_eq!(summary["exit"], 0);
    assert!(root.join(".wt/e-resume.start.json").is_file());
    assert!(root.join(".wt/e-resume.json").is_file());
}

#[test]
fn edit_finish_pass_records_artifacts_tree_and_refuses_second_finish() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();
    let main_before = git(root, &["rev-parse", "HEAD"]);

    let start_out = run_blyte(
        root,
        &["edit-start", "e-pass", "--task", "Update README.md text"],
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-pass");

    std::fs::write(wt.join("README.md"), "hello world\n").unwrap();
    git(&wt, &["add", "README.md"]);
    git(&wt, &["commit", "-m", "update readme"]);

    let out = run_blyte(root, &["edit-finish", "e-pass", "--request", "req-42"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(summary["id"], "e-pass");
    assert_eq!(summary["outcome"], "verified");
    assert_eq!(summary["exit"], 0);
    assert_eq!(summary["branch"], "wip/e-pass");
    assert_eq!(summary["turns"], 0);
    assert_eq!(summary["cost_usd"], 0.0);
    assert_eq!(summary["final_message"], "Update README.md text");
    assert_eq!(summary["commits"].as_array().unwrap().len(), 1);
    assert_eq!(summary["commits"][0]["subject"], "update readme");
    assert_eq!(summary["changed_files"], serde_json::json!(["README.md"]));

    let start_raw = std::fs::read_to_string(root.join(".wt/e-pass.start.json")).unwrap();
    let start_val: serde_json::Value = serde_json::from_str(&start_raw).unwrap();
    assert_eq!(start_val["kind"], "edit");
    assert_eq!(start_val["request"], "req-42");
    assert_eq!(start_val["base_sha"], main_before);

    assert!(root.join(".wt/e-pass.json").is_file());
    assert!(root.join(".wt/e-pass.checks.json").is_file());

    let head_tree = blyte_cli::passed_trees::tree_of(&wt, "HEAD").unwrap();
    let key = blyte_cli::passed_trees::check_key("true", &BTreeMap::new());
    let now = blyte_cli::passed_trees::now_secs();
    assert!(blyte_cli::passed_trees::has_passed(root, &head_tree, &key, now).unwrap());

    assert_eq!(git(root, &["rev-parse", "HEAD"]), main_before);

    let again = run_blyte(root, &["edit-finish", "e-pass", "--request", "req-42"]);
    assert!(!again.status.success());
    let err = String::from_utf8_lossy(&again.stderr);
    assert!(err.contains("e-pass.json"), "stderr: {err}");
}

#[test]
fn edit_finish_skips_gate_when_tree_already_passed() {
    let counter_dir = TempDir::new().unwrap();
    let counter_file = counter_dir.path().join("gate.count");
    let gate_cmd = format!("echo run >> '{}'", counter_file.display());
    let repo = setup_repo(&format!("[dispatch]\ngate = \"{gate_cmd}\"\n"));
    let root = repo.path();

    let start1 = run_blyte(root, &["edit-start", "e-first", "--task", "Add note"]);
    assert!(start1.status.success());
    let wt1 = root.join(".wt/e-first");
    std::fs::write(wt1.join("note.txt"), "same content\n").unwrap();
    git(&wt1, &["add", "note.txt"]);
    git(&wt1, &["commit", "-m", "add note"]);
    let finish1 = run_blyte(root, &["edit-finish", "e-first"]);
    assert_eq!(finish1.status.code(), Some(0));

    let count_after_first = std::fs::read_to_string(&counter_file)
        .unwrap()
        .lines()
        .count();
    assert_eq!(count_after_first, 1);

    let start2 = run_blyte(root, &["edit-start", "e-second", "--task", "Add same note"]);
    assert!(start2.status.success());
    let wt2 = root.join(".wt/e-second");
    std::fs::write(wt2.join("note.txt"), "same content\n").unwrap();
    git(&wt2, &["add", "note.txt"]);
    git(&wt2, &["commit", "-m", "add same note"]);
    let finish2 = run_blyte(root, &["edit-finish", "e-second"]);
    assert_eq!(finish2.status.code(), Some(0));

    let count_after_second = std::fs::read_to_string(&counter_file)
        .unwrap()
        .lines()
        .count();
    assert_eq!(count_after_second, 1);
}

#[test]
fn edit_finish_recovers_after_crash_between_start_record_and_summary() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let start_out = run_blyte(root, &["edit-start", "e-crash", "--task", "Add file"]);
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-crash");
    std::fs::write(wt.join("added.txt"), "data\n").unwrap();
    git(&wt, &["add", "added.txt"]);
    git(&wt, &["commit", "-m", "add file"]);

    let args = EditFinishArgs {
        id: "e-crash".to_string(),
        request: Some("req-crash".to_string()),
        message: None,
    };
    let crash_hook = || Err("simulated crash before summary".to_string());
    let code = blyte_cli::direct_edit::run_edit_finish_in(
        root,
        root,
        Path::new(blyte_bin()),
        &args,
        Some(&crash_hook),
    );
    assert_eq!(code, 2);
    assert!(root.join(".wt/e-crash.start.json").is_file());
    assert!(!root.join(".wt/e-crash.json").exists());

    let retry = run_blyte(root, &["edit-finish", "e-crash", "--request", "req-crash"]);
    assert_eq!(
        retry.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert!(root.join(".wt/e-crash.json").is_file());
    let summary: serde_json::Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(summary["outcome"], "verified");
}

#[test]
fn edit_finish_integrates_with_request_readiness_and_combine() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let start_out = run_blyte(
        root,
        &["edit-start", "e-comb", "--task", "Add greeting file"],
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-comb");
    std::fs::write(wt.join("greeting.txt"), "hello\n").unwrap();
    git(&wt, &["add", "greeting.txt"]);
    git(&wt, &["commit", "-m", "add greeting file"]);
    let edit_commit_msg = git(&wt, &["show", "-s", "--format=%s", "HEAD"]);

    let finish_out = run_blyte(root, &["edit-finish", "e-comb", "--request", "req-comb"]);
    assert_eq!(
        finish_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&finish_out.stderr)
    );

    let readiness = blyte_cli::requests::request_readiness(root, "req-comb").unwrap();
    match &readiness {
        Readiness::Ready => {}
        Readiness::Waiting { reasons } => {
            assert!(
                reasons.iter().all(|r| r.contains("flagged")),
                "unexpected waiting reasons: {reasons:?}"
            );
        }
    }

    let mut out_buf = Vec::new();
    let mut err_buf = Vec::new();
    let code = blyte_cli::combine::combine_in(root, "req-comb", &mut out_buf, &mut err_buf);
    assert_eq!(
        code,
        0,
        "combine stderr: {}",
        String::from_utf8_lossy(&err_buf)
    );

    let record: CombineRecord = serde_json::from_slice(&out_buf).expect("combine record json");
    assert_eq!(record.state, CombineState::Combined);
    assert_eq!(record.check, CombineCheck::AlreadyPassed);
    assert_eq!(record.commits.len(), 1);
    let combined_subject = git(root, &["show", "-s", "--format=%s", &record.branch]);
    assert_eq!(combined_subject, edit_commit_msg);
}

#[test]
fn edit_start_honours_explicit_base_and_edit_finish_runs_comment_step_and_env() {
    let repo = setup_repo(
        "[dispatch]\ngate = \"test \\\"$EDIT_ENV_FLAG\\\" = ok\"\n\n[dispatch.env]\nEDIT_ENV_FLAG = \"ok\"\n",
    );
    let root = repo.path();
    let first_sha = git(root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("second.txt"), "second\n").unwrap();
    git(root, &["add", "second.txt"]);
    git(root, &["commit", "-m", "second commit"]);

    let start_out = run_blyte(
        root,
        &[
            "edit-start",
            "e-env",
            "--task",
            "Add lib.rs without comments",
            "--base",
            &first_sha,
        ],
    );
    assert!(
        start_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&start_out.stderr)
    );
    let started: serde_json::Value = serde_json::from_slice(&start_out.stdout).unwrap();
    assert_eq!(started["base_sha"], first_sha);
    let wt = root.join(".wt/e-env");
    assert!(!wt.join("second.txt").exists());

    std::fs::write(
        wt.join("sample.rs"),
        "pub fn f() -> i32 {\n    // bad comment\n    1\n}\n",
    )
    .unwrap();
    git(&wt, &["add", "sample.rs"]);
    git(&wt, &["commit", "-m", "add commented rust file"]);

    let fail_out = run_blyte(root, &["edit-finish", "e-env"]);
    assert_eq!(fail_out.status.code(), Some(1));
    let failed: serde_json::Value = serde_json::from_slice(&fail_out.stdout).unwrap();
    assert_eq!(failed["outcome"], "gate_failed");
    assert!(
        failed["failing_step"]
            .as_str()
            .unwrap()
            .contains("comments"),
        "expected comment step failure, got: {failed}"
    );

    std::fs::write(wt.join("sample.rs"), "pub fn f() -> i32 {\n    1\n}\n").unwrap();
    git(&wt, &["add", "sample.rs"]);
    git(&wt, &["commit", "-m", "remove comment"]);

    let pass_out = run_blyte(root, &["edit-finish", "e-env"]);
    assert_eq!(
        pass_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&pass_out.stderr)
    );
}

#[test]
fn edit_finish_commits_dirty_worktree_including_untracked_file_with_message() {
    let repo = setup_repo("[dispatch]\ngate = \"test -f new_file.txt\"\n");
    let root = repo.path();

    let start_out = run_blyte(
        root,
        &[
            "edit-start",
            "e-msg",
            "--task",
            "Update readme and add file",
        ],
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-msg");

    std::fs::write(wt.join("README.md"), "updated content\n").unwrap();
    std::fs::write(wt.join("new_file.txt"), "brand new\n").unwrap();

    let subject = "feat: update readme and add \"new_file\"";
    let finish_out = run_blyte(root, &["edit-finish", "e-msg", "--message", subject]);
    assert_eq!(
        finish_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&finish_out.stderr)
    );

    let head_subject = git(&wt, &["show", "-s", "--format=%s", "HEAD"]);
    assert_eq!(head_subject, subject);
    let committed_files = git(&wt, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(committed_files.lines().any(|l| l == "README.md"));
    assert!(committed_files.lines().any(|l| l == "new_file.txt"));

    let summary: serde_json::Value =
        serde_json::from_slice(&finish_out.stdout).expect("summary json");
    assert_eq!(summary["outcome"], "verified");
    assert_eq!(summary["commits"].as_array().unwrap().len(), 1);
    assert_eq!(summary["commits"][0]["subject"], subject);
    assert!(root.join(".wt/e-msg.json").is_file());
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(".wt/e-msg.json")).unwrap())
            .unwrap();
    assert_eq!(on_disk["commits"][0]["subject"], subject);
}

#[test]
fn edit_finish_rejects_dirty_worktree_without_message_and_empty_message() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let start_out = run_blyte(root, &["edit-start", "e-dirty", "--task", "Edit file"]);
    assert!(start_out.status.success());
    let started: serde_json::Value = serde_json::from_slice(&start_out.stdout).unwrap();
    let base_sha = started["base_sha"].as_str().unwrap().to_string();
    let wt = root.join(".wt/e-dirty");

    std::fs::write(wt.join("README.md"), "dirty change\n").unwrap();

    let no_msg = run_blyte(root, &["edit-finish", "e-dirty"]);
    assert_eq!(no_msg.status.code(), Some(2));
    let err = String::from_utf8_lossy(&no_msg.stderr);
    assert!(err.contains("--message"), "stderr: {err}");
    assert!(!root.join(".wt/e-dirty.start.json").exists());
    assert!(!root.join(".wt/e-dirty.json").exists());
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), base_sha);

    for empty in ["", "   \t\n"] {
        let empty_out = run_blyte(root, &["edit-finish", "e-dirty", "--message", empty]);
        assert_eq!(empty_out.status.code(), Some(2));
        let err = String::from_utf8_lossy(&empty_out.stderr);
        assert!(err.contains("--message"), "stderr: {err}");
        assert!(!root.join(".wt/e-dirty.start.json").exists());
        assert!(!root.join(".wt/e-dirty.json").exists());
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), base_sha);
    }
}

#[test]
fn edit_finish_commits_again_after_gate_failure_and_handles_clean_worktree_with_message() {
    let repo = setup_repo("[dispatch]\ngate = \"test -f pass.txt\"\n");
    let root = repo.path();

    let start_out = run_blyte(root, &["edit-start", "e-retry", "--task", "Add pass.txt"]);
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-retry");

    std::fs::write(wt.join("first.txt"), "attempt 1\n").unwrap();
    let fail_out = run_blyte(
        root,
        &["edit-finish", "e-retry", "--message", "feat: first attempt"],
    );
    assert_eq!(fail_out.status.code(), Some(1));
    let failed: serde_json::Value = serde_json::from_slice(&fail_out.stdout).unwrap();
    assert_eq!(failed["outcome"], "gate_failed");
    assert!(!root.join(".wt/e-retry.json").exists());
    assert_eq!(
        git(&wt, &["show", "-s", "--format=%s", "HEAD"]),
        "feat: first attempt"
    );

    std::fs::write(wt.join("pass.txt"), "attempt 2\n").unwrap();
    let pass_out = run_blyte(
        root,
        &["edit-finish", "e-retry", "--message", "fix: add pass.txt"],
    );
    assert_eq!(
        pass_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&pass_out.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&pass_out.stdout).unwrap();
    assert_eq!(summary["outcome"], "verified");
    let commits = summary["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0]["subject"], "feat: first attempt");
    assert_eq!(commits[1]["subject"], "fix: add pass.txt");

    let start2 = run_blyte(
        root,
        &["edit-start", "e-clean", "--task", "Already committed"],
    );
    assert!(start2.status.success());
    let wt2 = root.join(".wt/e-clean");
    let clean_no_commit = run_blyte(
        root,
        &[
            "edit-finish",
            "e-clean",
            "--message",
            "chore: nothing changed",
        ],
    );
    assert_eq!(clean_no_commit.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&clean_no_commit.stderr).contains("no commits"),
        "stderr: {}",
        String::from_utf8_lossy(&clean_no_commit.stderr)
    );

    std::fs::write(wt2.join("pass.txt"), "committed earlier\n").unwrap();
    git(&wt2, &["add", "pass.txt"]);
    git(&wt2, &["commit", "-m", "feat: earlier commit"]);
    let sha_before = git(&wt2, &["rev-parse", "HEAD"]);

    let clean_pass = run_blyte(
        root,
        &["edit-finish", "e-clean", "--message", "chore: ignored"],
    );
    assert_eq!(
        clean_pass.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&clean_pass.stderr)
    );
    assert_eq!(git(&wt2, &["rev-parse", "HEAD"]), sha_before);
    let summary2: serde_json::Value = serde_json::from_slice(&clean_pass.stdout).unwrap();
    let commits2 = summary2["commits"].as_array().unwrap();
    assert_eq!(commits2.len(), 1);
    assert_eq!(commits2[0]["subject"], "feat: earlier commit");
}

#[cfg(unix)]
#[test]
fn edit_finish_reports_hook_stderr_when_git_commit_is_refused() {
    use std::os::unix::fs::PermissionsExt;

    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let hook_path = root.join(".git/hooks/pre-commit");
    std::fs::write(
        &hook_path,
        "#!/bin/sh\necho \"pre-commit refused this change\" >&2\nexit 1\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&hook_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&hook_path, perms).unwrap();

    let start_out = run_blyte(root, &["edit-start", "e-hook", "--task", "Blocked by hook"]);
    assert!(start_out.status.success());
    let started: serde_json::Value = serde_json::from_slice(&start_out.stdout).unwrap();
    let base_sha = started["base_sha"].as_str().unwrap().to_string();
    let wt = root.join(".wt/e-hook");

    std::fs::write(wt.join("README.md"), "hook will reject\n").unwrap();

    let out = run_blyte(
        root,
        &["edit-finish", "e-hook", "--message", "feat: rejected"],
    );
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("git commit"), "stderr: {stderr}");
    assert!(
        stderr.contains("pre-commit refused this change"),
        "stderr: {stderr}"
    );
    assert!(!root.join(".wt/e-hook.start.json").exists());
    assert!(!root.join(".wt/e-hook.json").exists());
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), base_sha);
}

#[test]
fn configure_orchestrate_policy_allows_writes_only_while_direct_edit_is_open() {
    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let repo = tempfile::tempdir_in(tmp_base).expect("tempdir in target tmpdir");
    let root = repo.path();
    init_repo(root, "[dispatch]\ngate = \"true\"\n");

    let start_out = run_blyte(
        root,
        &["edit-start", "e-policy", "--task", "Write via tool"],
    );
    assert!(start_out.status.success());

    let _ = blyte_cli::task_runner::configure_orchestrate_policy(true, true, root, None, false)
        .expect("configure policy");
    let scope = blyte_agent::tools::write_scope().expect("write scope set");
    assert_eq!(
        scope.edit_worktrees,
        Some(root.join(".wt").canonicalize().unwrap())
    );

    let shell = blyte_agent::shell::LocalShell::new();
    let fs = blyte_agent::fs::LocalFs;
    let during = blyte_agent::tools::execute(
        root,
        &shell,
        &fs,
        "write",
        &serde_json::json!({ "path": ".wt/e-policy/tool_edit.txt", "content": "written\n" }),
    );
    assert!(!during.is_error, "{}", during.content);

    let finish_out = run_blyte(
        root,
        &[
            "edit-finish",
            "e-policy",
            "--message",
            "feat: write via tool",
        ],
    );
    assert_eq!(
        finish_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&finish_out.stderr)
    );

    let after = blyte_agent::tools::execute(
        root,
        &shell,
        &fs,
        "write",
        &serde_json::json!({ "path": ".wt/e-policy/after.txt", "content": "blocked\n" }),
    );
    assert!(after.is_error, "write after edit-finish must be refused");

    blyte_agent::tools::set_write_scope(None);
    blyte_agent::tools::set_approval(None);
}

struct IdleRunner;

impl PromptRunner for IdleRunner {
    fn run(
        &mut self,
        _prompt: &str,
        _model: &str,
        _emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        Ok(())
    }
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn make_idle_surface(root: PathBuf) -> Surface {
    let repo = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string();
    let session = Session::start(Box::new(IdleRunner), "test-model".to_string());
    Surface {
        root,
        repo,
        session,
        session_id: "sess-direct-edit-test".to_string(),
        started_at: now_epoch(),
        days: 14,
        token: generate_token().unwrap(),
        requests: Arc::new(RequestsState::new()),
        publisher: Arc::new(RealPublisher),
        reconcile_lock: Mutex::new(()),
    }
}

#[test]
fn edit_finish_passing_is_listed_by_live_runs_with_report_and_log() {
    let repo = setup_repo("[dispatch]\ngate = \"echo step-one-ok\\necho step-two-ok\"\n");
    let root = repo.path();

    let start_out = run_blyte(root, &["edit-start", "e-live", "--task", "Live run task"]);
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-live");
    std::fs::write(wt.join("live.txt"), "ok\n").unwrap();

    let finish_out = run_blyte(
        root,
        &[
            "edit-finish",
            "e-live",
            "--request",
            "req-live",
            "--message",
            "feat: live run",
        ],
    );
    assert_eq!(
        finish_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&finish_out.stderr)
    );

    let now = now_epoch() + 1;
    let live = blyte_cli::live_runs::collect(root, now, 14).unwrap();
    assert!(live.running.is_empty(), "running: {:?}", live.running);
    assert!(live.skipped.is_empty(), "skipped: {:?}", live.skipped);
    assert_eq!(live.runs.len(), 1, "runs: {:?}", live.runs);
    let run = &live.runs[0];
    assert_eq!(run.id, "e-live");
    assert_eq!(run.request.as_deref(), Some("req-live"));
    assert_eq!(run.outcome, "verified");
    assert!(!run.research);

    let report_path = root.join(".wt/e-live.report.json");
    let report_raw = std::fs::read_to_string(&report_path).expect("report.json exists");
    let record: RunRecord = serde_json::from_str(&report_raw).expect("parses as RunRecord");
    assert_eq!(record.outcome, "verified");
    assert_eq!(record.final_message.as_deref(), Some("Live run task"));
    assert_eq!(record.turns, 0);
    assert_eq!(record.cost_usd, Some(0.0));
    assert_eq!(record.gate_attempts, 1);
    assert_eq!(record.gate_ms.len(), 1);
    assert_eq!(record.gate_turns, vec![0]);
    assert!(record.model_requested.is_none());
    assert!(record.model_id.is_none());
    assert!(record.effort.is_none());
    assert!(record.gated);
    assert!(!record.sandboxed);
    assert!(record.gate_failures.is_empty());

    let log_path = root.join(".wt/e-live.log");
    let log = std::fs::read_to_string(&log_path).expect("log exists");
    let idx1_cmd = log.find("echo step-one-ok").expect("first step cmd in log");
    let idx1_out = log.find("step-one-ok").expect("first step output in log");
    let idx2_cmd = log
        .find("echo step-two-ok")
        .expect("second step cmd in log");
    let idx2_out = log[idx2_cmd..]
        .find("step-two-ok")
        .expect("second step output in log");
    assert!(idx1_cmd <= idx1_out);
    assert!(idx1_cmd < idx2_cmd);
    assert!(idx2_out > 0);
}

#[test]
fn edit_finish_stays_listed_when_start_file_pid_is_not_alive() {
    let repo = setup_repo("[dispatch]\ngate = \"echo pid-check\"\n");
    let root = repo.path();

    let start_out = run_blyte(
        root,
        &["edit-start", "e-deadpid", "--task", "Exited pid task"],
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-deadpid");
    std::fs::write(wt.join("dead.txt"), "ok\n").unwrap();

    let finish_out = run_blyte(
        root,
        &[
            "edit-finish",
            "e-deadpid",
            "--request",
            "req-deadpid",
            "--message",
            "feat: dead pid",
        ],
    );
    assert_eq!(finish_out.status.code(), Some(0));

    let mut exited = Command::new("true").spawn().expect("spawn true");
    let dead_pid = exited.id();
    let _ = exited.wait();

    let start_path = root.join(".wt/e-deadpid.start.json");
    let mut start_val: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&start_path).unwrap()).unwrap();
    start_val["pid"] = serde_json::json!(dead_pid);
    std::fs::write(
        &start_path,
        serde_json::to_string_pretty(&start_val).unwrap() + "\n",
    )
    .unwrap();

    let now = now_epoch() + 1;
    let live = blyte_cli::live_runs::collect(root, now, 14).unwrap();
    assert!(live.running.is_empty(), "running: {:?}", live.running);
    assert!(live.skipped.is_empty(), "skipped: {:?}", live.skipped);
    assert_eq!(live.runs.len(), 1, "runs: {:?}", live.runs);
    let run = &live.runs[0];
    assert_eq!(run.id, "e-deadpid");
    assert_eq!(run.request.as_deref(), Some("req-deadpid"));
    assert_eq!(run.outcome, "verified");
    assert!(!run.research);
}

#[test]
fn edit_finish_surface_maybe_combine_combines_request_branch() {
    let repo = setup_repo("[dispatch]\ngate = \"echo combine-ok\"\n");
    let root = repo.path();

    let start_out = run_blyte(
        root,
        &["edit-start", "e-surf", "--task", "Surface combine task"],
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-surf");
    std::fs::write(wt.join("surface.txt"), "combined content\n").unwrap();

    let finish_out = run_blyte(
        root,
        &[
            "edit-finish",
            "e-surf",
            "--request",
            "req-surf",
            "--message",
            "feat: surface combine commit",
        ],
    );
    assert_eq!(
        finish_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&finish_out.stderr)
    );

    let surface = make_idle_surface(root.to_path_buf());
    let started = maybe_combine(&surface, "req-surf").expect("maybe_combine");
    assert!(started, "maybe_combine must return true");
    surface.requests.wait_for_idle("req-surf");

    let rec_path = root.join(".wt/combined/req-surf.json");
    assert!(rec_path.is_file(), "combine record missing");
    let rec: CombineRecord =
        serde_json::from_str(&std::fs::read_to_string(&rec_path).unwrap()).unwrap();
    assert_eq!(rec.state, CombineState::Combined);
    assert_eq!(rec.branch, "blyte/req-surf");
    let combined_subject = git(root, &["show", "-s", "--format=%s", &rec.branch]);
    assert_eq!(combined_subject, "feat: surface combine commit");
    let combined_file = git(root, &["show", &format!("{}:surface.txt", rec.branch)]);
    assert_eq!(combined_file, "combined content");
}

#[test]
fn edit_finish_gate_failure_writes_and_replaces_log_without_report_start_or_summary() {
    let repo =
        setup_repo("[dispatch]\ngate = \"echo step-before-fail\\ncat fail_msg.txt >&2; exit 1\"\n");
    let root = repo.path();

    let start_out = run_blyte(
        root,
        &["edit-start", "e-fail-log", "--task", "Failing gate"],
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-fail-log");
    std::fs::write(wt.join("fail_msg.txt"), "first failure reason\n").unwrap();

    let fail1 = run_blyte(
        root,
        &[
            "edit-finish",
            "e-fail-log",
            "--message",
            "feat: first failing attempt",
        ],
    );
    assert_eq!(fail1.status.code(), Some(1));

    let log_path = root.join(".wt/e-fail-log.log");
    assert!(log_path.is_file());
    let log1 = std::fs::read_to_string(&log_path).unwrap();
    assert!(log1.contains("echo step-before-fail"), "log1: {log1}");
    assert!(log1.contains("step-before-fail"), "log1: {log1}");
    assert!(log1.contains("first failure reason"), "log1: {log1}");
    assert!(!root.join(".wt/e-fail-log.report.json").exists());
    assert!(!root.join(".wt/e-fail-log.start.json").exists());
    assert!(!root.join(".wt/e-fail-log.json").exists());

    std::fs::write(wt.join("fail_msg.txt"), "second failure reason\n").unwrap();
    let fail2 = run_blyte(
        root,
        &[
            "edit-finish",
            "e-fail-log",
            "--message",
            "feat: second failing attempt",
        ],
    );
    assert_eq!(fail2.status.code(), Some(1));

    let log2 = std::fs::read_to_string(&log_path).unwrap();
    assert!(log2.contains("second failure reason"), "log2: {log2}");
    assert!(!log2.contains("first failure reason"), "log2: {log2}");
    assert!(!root.join(".wt/e-fail-log.report.json").exists());
    assert!(!root.join(".wt/e-fail-log.start.json").exists());
    assert!(!root.join(".wt/e-fail-log.json").exists());
}

#[test]
fn edit_finish_already_passed_writes_one_line_log_and_is_listed() {
    let repo = setup_repo("[dispatch]\ngate = \"echo should-not-run\"\n");
    let root = repo.path();

    let start_out = run_blyte(
        root,
        &["edit-start", "e-skipped", "--task", "Already passed tree"],
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-skipped");
    std::fs::write(wt.join("already.txt"), "clean\n").unwrap();
    git(&wt, &["add", "already.txt"]);
    git(&wt, &["commit", "-m", "feat: already passed"]);

    let head_tree = blyte_cli::passed_trees::tree_of(&wt, "HEAD").unwrap();
    let key = blyte_cli::passed_trees::check_key("echo should-not-run", &BTreeMap::new());
    blyte_cli::passed_trees::record_passed(root, &head_tree, &key).unwrap();

    let finish_out = run_blyte(
        root,
        &["edit-finish", "e-skipped", "--request", "req-skipped"],
    );
    assert_eq!(
        finish_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&finish_out.stderr)
    );

    let log = std::fs::read_to_string(root.join(".wt/e-skipped.log")).expect("log exists");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 1, "expected one line log, got: {log:?}");
    assert!(
        lines[0].to_lowercase().contains("skipped")
            && lines[0].to_lowercase().contains("already passed"),
        "unexpected skipped log line: {:?}",
        lines[0]
    );
    assert!(!log.contains("should-not-run"), "log: {log}");

    let report_raw =
        std::fs::read_to_string(root.join(".wt/e-skipped.report.json")).expect("report exists");
    let record: RunRecord = serde_json::from_str(&report_raw).expect("parses as RunRecord");
    assert_eq!(record.outcome, "verified");
    assert_eq!(record.gate_attempts, 0);
    assert!(record.gate_ms.is_empty());

    let now = now_epoch() + 1;
    let live = blyte_cli::live_runs::collect(root, now, 14).unwrap();
    assert!(live.running.is_empty(), "running: {:?}", live.running);
    assert!(live.skipped.is_empty(), "skipped: {:?}", live.skipped);
    assert_eq!(live.runs.len(), 1, "runs: {:?}", live.runs);
    let run = &live.runs[0];
    assert_eq!(run.id, "e-skipped");
    assert_eq!(run.request.as_deref(), Some("req-skipped"));
    assert_eq!(run.outcome, "verified");
    assert!(!run.research);
}

#[test]
fn edit_finish_reports_error_naming_path_and_resumes_when_report_or_log_write_fails() {
    let repo = setup_repo("[dispatch]\ngate = \"echo ok\"\n");
    let root = repo.path();

    let start_out = run_blyte(
        root,
        &["edit-start", "e-write-err", "--task", "Write failure"],
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-write-err");
    std::fs::write(wt.join("file.txt"), "content\n").unwrap();

    let report_path = root.join(".wt/e-write-err.report.json");
    std::fs::create_dir_all(&report_path).unwrap();

    let fail_out = run_blyte(
        root,
        &[
            "edit-finish",
            "e-write-err",
            "--request",
            "req-write-err",
            "--message",
            "feat: write err",
        ],
    );
    assert_eq!(fail_out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&fail_out.stderr);
    assert!(
        stderr.contains("e-write-err.report.json"),
        "stderr must name report path: {stderr}"
    );
    assert!(!root.join(".wt/e-write-err.start.json").exists());
    assert!(!root.join(".wt/e-write-err.json").exists());

    std::fs::remove_dir(&report_path).unwrap();

    let log_path = root.join(".wt/e-write-err.log");
    std::fs::remove_file(&log_path).unwrap();
    std::fs::create_dir_all(&log_path).unwrap();

    let fail_log_out = run_blyte(
        root,
        &["edit-finish", "e-write-err", "--request", "req-write-err"],
    );
    assert_eq!(fail_log_out.status.code(), Some(2));
    let stderr_log = String::from_utf8_lossy(&fail_log_out.stderr);
    assert!(
        stderr_log.contains("e-write-err.log"),
        "stderr must name log path: {stderr_log}"
    );
    assert!(!root.join(".wt/e-write-err.start.json").exists());
    assert!(!root.join(".wt/e-write-err.json").exists());

    std::fs::remove_dir(&log_path).unwrap();

    let retry = run_blyte(
        root,
        &["edit-finish", "e-write-err", "--request", "req-write-err"],
    );
    assert_eq!(
        retry.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert!(log_path.is_file());
    assert!(report_path.is_file());
    assert!(root.join(".wt/e-write-err.start.json").is_file());
    assert!(root.join(".wt/e-write-err.json").is_file());

    let live = blyte_cli::live_runs::collect(root, now_epoch() + 1, 14).unwrap();
    assert_eq!(live.runs.len(), 1);
    assert_eq!(live.runs[0].id, "e-write-err");
    assert_eq!(live.runs[0].outcome, "verified");
}

#[test]
fn scope_allows_one_file_and_documentation_only() {
    let s = |xs: &[&str]| xs.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();

    assert_eq!(direct_edit_scope(&s(&[])), Ok(()));
    assert_eq!(direct_edit_scope(&s(&["src/a.rs"])), Ok(()));
    assert_eq!(
        direct_edit_scope(&s(&["docs/x.md", "README.md", "notes.TXT"])),
        Ok(())
    );
    assert_eq!(direct_edit_scope(&s(&["docs/a.png", "docs/b.md"])), Ok(()));

    let err_two_code = direct_edit_scope(&s(&["src/a.rs", "src/b.rs"])).unwrap_err();
    assert_eq!(
        err_two_code,
        "direct edits are for documentation or a change confined to one file; this edit changes 2 files (src/a.rs, src/b.rs): hand it to a run with blyte dispatch"
    );

    let err_mixed = direct_edit_scope(&s(&["src/a.rs", "README.md"])).unwrap_err();
    assert_eq!(
        err_mixed,
        "direct edits are for documentation or a change confined to one file; this edit changes 2 files (src/a.rs, README.md): hand it to a run with blyte dispatch"
    );
}

#[test]
fn a_two_file_code_edit_is_refused_before_the_gate() {
    let marker_dir = TempDir::new().unwrap();
    let marker = marker_dir.path().join("gate.ran");
    let gate_cmd = format!("touch '{}'", marker.display());
    let repo = setup_repo(&format!("[dispatch]\ngate = \"{gate_cmd}\"\n"));
    let root = repo.path();

    let start_out = run_blyte(root, &["edit-start", "e-scope", "--task", "Edit two files"]);
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-scope");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::write(wt.join("src/a.rs"), "pub fn a() -> i32 { 1 }\n").unwrap();
    std::fs::write(wt.join("src/b.rs"), "pub fn b() -> i32 { 2 }\n").unwrap();

    let finish_out = run_blyte(
        root,
        &["edit-finish", "e-scope", "--message", "feat: two files"],
    );
    assert_eq!(
        finish_out.status.code(),
        Some(2),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&finish_out.stdout),
        String::from_utf8_lossy(&finish_out.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&finish_out.stdout).expect("out_of_scope json");
    assert_eq!(parsed["id"], "e-scope");
    assert_eq!(parsed["outcome"], "out_of_scope");
    assert_eq!(
        parsed["changed_files"],
        serde_json::json!(["src/a.rs", "src/b.rs"])
    );
    assert_eq!(
        parsed["error"],
        "direct edits are for documentation or a change confined to one file; this edit changes 2 files (src/a.rs, src/b.rs): hand it to a run with blyte dispatch"
    );
    assert!(!marker.exists(), "gate must not run when out of scope");
    assert!(root.join(".wt/e-scope.edit.json").is_file());
    assert!(!root.join(".wt/e-scope.json").exists());
}

#[test]
fn a_direct_edit_is_always_reviewed() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let server = MockReviewServer::start(MockReviewReply::Ok(
        r#"{"concerns": [], "fallbacks": []}"#.to_string(),
    ));

    let start_out = run_blyte_with_review(
        root,
        &[
            "edit-start",
            "e-reviewed",
            "--task",
            "Improve the greeting text.",
        ],
        server.port,
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-reviewed");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::write(
        wt.join("src/app.rs"),
        "pub fn greet() -> &'static str { \"hello\" }\n",
    )
    .unwrap();

    let finish_out = run_blyte_with_review(
        root,
        &["edit-finish", "e-reviewed", "--message", "feat: greeting"],
        server.port,
    );
    assert_eq!(
        finish_out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&finish_out.stdout),
        String::from_utf8_lossy(&finish_out.stderr)
    );
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    assert!(root.join(".wt/e-reviewed.model-review.json").is_file());
    assert!(root.join(".wt/e-reviewed.json").is_file());
}

#[test]
fn review_findings_come_back_once_then_the_edit_finishes() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let review_reply = serde_json::json!({
        "concerns": [],
        "fallbacks": [{
            "file": "src/app.rs",
            "line": 1,
            "verdict": "fallback",
            "reason": "Substitutes a hardcoded region when missing."
        }]
    })
    .to_string();
    let server = MockReviewServer::start(MockReviewReply::Ok(review_reply));

    let start_out = run_blyte_with_review(
        root,
        &["edit-start", "e-findings", "--task", "Update `src/app.rs`."],
        server.port,
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-findings");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::write(
        wt.join("src/app.rs"),
        "pub fn f(x: Option<&str>) -> &str { x.unwrap_or(\"us\") }\n",
    )
    .unwrap();

    let first = run_blyte_with_review(
        root,
        &["edit-finish", "e-findings", "--message", "feat: add f"],
        server.port,
    );
    assert_eq!(
        first.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first_json: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("review_findings json");
    assert_eq!(first_json["id"], "e-findings");
    assert_eq!(first_json["outcome"], "review_findings");
    let findings = first_json["findings"].as_array().expect("findings array");
    assert_eq!(findings.len(), 1, "{first_json}");
    assert_eq!(findings[0]["file"], "src/app.rs");
    assert_eq!(findings[0]["line"], 1);
    assert_eq!(
        findings[0]["text"],
        "Substitutes a hardcoded region when missing."
    );
    assert!(!root.join(".wt/e-findings.json").exists());
    assert!(root.join(".wt/e-findings.edit.json").is_file());
    assert!(root.join(".wt/e-findings.fixround.json").is_file());
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);

    let second = run_blyte_with_review(
        root,
        &["edit-finish", "e-findings", "--message", "feat: retry f"],
        server.port,
    );
    assert_eq!(
        second.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(server.calls.load(Ordering::SeqCst), 2);
    assert!(root.join(".wt/e-findings.json").is_file());
    let checks: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join(".wt/e-findings.checks.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        checks["flags"],
        serde_json::json!(["fallbacks added: 1 (src/app.rs:1)"])
    );
}

#[test]
fn a_failed_review_is_recorded_not_passed() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let server = MockReviewServer::start(MockReviewReply::HttpError(
        500,
        r#"{"error":{"message":"review backend down"}}"#.to_string(),
    ));

    let start_out = run_blyte_with_review(
        root,
        &["edit-start", "e-rev-fail", "--task", "Update `src/app.rs`."],
        server.port,
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-rev-fail");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::write(
        wt.join("src/app.rs"),
        "pub fn greet() -> &'static str { \"hi\" }\n",
    )
    .unwrap();

    let finish_out = run_blyte_with_review(
        root,
        &["edit-finish", "e-rev-fail", "--message", "feat: greet"],
        server.port,
    );
    assert_eq!(
        finish_out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&finish_out.stdout),
        String::from_utf8_lossy(&finish_out.stderr)
    );
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    assert!(root.join(".wt/e-rev-fail.json").is_file());

    let checks: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join(".wt/e-rev-fail.checks.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(checks["model_review"]["status"].as_str(), Some("failed"));
    let flags: Vec<&str> = checks["flags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        flags.iter().any(|f| f.starts_with("model review failed:")),
        "{flags:?}"
    );
}
