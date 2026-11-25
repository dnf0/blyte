use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use blyte_cli::combine::{combine_in, CombineCheck, CombineRecord, CombineState};
use blyte_cli::dispatch::{decide_base_check, record_passed_after_run, BaseCheckDecision};
use blyte_cli::passed_trees::{
    check_key, has_passed, record_passed, record_passed_at, record_path, tree_of,
    worktree_is_clean, MAX_AGE_SECS,
};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed:\nstdout: {}\nstderr: {}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn fast_import(dir: &Path, stream: &str) {
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git fast-import failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_recursive(&src_path, &dst_path);
        } else {
            std::fs::copy(&src_path, &dst_path).unwrap();
        }
    }
}

fn base_repo_template() -> &'static Path {
    static TEMPLATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            let d = tempfile::tempdir().unwrap();
            git(d.path(), &["init", "-q", "-b", "main"]);
            let config_path = d.path().join(".git/config");
            let mut cfg = std::fs::read_to_string(&config_path).unwrap();
            cfg.push_str("\n[user]\n\tname = Test\n\temail = test@example.com\n");
            std::fs::write(&config_path, cfg).unwrap();
            let stream = "commit refs/heads/main\ncommitter Test <test@example.com> 1700000000 +0000\ndata 4\ninit\nM 100644 inline README.md\ndata 6\nhello\n\n";
            fast_import(d.path(), stream);
            git(d.path(), &["reset", "--hard", "-q", "HEAD"]);
            d
        })
        .path()
}

fn init_repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    copy_dir_recursive(base_repo_template(), d.path());
    d
}

fn commit_file(dir: &Path, name: &str, content: &str, msg: &str) -> String {
    std::fs::write(dir.join(name), content).unwrap();
    git(dir, &["add", name]);
    git(
        dir,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qm",
            msg,
            "--no-gpg-sign",
        ],
    );
    git(dir, &["rev-parse", "HEAD"])
}

fn write_blyte_toml(root: &Path, gate: &str, setup: Option<&str>, base_check: Option<bool>) {
    let mut text = format!("[dispatch]\ngate = {:?}\n", gate);
    if let Some(s) = setup {
        text.push_str(&format!("setup = {:?}\n", s));
    }
    if let Some(bc) = base_check {
        text.push_str(&format!("base_check = {bc}\n"));
    }
    std::fs::write(root.join("blyte.toml"), text).unwrap();
}

