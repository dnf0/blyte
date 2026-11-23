use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=test", "-c", "user.email=test@test.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

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

fn fi_commit(
    branch: &str,
    mark: usize,
    from_mark: Option<usize>,
    msg: &str,
    files: &[(&str, &str)],
) -> String {
    let mut s = format!(
        "commit refs/heads/{branch}\nmark :{mark}\ncommitter test <test@test.com> 1700000000 +0000\ndata {}\n{msg}\n",
        msg.len()
    );
    if let Some(parent) = from_mark {
        s.push_str(&format!("from :{parent}\n"));
    }
    for (path, body) in files {
        s.push_str(&format!(
            "M 100644 inline {path}\ndata {}\n{body}\n",
            body.len()
        ));
    }
    s
}

fn write_file(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn setup_repo(squash_msg: &str, squash_a_content: &str, include_b1: bool) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    git(root, &["init", "-q", "-b", "main"]);

    let mut script = String::new();
    script.push_str(&fi_commit(
        "main",
        1,
        None,
        "initial",
        &[("README.md", "initial\n")],
    ));
    script.push_str(&fi_commit(
        "wip/A1",
        2,
        Some(1),
        "feat: add a",
        &[("a.txt", "line 1\n")],
    ));
    script.push_str(&fi_commit(
        "wip/A1",
        3,
        Some(2),
        "fix: tweak a",
        &[("a.txt", "line 1 tweaked\n")],
    ));
    script.push_str(&fi_commit(
        "main",
        4,
        Some(1),
        squash_msg,
        &[("a.txt", squash_a_content)],
    ));
    script.push_str(&fi_commit(
        "main",
        5,
        Some(4),
        "chore: edit a later",
        &[("a.txt", "line 1 edited later on main\n")],
    ));
    if include_b1 {
        script.push_str(&fi_commit(
            "wip/B1",
            6,
            Some(5),
            "feat: add b",
            &[("b.txt", "unmerged b\n")],
        ));
    }
    fast_import(root, &script);

    let wt_a1 = root.join(".wt/A1");
    git(
        root,
        &["worktree", "add", "-q", wt_a1.to_str().unwrap(), "wip/A1"],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("A1.start.json"),
        &serde_json::json!({ "id": "A1", "pid": 999999, "started_at": 1000 }).to_string(),
    );
    write_file(
        &wt_dir.join("A1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("A1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    if include_b1 {
        let wt_b1 = root.join(".wt/B1");
        git(
            root,
            &["worktree", "add", "-q", wt_b1.to_str().unwrap(), "wip/B1"],
        );
        write_file(
            &wt_dir.join("B1.start.json"),
            &serde_json::json!({ "id": "B1", "pid": 999999, "started_at": 1000 }).to_string(),
        );
        write_file(
            &wt_dir.join("B1.json"),
            &serde_json::json!({ "outcome": "verified" }).to_string(),
        );
        write_file(
            &wt_dir.join("B1.verdict.json"),
            &serde_json::json!({ "verdict": "accepted" }).to_string(),
        );
    }

    t
}

#[test]
fn test_squash_merged_quiet_clean() {
    let repo = setup_repo(
        "feat: x (#1)\n\n* feat: add a\n\n* fix: tweak a",
        "line 1 tweaked\n",
        false,
    );
    let root = repo.path();

    blyte_cli::clean::clean_repo(root, 14, false, None, true);

    assert!(
        !root.join(".wt/A1").exists(),
        ".wt/A1 should be removed by clean_repo"
    );
    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/wip/A1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        !branch_check.status.success(),
        "wip/A1 branch should be deleted"
    );
}

#[test]
fn test_partly_listed_quiet_clean() {
    let repo = setup_repo("feat: x (#1)\n\n* feat: add a", "line 1\n", false);
    let root = repo.path();

    blyte_cli::clean::clean_repo(root, 14, false, None, true);

    assert!(
        root.join(".wt/A1").exists(),
        ".wt/A1 should be kept when only partly listed"
    );
}

#[test]
fn test_review_sweep_removes_squash_merged_other_run() {
    let repo = setup_repo(
        "feat: x (#1)\n\n* feat: add a\n\n* fix: tweak a",
        "line 1 tweaked\n",
        true,
    );
    let root = repo.path();
    let wt_b1 = root.join(".wt/B1");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["review", "B1", "--verdict", "accepted"])
        .current_dir(root)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    assert!(out.status.success());

    let wt_a1 = root.join(".wt/A1");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while wt_a1.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(
        wt_b1.exists(),
        ".wt/B1 should be kept because B1 is not merged"
    );
    assert!(
        !wt_a1.exists(),
        ".wt/A1 should be removed by the review sweep"
    );
}
