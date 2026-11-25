use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use blyte_cli::passed_trees::{check_key, has_passed, now_secs, tree_of};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const HANDSHAKE_POLL: Duration = Duration::from_millis(20);

const TOOL_CALL_BODY: &str = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"bash","arguments":"{\"command\":\"true\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;
const COMMIT_CALL_BODY: &str = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"bash","arguments":"{\"command\":\"echo ok > change.txt && git add change.txt && git commit -m change --no-gpg-sign\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;
const DONE_BODY: &str = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;

fn model_server_with<F>(handler: F) -> (u16, Arc<AtomicUsize>)
where
    F: Fn(usize) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&requests);
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
            let mut buf = vec![0u8; 65536];
            let mut got = Vec::new();
            loop {
                let n = s.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got);
                if let Some(i) = text.find("\r\n\r\n") {
                    let len = text[..i]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if got.len() >= i + 4 + len {
                        break;
                    }
                }
            }
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            let body = handler(n);
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });
    (port, requests)
}

fn model_server() -> (u16, Arc<AtomicUsize>) {
    model_server_with(|_| DONE_BODY.to_string())
}

fn wait_for_marker(path: &Path, step: &str) {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    while !path.exists() {
        assert!(Instant::now() < deadline, "{step}: {}", path.display());
        std::thread::sleep(HANDSHAKE_POLL);
    }
}

fn release_gate(release: &Path, ack: &Path) {
    std::fs::write(release, "go\n").unwrap();
    wait_for_marker(ack, "base check never took the release");
}

fn wait_for_release_sh(release: &Path, ack: &Path) -> String {
    format!(
        "i=0; while [ ! -f '{}' ]; do i=$((i+1)); [ \"$i\" -gt 750 ] && exit 99; sleep 0.02; done; echo ok > '{}'",
        release.display(),
        ack.display()
    )
}

fn dispatch_command(repo_dir: &Path, home: &Path, port: u16) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.current_dir(repo_dir)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("BLYTE_STATE_DIR", home.join("state"))
        .env("BLYTE_GATE_SLOT_DIR", home.join("slots"))
        .env("BLYTE_GATE_SLOTS", "16")
        .env("BLYTE_ENV_FILE", "")
        .env("BLYTE_USAGE_LOG", "off")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_NO_KEEP_AWAKE", "1")
        .env("BLYTE_MODEL_TIMEOUT_SECS", "10")
        .env("BLYTE_RETRY_DELAY_MS", "10")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_HOST")
        .env_remove("OLLAMA_API_KEY")
        .env("OPENAI_BASE_URL", format!("http://127.0.0.1:{port}/v1"))
        .env("OPENAI_API_KEY", "test")
        .env("BLYTE_MODEL", "openai/gemini-3.8-flash");
    cmd
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

fn base_repo_template() -> &'static Path {
    static TEMPLATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path();
            Command::new("git")
                .args(["init", "-b", "main"])
                .current_dir(p)
                .output()
                .unwrap();
            Command::new("git")
                .args(["config", "user.email", "test@example.com"])
                .current_dir(p)
                .output()
                .unwrap();
            Command::new("git")
                .args(["config", "user.name", "Test User"])
                .current_dir(p)
                .output()
                .unwrap();
            std::fs::write(p.join("README.md"), "initial").unwrap();
            Command::new("git")
                .args(["add", "."])
                .current_dir(p)
                .output()
                .unwrap();
            Command::new("git")
                .args([
                    "-c",
                    "maintenance.auto=false",
                    "-c",
                    "gc.auto=0",
                    "commit",
                    "-m",
                    "initial",
                    "--no-gpg-sign",
                ])
                .current_dir(p)
                .output()
                .unwrap();
            dir
        })
        .path()
}

fn init_repo(dir: &Path) {
    copy_dir_all(base_repo_template(), dir);
}