fn add_verified_run(
    root: &Path,
    id: &str,
    request: &str,
    base_sha: &str,
    started_at: u64,
    file: &str,
    content: &str,
) -> String {
    let wt_parent = root.join(".wt");
    std::fs::create_dir_all(&wt_parent).unwrap();
    let branch = format!("wip/{id}");
    let msg = format!("change {id}");
    let stream = format!(
        "commit refs/heads/{branch}\ncommitter Test <test@example.com> 1700000001 +0000\ndata {}\n{msg}\nfrom {base_sha}\nM 100644 inline {file}\ndata {}\n{content}\n",
        msg.len(),
        content.len()
    );
    fast_import(root, &stream);
    let tip = std::fs::read_to_string(root.join(format!(".git/refs/heads/{branch}")))
        .unwrap()
        .trim()
        .to_string();

    let start = serde_json::json!({
        "id": id,
        "started_at": started_at,
        "base": base_sha,
        "base_sha": base_sha,
        "request": request,
    });
    std::fs::write(
        wt_parent.join(format!("{id}.start.json")),
        serde_json::to_string_pretty(&start).unwrap(),
    )
    .unwrap();

    let summary = serde_json::json!({
        "id": id,
        "branch": branch,
        "outcome": "verified",
        "exit": 0,
    });
    std::fs::write(
        wt_parent.join(format!("{id}.json")),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .unwrap();

    tip
}

fn count_lines(path: &Path) -> usize {
    if !path.exists() {
        return 0;
    }
    std::fs::read_to_string(path).unwrap().lines().count()
}

fn wait_for_marker(path: &Path, step: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "{step}: {}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn blyte_dispatch(dir: &Path, args: &[&str]) -> std::process::Output {
    let home = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("BLYTE_STATE_DIR", home.path().join("state"))
        .env("BLYTE_GATE_SLOT_DIR", home.path().join("slots"))
        .env("BLYTE_GATE_SLOTS", "16")
        .env("BLYTE_ENV_FILE", "")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_NO_KEEP_AWAKE", "1")
        .env("BLYTE_USAGE_LOG", "off")
        .env("GEMINI_API_KEY", "test-key")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

#[test]
fn passed_trees_record_lookup_expiry_corrupt_and_linked_worktree() {
    let repo = init_repo();
    let root = repo.path();

    let tree_a = tree_of(root, "HEAD").unwrap();
    let _c2 = commit_file(root, "b.txt", "b\n", "add b");
    let tree_b = tree_of(root, "HEAD").unwrap();
    assert_ne!(tree_a, tree_b);

    let mut env1 = BTreeMap::new();
    env1.insert("A".to_string(), "1".to_string());
    env1.insert("B".to_string(), "2".to_string());
    let mut env2 = BTreeMap::new();
    env2.insert("A".to_string(), "1".to_string());
    env2.insert("B".to_string(), "3".to_string());

    let key1 = check_key("cargo test", &env1);
    let key2 = check_key("cargo test --all", &env1);
    let key3 = check_key("cargo test", &env2);
    assert_eq!(key1.len(), 64);
    assert_ne!(key1, key2);
    assert_ne!(key1, key3);

    let now = 1_700_000_000u64;
    assert!(!has_passed(root, &tree_a, &key1, now).unwrap());

    record_passed_at(root, &tree_a, &key1, now).unwrap();
    assert!(has_passed(root, &tree_a, &key1, now).unwrap());
    assert!(has_passed(root, &tree_a, &key1, now + MAX_AGE_SECS).unwrap());
    assert!(!has_passed(root, &tree_b, &key1, now).unwrap());
    assert!(!has_passed(root, &tree_a, &key2, now).unwrap());
    assert!(!has_passed(root, &tree_a, &key1, now + 8 * 24 * 60 * 60).unwrap());

    let bad_path = record_path(root, &tree_b, &key1);
    std::fs::write(&bad_path, "{ not valid json").unwrap();
    let err = has_passed(root, &tree_b, &key1, now).unwrap_err();
    assert!(
        err.contains(&bad_path.display().to_string()),
        "error should name the bad file, got: {err}"
    );
    std::fs::remove_file(&bad_path).unwrap();

    let linked_parent = tempfile::tempdir().unwrap();
    let linked_wt = linked_parent.path().join("linked");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "linked-branch",
            linked_wt.to_str().unwrap(),
        ],
    );
    assert!(worktree_is_clean(&linked_wt).unwrap());
    record_passed_at(&linked_wt, &tree_b, &key2, now).unwrap();

    let expected_in_main = root
        .join(".wt")
        .join("passed")
        .join(format!("{tree_b}.{key2}.json"));
    assert!(expected_in_main.exists());
    assert!(!linked_wt.join(".wt").join("passed").exists());
    assert!(has_passed(&linked_wt, &tree_b, &key2, now).unwrap());
    assert!(has_passed(root, &tree_b, &key2, now).unwrap());

    std::fs::write(linked_wt.join("dirty.txt"), "x").unwrap();
    assert!(!worktree_is_clean(&linked_wt).unwrap());
}

