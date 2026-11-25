use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::OnceLock;

const VALID_SURFACE_ID: &str = "s0123456789abcdef";

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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
            git(root, &["init", "-q", "-b", "main"]);
            std::fs::write(root.join("f.txt"), "a\n").unwrap();
            std::fs::write(root.join("task.md"), "do the task").unwrap();
            std::fs::write(root.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n").unwrap();
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

fn branch_exists(repo: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(repo)
        .status()
        .unwrap()
        .success()
}

fn run_dispatch_cli(repo: &Path, id: &str, extra: &[&str]) -> Output {
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

fn mock_review_port() -> u16 {
    static PORT: OnceLock<u16> = OnceLock::new();
    *PORT.get_or_init(|| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let _ = read_http_body(&mut stream);
                let body = serde_json::json!({
                    "id": "rev-1",
                    "object": "chat.completion",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": r#"{"concerns": [], "fallbacks": []}"#,
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
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        port
    })
}

fn run_edit_cli(repo: &Path, args: &[&str]) -> Output {
    let home = repo.join(".home");
    let _ = std::fs::create_dir_all(&home);
    let port = mock_review_port();
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(repo)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("BLYTE_STATE_DIR", repo.join(".state"))
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
        .env("OPENAI_BASE_URL", format!("http://127.0.0.1:{port}/v1"))
        .env("OPENAI_API_KEY", "test")
        .env("BLYTE_MODEL", "openai/gemini-3.8-flash")
        .output()
        .unwrap()
}

#[test]
fn dispatch_with_surface_records_surface_id_in_start_record() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path();
    init_repo(repo);

    let out = run_dispatch_cli(repo, "S1", &["--surface", VALID_SURFACE_ID]);
    assert!(
        !out.stdout.is_empty(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/S1.start.json")).unwrap())
            .unwrap();
    assert_eq!(start["surface"], VALID_SURFACE_ID);
}

#[test]
fn dispatch_without_surface_records_null_surface_in_start_record() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path();
    init_repo(repo);

    let out = run_dispatch_cli(repo, "S2", &[]);
    assert!(
        !out.stdout.is_empty(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/S2.start.json")).unwrap())
            .unwrap();
    assert!(
        start.get("surface").is_some_and(|v| v.is_null()),
        "expected surface: null in {start}"
    );
}

#[test]
fn dispatch_rejects_invalid_surface_id_before_creating_anything() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path();
    init_repo(repo);

    let out = run_dispatch_cli(repo, "BAD1", &["--surface", "bad/id"]);
    assert!(!out.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("blyte: invalid surface id"),
        "expected invalid surface id error, got stderr={stderr:?}"
    );
    assert!(!repo.join(".wt/BAD1.start.json").exists());
    assert!(!repo.join(".wt/BAD1").exists());
    assert!(!branch_exists(repo, "wip/BAD1"));
}

#[test]
fn edit_finish_records_surface_and_rejects_invalid_surface_id() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path();
    init_repo(repo);

    let start_out = run_edit_cli(repo, &["edit-start", "E1", "--task", "Update f.txt"]);
    assert!(
        start_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&start_out.stderr)
    );
    let wt1 = repo.join(".wt/E1");
    std::fs::write(wt1.join("f.txt"), "b\n").unwrap();
    git(&wt1, &["add", "f.txt"]);
    git(&wt1, &["commit", "-qm", "update f", "--no-gpg-sign"]);

    let bad = run_edit_cli(repo, &["edit-finish", "E1", "--surface", "bad/id"]);
    assert!(!bad.status.success(), "expected non-zero exit");
    let bad_stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(
        bad_stderr.contains("blyte: invalid surface id"),
        "expected invalid surface id error, got stderr={bad_stderr:?}"
    );
    assert!(!repo.join(".wt/E1.start.json").exists());

    let finish_out = run_edit_cli(repo, &["edit-finish", "E1", "--surface", VALID_SURFACE_ID]);
    assert_eq!(
        finish_out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&finish_out.stderr)
    );
    let start_e1: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/E1.start.json")).unwrap())
            .unwrap();
    assert_eq!(start_e1["surface"], VALID_SURFACE_ID);

    let start_out2 = run_edit_cli(repo, &["edit-start", "E2", "--task", "Update f.txt again"]);
    assert!(
        start_out2.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&start_out2.stderr)
    );
    let wt2 = repo.join(".wt/E2");
    std::fs::write(wt2.join("f.txt"), "c\n").unwrap();
    git(&wt2, &["add", "f.txt"]);
    git(&wt2, &["commit", "-qm", "update f again", "--no-gpg-sign"]);

    let finish_out2 = run_edit_cli(repo, &["edit-finish", "E2"]);
    assert_eq!(
        finish_out2.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&finish_out2.stderr)
    );
    let start_e2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.join(".wt/E2.start.json")).unwrap())
            .unwrap();
    assert!(
        start_e2.get("surface").is_some_and(|v| v.is_null()),
        "expected surface: null in {start_e2}"
    );
}