#[test]
fn test_dispatch_failing_gate_stops_as_blocked_before_model() {
    let temp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    let task_file = repo_dir.join("task.md");
    std::fs::write(&task_file, "Do something").unwrap();

    let (port, _requests) = model_server_with(|_| TOOL_CALL_BODY.to_string());
    let out = dispatch_command(repo_dir, home.path(), port)
        .args([
            "dispatch",
            "T_BASE_FAIL",
            task_file.to_str().unwrap(),
            "--gate",
            "exit 3",
            "--max-seconds",
            "60",
            "--sandbox",
            "off",
        ])
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(5),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let summary: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "Failed to parse JSON: {}\nStdout: {}\nStderr: {}",
            e,
            stdout,
            String::from_utf8_lossy(&out.stderr)
        )
    });

    assert_eq!(summary["outcome"], "blocked");
    assert_eq!(summary["exit"], 5);
    assert!(summary["base_check_seconds"].as_f64().is_some());
    assert!(!repo_dir.join(".wt/T_BASE_FAIL.base").exists());
    let final_msg = summary["final_message"].as_str().unwrap_or("");
    assert!(
        final_msg.contains("--no-base-check"),
        "expected final_message to mention --no-base-check, got: {final_msg}"
    );
    let err_msg = summary["error"].as_str().unwrap_or("");
    assert!(
        err_msg.contains("the check fails before any change"),
        "expected error to mention 'the check fails before any change', got: {err_msg}"
    );
    assert!(
        !err_msg.contains('\n'),
        "expected summary error to be a single line, got: {err_msg}"
    );

    let start_json_str =
        std::fs::read_to_string(repo_dir.join(".wt/T_BASE_FAIL.start.json")).unwrap();
    let start_json: serde_json::Value = serde_json::from_str(&start_json_str).unwrap();
    assert_eq!(start_json["base_check"], true);
}