#[test]
fn combine_skips_gate_when_single_run_tip_tree_already_passed() {
    let repo = init_repo();
    let root = repo.path();
    let counter_dir = tempfile::tempdir().unwrap();
    let counter = counter_dir.path().join("runs.log");
    let gate = format!("echo run >> {}", counter.display());
    write_blyte_toml(root, &gate, None, None);

    let base = git(root, &["rev-parse", "HEAD"]);
    let tip = add_verified_run(root, "T1", "req-skip", &base, 100, "a.txt", "a\n");
    let tip_tree = tree_of(root, &tip).unwrap();
    let key = check_key(&gate, &BTreeMap::new());
    record_passed(root, &tip_tree, &key).unwrap();

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = combine_in(root, "req-skip", &mut out, &mut err);
    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
    assert_eq!(count_lines(&counter), 0);

    let rec: CombineRecord = serde_json::from_slice(&out).unwrap();
    assert_eq!(rec.state, CombineState::Combined);
    assert_eq!(rec.check, CombineCheck::AlreadyPassed);
    let out_json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(out_json["check"], "already_passed");
    assert!(
        String::from_utf8_lossy(&err).contains("already_passed"),
        "stderr: {}",
        String::from_utf8_lossy(&err)
    );
}

#[test]
fn combine_runs_gate_when_unrecorded_and_records_combined_tree() {
    let repo = init_repo();
    let root = repo.path();
    let counter_dir = tempfile::tempdir().unwrap();
    let counter = counter_dir.path().join("runs.log");
    let gate = format!("echo run >> {}", counter.display());
    write_blyte_toml(root, &gate, None, None);

    let base = git(root, &["rev-parse", "HEAD"]);
    let tip = add_verified_run(root, "T1", "req-run", &base, 100, "a.txt", "a\n");
    let tip_tree = tree_of(root, &tip).unwrap();
    let key = check_key(&gate, &BTreeMap::new());
    assert!(!has_passed(root, &tip_tree, &key, blyte_cli::passed_trees::now_secs()).unwrap());

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = combine_in(root, "req-run", &mut out, &mut err);
    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
    assert_eq!(count_lines(&counter), 1);

    let rec: CombineRecord = serde_json::from_slice(&out).unwrap();
    assert_eq!(rec.state, CombineState::Combined);
    assert_eq!(rec.check, CombineCheck::Ran);
    let out_json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(out_json["check"], "ran");
    assert!(
        String::from_utf8_lossy(&err).contains("ran"),
        "stderr: {}",
        String::from_utf8_lossy(&err)
    );

    let combined_tree = tree_of(root, "blyte/req-run").unwrap();
    assert_eq!(combined_tree, tip_tree);
    assert!(has_passed(
        root,
        &combined_tree,
        &key,
        blyte_cli::passed_trees::now_secs()
    )
    .unwrap());
}

#[test]
fn combine_runs_gate_for_two_runs_with_unrecorded_combined_tree() {
    let repo = init_repo();
    let root = repo.path();
    let counter_dir = tempfile::tempdir().unwrap();
    let counter = counter_dir.path().join("runs.log");
    let gate = format!("echo run >> {}", counter.display());
    write_blyte_toml(root, &gate, None, None);

    let base = git(root, &["rev-parse", "HEAD"]);
    let tip1 = add_verified_run(root, "T1", "req-two", &base, 100, "a.txt", "a\n");
    let tip2 = add_verified_run(root, "T2", "req-two", &base, 200, "b.txt", "b\n");
    let key = check_key(&gate, &BTreeMap::new());
    record_passed(root, &tree_of(root, &tip1).unwrap(), &key).unwrap();
    record_passed(root, &tree_of(root, &tip2).unwrap(), &key).unwrap();

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = combine_in(root, "req-two", &mut out, &mut err);
    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
    assert_eq!(count_lines(&counter), 1);

    let rec: CombineRecord = serde_json::from_slice(&out).unwrap();
    assert_eq!(rec.state, CombineState::Combined);
    assert_eq!(rec.check, CombineCheck::Ran);
    let combined_tree = tree_of(root, "blyte/req-two").unwrap();
    assert!(has_passed(
        root,
        &combined_tree,
        &key,
        blyte_cli::passed_trees::now_secs()
    )
    .unwrap());
}

