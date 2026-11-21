use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn model_server() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&requests);
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
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
            counter.fetch_add(1, Ordering::SeqCst);
            let body = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;
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

fn dispatch_command(repo_dir: &Path, home: &Path, port: u16) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.current_dir(repo_dir)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("BLYTE_USAGE_LOG", "off")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
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

fn init_repo(dir: &Path) {
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(dir)
        .output()
        .unwrap();
    std::fs::write(dir.join("README.md"), "initial").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial", "--no-gpg-sign"])
        .current_dir(dir)
        .output()
        .unwrap();
}

#[test]
fn test_dispatch_failing_gate_stops_as_blocked_before_model() {
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
            "T_BASE_FAIL",
            task_file.to_str().unwrap(),
            "--gate",
            "exit 3",
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
    assert_eq!(summary["turns"], 0);
    assert_eq!(summary["exit"], 5);
    assert_eq!(requests.load(Ordering::SeqCst), 0);
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
            "2",
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
        .args(["commit", "-m", "add blyte.toml", "--no-gpg-sign"])
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
            "2",
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