#[test]
fn test_dispatch_no_base_check_flag_skips_base_check() {
    let temp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    let task_file = repo_dir.join("task.md");
    std::fs::write(&task_file, "Do something").unwrap();

    let (port, requests) = model_server();
    let out = dispatch_command(repo_dir, home.path(), port)
        .args([
            "dispatch",
            "T_NO_BASE_CHECK",
            task_file.to_str().unwrap(),
            "--gate",
            "exit 3",
            "--no-base-check",
            "--max-turns",
            "1",
            "--max-seconds",
            "60",
            "--sandbox",
            "off",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let summary: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "Failed to parse JSON: {}\nStdout: {}\nStderr: {}",
            e,
            stdout,
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_ne!(summary["outcome"], "blocked");
    assert!(
        summary["turns"].as_u64().unwrap_or(0) >= 1,
        "expected at least 1 turn, got: {summary}"
    );
    assert!(
        requests.load(Ordering::SeqCst) >= 1,
        "expected at least 1 model request"
    );

    let start_json_str =
        std::fs::read_to_string(repo_dir.join(".wt/T_NO_BASE_CHECK.start.json")).unwrap();
    let start_json: serde_json::Value = serde_json::from_str(&start_json_str).unwrap();
    assert_eq!(start_json["base_check"], false);
}

#[test]
fn test_dispatch_config_base_check_false_skips_base_check() {
    let temp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    std::fs::write(
        repo_dir.join("blyte.toml"),
        "[dispatch]\nbase_check = false\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", "blyte.toml"])
        .current_dir(repo_dir)
        .output()
        .unwrap();
    Command::new("git")
        .args([
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-m",
            "add blyte.toml",
            "--no-gpg-sign",
        ])
        .current_dir(repo_dir)
        .output()
        .unwrap();

    let task_file = repo_dir.join("task.md");
    std::fs::write(&task_file, "Do something").unwrap();

    let (port, requests) = model_server();
    let out = dispatch_command(repo_dir, home.path(), port)
        .args([
            "dispatch",
            "T_CFG_NO_BASE_CHECK",
            task_file.to_str().unwrap(),
            "--gate",
            "exit 3",
            "--max-turns",
            "1",
            "--max-seconds",
            "60",
            "--sandbox",
            "off",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let summary: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "Failed to parse JSON: {}\nStdout: {}\nStderr: {}",
            e,
            stdout,
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_ne!(summary["outcome"], "blocked");
    assert!(
        summary["turns"].as_u64().unwrap_or(0) >= 1,
        "expected at least 1 turn, got: {summary}"
    );
    assert!(
        requests.load(Ordering::SeqCst) >= 1,
        "expected at least 1 model request"
    );

    let start_json_str =
        std::fs::read_to_string(repo_dir.join(".wt/T_CFG_NO_BASE_CHECK.start.json")).unwrap();
    let start_json: serde_json::Value = serde_json::from_str(&start_json_str).unwrap();
    assert_eq!(start_json["base_check"], false);
}

#[test]
fn test_dispatch_first_model_call_happens_before_base_check_ends() {
    let temp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let sync = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    let task_file = repo_dir.join("task.md");
    std::fs::write(&task_file, "Do something").unwrap();

    let entered_marker = sync.path().join("entered");
    let release_marker = sync.path().join("release");
    let ack_marker = sync.path().join("released");
    let done_file = sync.path().join("base_done");

    let gate = format!(
        "if [ \"$(basename \"$PWD\")\" = \"T_PARALLEL.base\" ]; then echo started > '{}'; {}; echo done > '{}'; fi; exit 0",
        entered_marker.display(),
        wait_for_release_sh(&release_marker, &ack_marker),
        done_file.display()
    );

    let model_before_base_done = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&model_before_base_done);
    let entered_for_server = entered_marker.clone();
    let release_for_server = release_marker.clone();
    let ack_for_server = ack_marker.clone();
    let done_for_server = done_file.clone();

    let (port, requests) = model_server_with(move |n| {
        if n == 1 {
            wait_for_marker(
                &entered_for_server,
                "base check never reached its gate step",
            );
            flag.store(!done_for_server.exists(), Ordering::SeqCst);
            release_gate(&release_for_server, &ack_for_server);
            COMMIT_CALL_BODY.to_string()
        } else {
            DONE_BODY.to_string()
        }
    });

    let out = dispatch_command(repo_dir, home.path(), port)
        .args([
            "dispatch",
            "T_PARALLEL",
            task_file.to_str().unwrap(),
            "--gate",
            &gate,
            "--max-seconds",
            "60",
            "--sandbox",
            "off",
        ])
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(model_before_base_done.load(Ordering::SeqCst));
    assert!(done_file.exists());
    assert_eq!(requests.load(Ordering::SeqCst), 2);

    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(summary["outcome"], "verified");
    assert!(summary["base_check_seconds"].as_f64().is_some());
    assert!(!repo_dir.join(".wt/T_PARALLEL.base").exists());

    let base_tree = tree_of(repo_dir, "HEAD").unwrap();
    let key = check_key(&gate, &BTreeMap::new());
    assert!(has_passed(repo_dir, &base_tree, &key, now_secs()).unwrap());
}

#[test]
fn test_dispatch_failing_parallel_base_gate_blocks_and_cleans_up() {
    let temp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let sync = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    let task_file = repo_dir.join("task.md");
    std::fs::write(&task_file, "Do something").unwrap();

    let entered_marker = sync.path().join("entered");
    let release_marker = sync.path().join("release");
    let ack_marker = sync.path().join("released");

    let gate = format!(
        "if [ \"$(basename \"$PWD\")\" = \"T_PAR_FAIL.base\" ]; then echo started > '{}'; {}; echo 'broken on base'; exit 3; fi; exit 0",
        entered_marker.display(),
        wait_for_release_sh(&release_marker, &ack_marker)
    );

    let entered_for_server = entered_marker.clone();
    let release_for_server = release_marker.clone();
    let ack_for_server = ack_marker.clone();
    let (port, requests) = model_server_with(move |n| {
        if n == 1 {
            wait_for_marker(
                &entered_for_server,
                "base check never reached its gate step",
            );
            release_gate(&release_for_server, &ack_for_server);
        }
        TOOL_CALL_BODY.to_string()
    });

    let out = dispatch_command(repo_dir, home.path(), port)
        .args([
            "dispatch",
            "T_PAR_FAIL",
            task_file.to_str().unwrap(),
            "--gate",
            &gate,
            "--max-turns",
            "20",
            "--max-seconds",
            "60",
            "--sandbox",
            "off",
        ])
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(5),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(requests.load(Ordering::SeqCst) >= 1);

    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(summary["outcome"], "blocked");
    assert!(summary["base_check_seconds"].as_f64().is_some());
    let final_msg = summary["final_message"].as_str().unwrap_or("");
    assert!(final_msg.contains("broken on base"), "got: {final_msg}");
    assert!(final_msg.contains("--no-base-check"), "got: {final_msg}");
    assert!(!repo_dir.join(".wt/T_PAR_FAIL.base").exists());

    let base_tree = tree_of(repo_dir, "HEAD").unwrap();
    let key = check_key(&gate, &BTreeMap::new());
    assert!(!has_passed(repo_dir, &base_tree, &key, now_secs()).unwrap());
}

#[test]
fn test_dispatch_pending_base_check_stopped_without_orphan_process_and_removes_base_wt() {
    let temp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let sync = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    let task_file = repo_dir.join("task.md");
    std::fs::write(&task_file, "Do something").unwrap();

    let entered_marker = sync.path().join("entered");
    let pid_file = sync.path().join("child.pid");

    let gate = format!(
        "if [ \"$(basename \"$PWD\")\" = \"T_PENDING_STOP.base\" ]; then sleep 60 & CHILD=$!; echo \"$CHILD\" > '{}'; echo started > '{}'; wait \"$CHILD\"; fi; exit 0",
        pid_file.display(),
        entered_marker.display()
    );

    let entered_for_server = entered_marker.clone();
    let (port, _requests) = model_server_with(move |n| {
        if n == 1 {
            wait_for_marker(
                &entered_for_server,
                "base check never reached its gate step",
            );
        }
        TOOL_CALL_BODY.to_string()
    });

    let out = dispatch_command(repo_dir, home.path(), port)
        .args([
            "dispatch",
            "T_PENDING_STOP",
            task_file.to_str().unwrap(),
            "--gate",
            &gate,
            "--max-turns",
            "1",
            "--max-seconds",
            "60",
            "--sandbox",
            "off",
        ])
        .output()
        .unwrap();

    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "Failed to parse JSON: {e}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(summary["outcome"], "budget_exhausted");
    assert!(summary["base_check_seconds"].is_null());
    assert!(!repo_dir.join(".wt/T_PENDING_STOP.base").exists());

    let child_pid = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .to_string();
    assert!(!child_pid.is_empty());
    let alive = Command::new("kill")
        .args(["-0", &child_pid])
        .status()
        .unwrap()
        .success();
    assert!(
        !alive,
        "expected base check child process {child_pid} to be stopped"
    );
}

#[test]
fn test_dispatch_failing_base_check_when_change_passes_verifies_with_notice_and_skips_base_record()
{
    let temp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let sync = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    let task_file = repo_dir.join("task.md");
    std::fs::write(&task_file, "Fix it").unwrap();

    let entered_marker = sync.path().join("entered");
    let release_marker = sync.path().join("release");
    let ack_marker = sync.path().join("released");

    let gate = format!(
        "if [ \"$(basename \"$PWD\")\" = \"T_FIXES_BASE.base\" ]; then echo started > '{}'; {}; exit 3; fi; exit 0",
        entered_marker.display(),
        wait_for_release_sh(&release_marker, &ack_marker)
    );

    let entered_for_server = entered_marker.clone();
    let release_for_server = release_marker.clone();
    let ack_for_server = ack_marker.clone();
    let (port, _requests) = model_server_with(move |n| {
        if n == 1 {
            wait_for_marker(
                &entered_for_server,
                "base check never reached its gate step",
            );
            COMMIT_CALL_BODY.to_string()
        } else if n == 2 {
            release_gate(&release_for_server, &ack_for_server);
            DONE_BODY.to_string()
        } else {
            DONE_BODY.to_string()
        }
    });

    let out = dispatch_command(repo_dir, home.path(), port)
        .args([
            "dispatch",
            "T_FIXES_BASE",
            task_file.to_str().unwrap(),
            "--gate",
            &gate,
            "--max-seconds",
            "60",
            "--sandbox",
            "off",
        ])
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(summary["outcome"], "verified");
    assert!(summary["base_check_seconds"].as_f64().is_some());
    let final_msg = summary["final_message"].as_str().unwrap_or("");
    assert!(
        final_msg.contains("the check failed on the untouched code but passes with this change"),
        "got: {final_msg}"
    );
    assert!(!repo_dir.join(".wt/T_FIXES_BASE.base").exists());

    let base_tree = tree_of(repo_dir, "HEAD").unwrap();
    let key = check_key(&gate, &BTreeMap::new());
    assert!(!has_passed(repo_dir, &base_tree, &key, now_secs()).unwrap());
}