#[test]
fn combine_runs_gate_when_gate_text_differs_from_recorded_key() {
    let repo = init_repo();
    let root = repo.path();
    let counter_dir = tempfile::tempdir().unwrap();
    let counter = counter_dir.path().join("runs.log");
    let gate = format!("echo run >> {}", counter.display());
    write_blyte_toml(root, &gate, None, None);

    let base = git(root, &["rev-parse", "HEAD"]);
    let tip = add_verified_run(root, "T1", "req-diffgate", &base, 100, "a.txt", "a\n");
    let tip_tree = tree_of(root, &tip).unwrap();
    let other_key = check_key("different gate", &BTreeMap::new());
    record_passed(root, &tip_tree, &other_key).unwrap();

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = combine_in(root, "req-diffgate", &mut out, &mut err);
    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
    assert_eq!(count_lines(&counter), 1);

    let rec: CombineRecord = serde_json::from_slice(&out).unwrap();
    assert_eq!(rec.state, CombineState::Combined);
    assert_eq!(rec.check, CombineCheck::Ran);
}

#[test]
fn combine_runs_gate_when_combine_worktree_has_untracked_file() {
    let repo = init_repo();
    let root = repo.path();
    let counter_dir = tempfile::tempdir().unwrap();
    let counter = counter_dir.path().join("runs.log");
    let gate = format!("echo run >> {}", counter.display());
    write_blyte_toml(root, &gate, Some("touch untracked-in-combine.txt"), None);

    let base = git(root, &["rev-parse", "HEAD"]);
    let tip = add_verified_run(root, "T1", "req-dirty", &base, 100, "a.txt", "a\n");
    let tip_tree = tree_of(root, &tip).unwrap();
    let key = check_key(&gate, &BTreeMap::new());
    record_passed(root, &tip_tree, &key).unwrap();

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = combine_in(root, "req-dirty", &mut out, &mut err);
    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
    assert_eq!(count_lines(&counter), 1);

    let rec: CombineRecord = serde_json::from_slice(&out).unwrap();
    assert_eq!(rec.state, CombineState::Combined);
    assert_eq!(rec.check, CombineCheck::Ran);
}

#[test]
fn combine_parses_legacy_record_without_check_field_as_ran() {
    let legacy = serde_json::json!({
        "request": "req-old",
        "state": "combined",
        "branch": "blyte/req-old",
        "base": "abc",
        "runs": [],
        "commits": []
    });
    let rec: CombineRecord = serde_json::from_value(legacy).unwrap();
    assert_eq!(rec.check, CombineCheck::Ran);
}

#[test]
fn combine_errors_on_unparseable_passed_tree_record() {
    let repo = init_repo();
    let root = repo.path();
    let gate = "true";
    write_blyte_toml(root, gate, None, None);

    let base = git(root, &["rev-parse", "HEAD"]);
    let tip = add_verified_run(root, "T1", "req-corrupt", &base, 100, "a.txt", "a\n");
    let tip_tree = tree_of(root, &tip).unwrap();
    let key = check_key(gate, &BTreeMap::new());
    let bad_path = record_path(root, &tip_tree, &key);
    std::fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
    std::fs::write(&bad_path, "corrupt").unwrap();

    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = combine_in(root, "req-corrupt", &mut out, &mut err);
    assert_eq!(code, 2);
    let err_str = String::from_utf8_lossy(&err);
    assert!(
        err_str.contains(&bad_path.display().to_string()),
        "stderr: {err_str}"
    );
}

