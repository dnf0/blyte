use clap::{CommandFactory, Parser};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn fast_import(dir: &Path, script: &str) {
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git fast-import failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git(dir: &Path, args: &[&str]) {
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
}

fn write_file(path: &Path, content: &str) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn setup_repo_with_runs(merged_ids: &[&str], unmerged_ids: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    git(root, &["init", "-q", "-b", "main"]);
    let mut script = String::from(
        "commit refs/heads/main\nmark :1\ncommitter Test User <test@example.com> 1700000000 +0000\ndata 4\ninit\nM 100644 inline README.md\ndata 6\nhello\n\n",
    );
    let mut next_mark = 2usize;
    let mut main_mark = 1usize;

    for id in merged_ids {
        let br_mark = next_mark;
        next_mark += 1;
        let msg = format!("feat: {id}");
        let body = format!("work {id}\n");
        script.push_str(&format!(
            "commit refs/heads/wip/{id}\nmark :{br_mark}\ncommitter Test User <test@example.com> 1700000000 +0000\ndata {}\n{msg}\nfrom :{main_mark}\nM 100644 inline {id}.txt\ndata {}\n{body}\n",
            msg.len(),
            body.len()
        ));
        let merge_mark = next_mark;
        next_mark += 1;
        script.push_str(&format!(
            "commit refs/heads/main\nmark :{merge_mark}\ncommitter Test User <test@example.com> 1700000000 +0000\ndata 5\nmerge\nfrom :{main_mark}\nmerge :{br_mark}\nM 100644 inline {id}.txt\ndata {}\n{body}\n",
            body.len()
        ));
        main_mark = merge_mark;
    }

    for id in unmerged_ids {
        let br_mark = next_mark;
        next_mark += 1;
        let msg = format!("feat: {id}");
        let body = format!("unmerged work {id}\n");
        script.push_str(&format!(
            "commit refs/heads/wip/{id}\nmark :{br_mark}\ncommitter Test User <test@example.com> 1700000000 +0000\ndata {}\n{msg}\nfrom :{main_mark}\nM 100644 inline {id}.txt\ndata {}\n{body}\n",
            msg.len(),
            body.len()
        ));
    }

    fast_import(root, &script);

    let wt_dir = root.join(".wt");
    for id in merged_ids {
        let wt_path = wt_dir.join(id);
        let branch = format!("wip/{id}");
        git(
            root,
            &["worktree", "add", "-q", wt_path.to_str().unwrap(), &branch],
        );
        write_file(
            &wt_dir.join(format!("{id}.start.json")),
            &serde_json::json!({ "id": id, "pid": 999999 }).to_string(),
        );
        write_file(
            &wt_dir.join(format!("{id}.json")),
            &serde_json::json!({ "outcome": "verified" }).to_string(),
        );
        write_file(
            &wt_dir.join(format!("{id}.verdict.json")),
            &serde_json::json!({ "verdict": "accepted" }).to_string(),
        );
    }

    for id in unmerged_ids {
        let wt_path = wt_dir.join(id);
        let branch = format!("wip/{id}");
        git(
            root,
            &["worktree", "add", "-q", wt_path.to_str().unwrap(), &branch],
        );
        write_file(
            &wt_dir.join(format!("{id}.start.json")),
            &serde_json::json!({ "id": id, "pid": 999999 }).to_string(),
        );
        write_file(
            &wt_dir.join(format!("{id}.json")),
            &serde_json::json!({ "outcome": "verified" }).to_string(),
        );
    }

    dir
}

#[test]
fn test_clean_quiet_skip_keeps_skipped_and_prints_nothing_to_stdout() {
    let repo = setup_repo_with_runs(&["A", "B", "C"], &[]);
    let root = repo.path();

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--quiet", "--skip", "B"])
        .current_dir(root)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "expected empty stdout, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!root.join(".wt/A").exists(), "A should be removed");
    assert!(root.join(".wt/B").exists(), "B should be kept");
    assert!(!root.join(".wt/C").exists(), "C should be removed");
}

#[test]
fn test_clean_quiet_live_lock_exits_zero_and_removes_nothing() {
    let repo = setup_repo_with_runs(&["A"], &[]);
    let root = repo.path();

    let lock_path = root.join(".wt/clean.lock");
    write_file(&lock_path, &format!("{}\n", std::process::id()));

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--quiet"])
        .current_dir(root)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(
        root.join(".wt/A").exists(),
        "A should not be removed while live lock is held"
    );
    assert!(
        lock_path.exists(),
        "live process lock should remain in place"
    );
}

#[test]
fn test_clean_quiet_dead_lock_replaces_and_removes_lock_afterwards() {
    let repo = setup_repo_with_runs(&["A"], &[]);
    let root = repo.path();

    let mut child = Command::new("true").spawn().unwrap();
    let dead_pid = child.id();
    let _ = child.wait();

    let lock_path = root.join(".wt/clean.lock");
    write_file(&lock_path, &format!("{dead_pid}\n"));

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--quiet"])
        .current_dir(root)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty());
    assert!(
        !root.join(".wt/A").exists(),
        "A should be removed after replacing stale lock"
    );
    assert!(
        !lock_path.exists(),
        "clean.lock should be removed when clean ends"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("clean.lock"),
        "expected stderr to mention replacing stale lock, got: {stderr}"
    );
}

#[test]
fn test_clean_quiet_rejects_all_and_dry_run_and_hides_flags_in_help() {
    for bad_args in [
        &["blyte", "clean", "--quiet", "--all"][..],
        &["blyte", "clean", "--quiet", "--dry-run"][..],
        &["blyte", "clean", "--skip", "A", "--all"][..],
        &["blyte", "clean", "--skip", "A", "--dry-run"][..],
    ] {
        assert!(
            blyte_cli::cli::Cli::try_parse_from(bad_args).is_err(),
            "expected {:?} to be rejected",
            bad_args
        );
    }

    let mut cmd = blyte_cli::cli::Cli::command();
    let help_text = cmd
        .find_subcommand_mut("clean")
        .unwrap()
        .render_help()
        .to_string();
    assert!(
        !help_text.contains("--quiet"),
        "--quiet should be hidden from --help: {help_text}"
    );
    assert!(
        !help_text.contains("--skip"),
        "--skip should be hidden from --help: {help_text}"
    );
}

#[test]
fn test_review_accepted_removes_another_merged_run_in_background() {
    let repo = setup_repo_with_runs(&["M1"], &["R1"]);
    let root = repo.path();
    let wt_r1 = root.join(".wt/R1");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["review", "R1", "--verdict", "accepted"])
        .current_dir(root)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "review failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let wt_m1 = root.join(".wt/M1");
    let deadline = Instant::now() + Duration::from_secs(60);
    while wt_m1.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        !wt_m1.exists(),
        ".wt/M1 should be removed within 60 s by the background clean"
    );
    assert!(
        wt_r1.exists(),
        ".wt/R1 should still exist since it is not merged"
    );
}