#[test]
fn dispatch_skips_base_check_when_base_tree_already_passed() {
    let repo = init_repo();
    let root = repo.path();
    let counter_dir = tempfile::tempdir().unwrap();
    let counter = counter_dir.path().join("base.log");
    let gate = format!("echo base >> {} && false", counter.display());
    write_blyte_toml(root, &gate, None, None);
    std::fs::write(root.join("task.md"), "Do work.\n").unwrap();

    let base_tree = tree_of(root, "HEAD").unwrap();
    let key = check_key(&gate, &BTreeMap::new());
    record_passed(root, &base_tree, &key).unwrap();

    let out = blyte_dispatch(
        root,
        &[
            "dispatch",
            "D1",
            "task.md",
            "--max-turns",
            "0",
            "--sandbox",
            "off",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(count_lines(&counter), 0);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("base check: this commit's content already passed"),
        "stderr: {stderr}"
    );

    let start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(".wt/D1.start.json")).unwrap())
            .unwrap();
    assert_eq!(start["base_check"], "already_passed");

    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_ne!(summary["outcome"], "blocked");
}

#[test]
fn dispatch_runs_base_check_when_base_tree_not_recorded() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let repo = init_repo();
    let root = repo.path();
    let counter_dir = tempfile::tempdir().unwrap();
    let counter = counter_dir.path().join("base.log");
    let done_marker = counter_dir.path().join("done.marker");
    let gate = format!(
        "if [ \"$(basename \"$PWD\")\" = \"D2.base\" ]; then echo base >> '{}'; echo done > '{}'; exit 1; fi; exit 0",
        counter.display(),
        done_marker.display()
    );
    write_blyte_toml(root, &gate, None, None);
    std::fs::write(root.join("task.md"), "Do work.\n").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let done_for_server = done_marker.clone();
    std::thread::spawn(move || {
        let mut first = true;
        for mut s in listener.incoming().flatten() {
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
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
            if first {
                first = false;
                wait_for_marker(&done_for_server, "base check never reached its gate step");
            }
            let body = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"bash","arguments":"{\"command\":\"true\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });

    let home = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "dispatch",
            "D2",
            "task.md",
            "--max-turns",
            "10",
            "--max-seconds",
            "60",
            "--sandbox",
            "off",
        ])
        .current_dir(root)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("BLYTE_STATE_DIR", home.path().join("state"))
        .env("BLYTE_GATE_SLOT_DIR", home.path().join("slots"))
        .env("BLYTE_GATE_SLOTS", "16")
        .env("BLYTE_ENV_FILE", "")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_NO_KEEP_AWAKE", "1")
        .env("BLYTE_USAGE_LOG", "off")
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
        .env("BLYTE_MODEL", "openai/gemini-3.8-flash")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(5),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(count_lines(&counter), 1);

    let start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(".wt/D2.start.json")).unwrap())
            .unwrap();
    assert_eq!(start["base_check"], true);

    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(summary["outcome"], "blocked");
    let base_tree = tree_of(root, "HEAD").unwrap();
    let key = check_key(&gate, &BTreeMap::new());
    assert!(!has_passed(root, &base_tree, &key, blyte_cli::passed_trees::now_secs()).unwrap());
}

#[test]
fn dispatch_no_base_check_and_config_false_write_false_even_when_recorded() {
    let repo_flag = init_repo();
    let root_flag = repo_flag.path();
    let repo_cfg = init_repo();
    let root_cfg = repo_cfg.path();
    let counter_dir = tempfile::tempdir().unwrap();
    let counter_flag = counter_dir.path().join("base_flag.log");
    let counter_cfg = counter_dir.path().join("base_cfg.log");
    let gate_flag = format!("echo base >> {} && false", counter_flag.display());
    let gate_cfg = format!("echo base >> {} && false", counter_cfg.display());
    write_blyte_toml(root_flag, &gate_flag, None, None);
    std::fs::write(root_flag.join("task.md"), "Do work.\n").unwrap();
    let base_tree_flag = tree_of(root_flag, "HEAD").unwrap();
    let key_flag = check_key(&gate_flag, &BTreeMap::new());
    record_passed(root_flag, &base_tree_flag, &key_flag).unwrap();

    write_blyte_toml(root_cfg, &gate_cfg, None, Some(false));
    std::fs::write(root_cfg.join("task.md"), "Do work.\n").unwrap();
    let base_tree_cfg = tree_of(root_cfg, "HEAD").unwrap();
    let key_cfg = check_key(&gate_cfg, &BTreeMap::new());
    record_passed(root_cfg, &base_tree_cfg, &key_cfg).unwrap();

    let (out_flag, out_cfg) = std::thread::scope(|s| {
        let h1 = s.spawn(|| {
            blyte_dispatch(
                root_flag,
                &[
                    "dispatch",
                    "D3",
                    "task.md",
                    "--no-base-check",
                    "--max-turns",
                    "0",
                    "--sandbox",
                    "off",
                ],
            )
        });
        let h2 = s.spawn(|| {
            blyte_dispatch(
                root_cfg,
                &[
                    "dispatch",
                    "D4",
                    "task.md",
                    "--max-turns",
                    "0",
                    "--sandbox",
                    "off",
                ],
            )
        });
        (h1.join().unwrap(), h2.join().unwrap())
    });

    assert_eq!(out_flag.status.code(), Some(1));
    assert_eq!(count_lines(&counter_flag), 0);
    let start_flag: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root_flag.join(".wt/D3.start.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(start_flag["base_check"], false);

    assert_eq!(out_cfg.status.code(), Some(1));
    assert_eq!(count_lines(&counter_cfg), 0);
    let start_cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root_cfg.join(".wt/D4.start.json")).unwrap())
            .unwrap();
    assert_eq!(start_cfg["base_check"], false);
}

#[test]
fn dispatch_stops_before_child_on_corrupt_passed_record_and_research_ignores_it() {
    let repo = init_repo();
    let root = repo.path();
    let counter_dir = tempfile::tempdir().unwrap();
    let counter = counter_dir.path().join("base.log");
    let gate = format!("echo base >> {}", counter.display());
    write_blyte_toml(root, &gate, None, None);
    std::fs::write(root.join("task.md"), "Do work.\n").unwrap();

    let base_tree = tree_of(root, "HEAD").unwrap();
    let key = check_key(&gate, &BTreeMap::new());
    let bad_path = record_path(root, &base_tree, &key);
    std::fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
    std::fs::write(&bad_path, "{ bad json").unwrap();

    let (out, out_research) = std::thread::scope(|s| {
        let h1 = s.spawn(|| {
            blyte_dispatch(
                root,
                &[
                    "dispatch",
                    "D5",
                    "task.md",
                    "--max-turns",
                    "0",
                    "--sandbox",
                    "off",
                ],
            )
        });
        let h2 = s.spawn(|| {
            blyte_dispatch(
                root,
                &[
                    "dispatch",
                    "R1",
                    "task.md",
                    "--research",
                    "--no-brief-check",
                    "--max-turns",
                    "0",
                    "--sandbox",
                    "off",
                ],
            )
        });
        (h1.join().unwrap(), h2.join().unwrap())
    });

    assert_eq!(out.status.code(), Some(2));
    assert_eq!(count_lines(&counter), 0);
    assert!(!root.join(".wt/D5.log").exists());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&bad_path.display().to_string()),
        "stderr: {stderr}"
    );

    assert_eq!(
        out_research.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out_research.stderr)
    );
    let start_research: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(".wt/R1.start.json")).unwrap())
            .unwrap();
    assert_eq!(start_research["base_check"], false);
}

#[test]
fn dispatch_decision_and_post_run_recording_rules() {
    let repo = init_repo();
    let root = repo.path();
    let base_sha = git(root, &["rev-parse", "HEAD"]);
    let base_tree = tree_of(root, &base_sha).unwrap();
    let gate = "cargo test";
    let env = BTreeMap::new();
    let key = check_key(gate, &env);
    let now = blyte_cli::passed_trees::now_secs();

    let wt = root.join(".wt/W1");
    std::fs::create_dir_all(root.join(".wt")).unwrap();
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/W1",
            wt.to_str().unwrap(),
            &base_sha,
        ],
    );

    let (dec, bt) =
        decide_base_check(root, &wt, &base_sha, gate, &env, false, false, None, now).unwrap();
    assert_eq!(dec, BaseCheckDecision::Run);
    assert_eq!(bt.as_deref(), Some(base_tree.as_str()));

    let warnings = record_passed_after_run(
        root,
        &wt,
        false,
        BaseCheckDecision::Run,
        Some(&base_tree),
        Some(&key),
        "blocked",
        Some(false),
    );
    assert!(warnings.is_empty());
    assert!(!has_passed(root, &base_tree, &key, now).unwrap());

    let warnings = record_passed_after_run(
        root,
        &wt,
        false,
        BaseCheckDecision::Run,
        Some(&base_tree),
        Some(&key),
        "budget_exhausted",
        None,
    );
    assert!(warnings.is_empty());
    assert!(!has_passed(root, &base_tree, &key, now).unwrap());

    let tip_sha = commit_file(&wt, "work.txt", "done\n", "implement");
    let tip_tree = tree_of(&wt, &tip_sha).unwrap();

    let warnings = record_passed_after_run(
        root,
        &wt,
        true,
        BaseCheckDecision::Disabled,
        None,
        Some(&key),
        "verified",
        None,
    );
    assert!(warnings.is_empty());
    assert!(!has_passed(root, &tip_tree, &key, now).unwrap());

    std::fs::write(wt.join("work.txt"), "uncommitted\n").unwrap();
    let warnings = record_passed_after_run(
        root,
        &wt,
        false,
        BaseCheckDecision::Disabled,
        None,
        Some(&key),
        "verified",
        None,
    );
    assert!(warnings.is_empty());
    assert!(!has_passed(root, &tip_tree, &key, now).unwrap());

    git(&wt, &["checkout", "--", "work.txt"]);
    assert!(worktree_is_clean(&wt).unwrap());
    let warnings = record_passed_after_run(
        root,
        &wt,
        false,
        BaseCheckDecision::Run,
        Some(&base_tree),
        Some(&key),
        "verified",
        Some(true),
    );
    assert!(warnings.is_empty());
    assert!(has_passed(root, &base_tree, &key, now).unwrap());
    assert!(has_passed(root, &tip_tree, &key, now).unwrap());

    let (dec_after, _) =
        decide_base_check(root, &wt, &base_sha, gate, &env, false, false, None, now).unwrap();
    assert_eq!(dec_after, BaseCheckDecision::AlreadyPassed);
    assert_eq!(dec_after.as_json(), serde_json::json!("already_passed"));

    let (dec_no_flag, _) =
        decide_base_check(root, &wt, &base_sha, gate, &env, false, true, None, now).unwrap();
    assert_eq!(dec_no_flag, BaseCheckDecision::Disabled);
    assert_eq!(dec_no_flag.as_json(), serde_json::json!(false));

    let (dec_cfg_false, _) = decide_base_check(
        root,
        &wt,
        &base_sha,
        gate,
        &env,
        false,
        false,
        Some(false),
        now,
    )
    .unwrap();
    assert_eq!(dec_cfg_false, BaseCheckDecision::Disabled);

    let repo2 = init_repo();
    let root2 = repo2.path();
    std::fs::create_dir_all(root2.join(".wt")).unwrap();
    std::fs::write(root2.join(".wt/passed"), "not a directory").unwrap();
    let warn = record_passed_after_run(
        root2,
        root2,
        false,
        BaseCheckDecision::Run,
        Some(&base_tree),
        Some(&key),
        "verified",
        Some(true),
    );
    assert!(!warn.is_empty());
}
