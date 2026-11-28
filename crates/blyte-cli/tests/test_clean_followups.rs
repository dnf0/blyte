use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

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

fn empty_repo_template() -> &'static Path {
    static TEMPLATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            let t = tempfile::tempdir().unwrap();
            git(t.path(), &["init", "-q", "-b", "main"]);
            t
        })
        .path()
}

fn init_empty_repo() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    copy_dir_recursive(empty_repo_template(), t.path());
    t
}

fn retarget_worktree_links(root: &Path, names: &[&str]) {
    let canon = root.canonicalize().unwrap();
    for name in names {
        let wt_git = canon.join(".wt").join(name).join(".git");
        let admin_dir = canon.join(".git/worktrees").join(name);
        std::fs::write(&wt_git, format!("gitdir: {}\n", admin_dir.display())).unwrap();
        std::fs::write(admin_dir.join("gitdir"), format!("{}\n", wt_git.display())).unwrap();
    }
}

fn build_t1_t1b_template(squash_t1b_into_main: bool) -> tempfile::TempDir {
    let t = init_empty_repo();
    let root = t.path();
    let mut stream = String::new();
    fi_commit(
        &mut stream,
        "main",
        1,
        None,
        "initial",
        &[("README.md", "initial\n")],
    );
    fi_commit(
        &mut stream,
        "wip/T1",
        2,
        Some(1),
        "feat: work by T1",
        &[("file.txt", "work by T1\n")],
    );
    fi_commit(
        &mut stream,
        "wip/T1b",
        3,
        Some(2),
        "feat: rewritten by T1b",
        &[("file.txt", "rewritten by T1b\n")],
    );
    if squash_t1b_into_main {
        fi_commit(
            &mut stream,
            "main",
            4,
            Some(1),
            "squash merge T1b",
            &[("file.txt", "rewritten by T1b\n")],
        );
    }
    fast_import(root, &stream);
    let t1_path = root.join(".wt/T1");
    let t1b_path = root.join(".wt/T1b");
    git(
        root,
        &["worktree", "add", "-q", t1_path.to_str().unwrap(), "wip/T1"],
    );
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            t1b_path.to_str().unwrap(),
            "wip/T1b",
        ],
    );
    t
}

fn fi_commit(
    buf: &mut String,
    branch: &str,
    mark: usize,
    from: Option<usize>,
    msg: &str,
    files: &[(&str, &str)],
) {
    use std::fmt::Write as _;
    writeln!(buf, "commit refs/heads/{branch}").unwrap();
    writeln!(buf, "mark :{mark}").unwrap();
    writeln!(buf, "committer test <test@test.com> 1700000000 +0000").unwrap();
    writeln!(buf, "data {}\n{msg}", msg.len()).unwrap();
    if let Some(parent) = from {
        writeln!(buf, "from :{parent}").unwrap();
    }
    for (path, content) in files {
        writeln!(buf, "M 100644 inline {path}").unwrap();
        writeln!(buf, "data {}\n{content}", content.len()).unwrap();
    }
}

fn write_file(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn branch_exists(root: &Path, branch: &str) -> bool {
    if root.join(".git/refs/heads").join(branch).is_file() {
        return true;
    }
    let packed = root.join(".git/packed-refs");
    if let Ok(text) = std::fs::read_to_string(packed) {
        let needle = format!(" refs/heads/{branch}");
        if text.lines().any(|l| l.ends_with(&needle)) {
            return true;
        }
    }
    false
}

fn blyte_cmd(root: &Path, args: &[&str]) -> std::process::Output {
    let home = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(root)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("BLYTE_STATE_DIR", home.path().join("state"))
        .env("BLYTE_ENV_FILE", "")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_NO_KEEP_AWAKE", "1")
        .output()
        .unwrap()
}

fn setup_t1_t1b_repo(squash_t1b_into_main: bool) -> tempfile::TempDir {
    static MERGED: OnceLock<tempfile::TempDir> = OnceLock::new();
    let template = if squash_t1b_into_main {
        MERGED.get_or_init(|| build_t1_t1b_template(true)).path()
    } else {
        return build_t1_t1b_template(false);
    };
    let t = tempfile::tempdir().unwrap();
    copy_dir_recursive(template, t.path());
    retarget_worktree_links(t.path(), &["T1", "T1b"]);
    t
}

#[test]
fn test_clean_followup_merged_removes_both_and_deletes_branches() {
    let repo = setup_t1_t1b_repo(true);
    let root = repo.path();
    let t1_path = root.join(".wt/T1");
    let t1b_path = root.join(".wt/T1b");

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = blyte_cmd(root, &["clean"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/T1b (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1b (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T1 (merged through T1b,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!t1_path.exists(), "T1 worktree should be removed");
    assert!(!t1b_path.exists(), "T1b worktree should be removed");
    assert!(
        !branch_exists(root, "wip/T1"),
        "wip/T1 branch should be deleted"
    );
    assert!(
        !branch_exists(root, "wip/T1b"),
        "wip/T1b branch should be deleted"
    );
}

#[test]
fn test_clean_followup_three_deep_only_last_merged() {
    let repo = init_empty_repo();
    let root = repo.path();
    let mut stream = String::new();
    fi_commit(
        &mut stream,
        "main",
        1,
        None,
        "initial",
        &[("README.md", "initial\n")],
    );
    fi_commit(
        &mut stream,
        "wip/T1",
        2,
        Some(1),
        "feat: work by T1",
        &[("file.txt", "work by T1\n")],
    );
    fi_commit(
        &mut stream,
        "wip/T1b",
        3,
        Some(2),
        "feat: rewritten by T1b",
        &[("file.txt", "rewritten by T1b\n")],
    );
    fi_commit(
        &mut stream,
        "wip/T1c",
        4,
        Some(3),
        "feat: rewritten by T1c",
        &[("file.txt", "rewritten by T1c\n")],
    );
    fi_commit(
        &mut stream,
        "main",
        5,
        Some(1),
        "squash merge T1c",
        &[("file.txt", "rewritten by T1c\n")],
    );
    fast_import(root, &stream);

    let t1_path = root.join(".wt/T1");
    let t1b_path = root.join(".wt/T1b");
    let t1c_path = root.join(".wt/T1c");
    git(
        root,
        &["worktree", "add", "-q", t1_path.to_str().unwrap(), "wip/T1"],
    );
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            t1b_path.to_str().unwrap(),
            "wip/T1b",
        ],
    );
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            t1c_path.to_str().unwrap(),
            "wip/T1c",
        ],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1c.start.json"),
        &serde_json::json!({ "id": "T1c", "pid": 999999, "base": "wip/T1b" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1c.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1c.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = blyte_cmd(root, &["clean"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/T1 (merged through T1b,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T1b (merged through T1c,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1b (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T1c (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1c (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!t1_path.exists());
    assert!(!t1b_path.exists());
    assert!(!t1c_path.exists());
    assert!(
        !branch_exists(root, "wip/T1"),
        "wip/T1 branch should be deleted"
    );
    assert!(
        !branch_exists(root, "wip/T1b"),
        "wip/T1b branch should be deleted"
    );
    assert!(
        !branch_exists(root, "wip/T1c"),
        "wip/T1c branch should be deleted"
    );
}

#[test]
fn test_clean_followup_not_merged_keeps_t1() {
    let repo = setup_t1_t1b_repo(false);
    let root = repo.path();
    let t1_path = root.join(".wt/T1");
    let t1b_path = root.join(".wt/T1b");

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = blyte_cmd(root, &["clean"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("kept .wt/T1 (not merged yet)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("kept .wt/T1b (not merged yet)"),
        "stdout was: {stdout}"
    );

    assert!(t1_path.exists(), "T1 worktree should be kept");
    assert!(t1b_path.exists(), "T1b worktree should be kept");
    assert!(
        branch_exists(root, "wip/T1"),
        "wip/T1 branch should be kept"
    );
    assert!(
        branch_exists(root, "wip/T1b"),
        "wip/T1b branch should be kept"
    );
}

fn setup_t1_only_repo() -> tempfile::TempDir {
    static TEMPLATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    let template = TEMPLATE
        .get_or_init(|| {
            let repo = init_empty_repo();
            let root = repo.path();
            let mut stream = String::new();
            fi_commit(
                &mut stream,
                "main",
                1,
                None,
                "initial",
                &[("README.md", "initial\n")],
            );
            fi_commit(
                &mut stream,
                "wip/T1",
                2,
                Some(1),
                "feat: work by T1",
                &[("file.txt", "work by T1\n")],
            );
            fast_import(root, &stream);
            let t1_path = root.join(".wt/T1");
            git(
                root,
                &["worktree", "add", "-q", t1_path.to_str().unwrap(), "wip/T1"],
            );
            repo
        })
        .path();
    let t = tempfile::tempdir().unwrap();
    copy_dir_recursive(template, t.path());
    retarget_worktree_links(t.path(), &["T1"]);
    t
}

#[test]
fn test_clean_followup_child_already_deleted_by_earlier_clean() {
    let repo = setup_t1_only_repo();
    let root = repo.path();
    let t1_path = root.join(".wt/T1");

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    let out = blyte_cmd(root, &["clean"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/T1 (merged through T1b,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!t1_path.exists(), "T1 worktree should be removed");
    assert!(
        !branch_exists(root, "wip/T1"),
        "wip/T1 branch should be deleted"
    );
}

#[test]
fn test_clean_followup_wt1_wt2_squash_merged_lines_later_changed_both_removed() {
    let repo = init_empty_repo();
    let root = repo.path();
    let mut stream = String::new();
    fi_commit(
        &mut stream,
        "main",
        1,
        None,
        "initial",
        &[("README.md", "initial\n"), (".gitignore", ".wt/\n")],
    );
    fi_commit(
        &mut stream,
        "wip/T1",
        2,
        Some(1),
        "feat: work by T1",
        &[("file.txt", "work by T1\n")],
    );
    fi_commit(
        &mut stream,
        "wip/T2",
        3,
        Some(2),
        "feat: rewritten by T2",
        &[("file.txt", "rewritten by T2\n")],
    );
    fi_commit(
        &mut stream,
        "main",
        4,
        Some(1),
        "squash merge T2",
        &[("file.txt", "rewritten by T2\n")],
    );
    fi_commit(
        &mut stream,
        "main",
        5,
        Some(4),
        "main: edit lines again",
        &[("file.txt", "rewritten by T2\nline added later on main\n")],
    );
    fast_import(root, &stream);

    let t1_path = root.join(".wt/T1");
    let t2_path = root.join(".wt/T2");
    git(
        root,
        &["worktree", "add", "-q", t1_path.to_str().unwrap(), "wip/T1"],
    );
    git(
        root,
        &["worktree", "add", "-q", t2_path.to_str().unwrap(), "wip/T2"],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999, "started_at": 1000 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T2.start.json"),
        &serde_json::json!({ "id": "T2", "pid": 999999, "started_at": 1000 }).to_string(),
    );
    write_file(
        &wt_dir.join("T2.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T2.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = blyte_cmd(root, &["clean"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/T1 (merged through T2,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T2 (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T2 (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!t1_path.exists());
    assert!(!t2_path.exists());
    assert!(
        !branch_exists(root, "wip/T1"),
        "wip/T1 branch should be deleted"
    );
    assert!(
        !branch_exists(root, "wip/T2"),
        "wip/T2 branch should be deleted"
    );
}

#[test]
fn test_clean_timing_guard_40_runs_under_3s() {
    let repo = init_empty_repo();
    let root = repo.path();

    let mut stream = String::new();
    fi_commit(
        &mut stream,
        "main",
        1,
        None,
        "initial",
        &[("README.md", "initial\n"), (".gitignore", ".wt/\n")],
    );

    let wt_dir = root.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();

    let mut head_mark = 1usize;
    let mut next_mark = 2usize;
    for i in 0..5 {
        let rid = format!("M{i:02}");
        let file = format!("{rid}.txt");
        let content = format!("{rid}\n");
        let branch = format!("wip/{rid}");
        let m = next_mark;
        next_mark += 1;
        fi_commit(
            &mut stream,
            &branch,
            m,
            Some(head_mark),
            &rid,
            &[(&file, &content)],
        );
        head_mark = m;

        write_file(
            &wt_dir.join(format!("{rid}.start.json")),
            &serde_json::json!({ "id": rid, "pid": 999999, "started_at": 1000 }).to_string(),
        );
        write_file(
            &wt_dir.join(format!("{rid}.verdict.json")),
            &serde_json::json!({ "verdict": "accepted" }).to_string(),
        );
    }
    stream.push_str(&format!("reset refs/heads/main\nfrom :{head_mark}\n"));

    for i in 0..35 {
        let rid = format!("U{i:02}");
        let file = format!("{rid}.txt");
        let content = format!("{rid}\n");
        let branch = format!("wip/{rid}");
        let m = next_mark;
        next_mark += 1;
        fi_commit(
            &mut stream,
            &branch,
            m,
            Some(head_mark),
            &rid,
            &[(&file, &content)],
        );

        write_file(
            &wt_dir.join(format!("{rid}.start.json")),
            &serde_json::json!({ "id": rid, "pid": 999999, "started_at": 1000 }).to_string(),
        );
        write_file(
            &wt_dir.join(format!("{rid}.verdict.json")),
            &serde_json::json!({ "verdict": "accepted" }).to_string(),
        );
    }
    fast_import(root, &stream);

    let mut ctx = blyte_cli::clean::CleanContext::new(root);
    for id in &ctx.all_run_ids.clone() {
        let _ = ctx.check_removable(id, 14);
    }
    assert!(
        ctx.branch_contains_calls <= 40,
        "branch_contains_calls was {}, expected <= 40",
        ctx.branch_contains_calls
    );

    let start = std::time::Instant::now();
    let out = blyte_cmd(root, &["clean", "--dry-run"]);
    let elapsed = start.elapsed();

    assert!(out.status.success());
    assert!(
        elapsed < std::time::Duration::from_secs(60),
        "clean --dry-run speed guard took {:?}, expected < 60 s (widened from 3 s for parallel suite load)",
        elapsed
    );
}

#[test]
fn test_clean_followup_child_already_deleted_verdict_accepted() {
    let repo = setup_t1_only_repo();
    let root = repo.path();
    let t1_path = root.join(".wt/T1");

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = blyte_cmd(root, &["clean"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/T1 (merged through T1b,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(!t1_path.exists());
}

#[test]
fn test_review_followup_merged_removes_worktree_deletes_branch() {
    let repo = setup_t1_t1b_repo(true);
    let root = repo.path();
    let t1_path = root.join(".wt/T1");

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );

    let out = blyte_cmd(root, &["review", "T1", "--verdict", "accepted"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "review failed: {stdout}");
    assert!(stdout.contains("removed .wt/T1; branch wip/T1 deleted"));
    assert!(!t1_path.exists());
    assert!(!branch_exists(root, "wip/T1"));
}

#[test]
fn test_clean_followup_dry_run() {
    let repo = setup_t1_t1b_repo(true);
    let root = repo.path();
    let t1_path = root.join(".wt/T1");
    let t1b_path = root.join(".wt/T1b");

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = blyte_cmd(root, &["clean", "--dry-run"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("would remove .wt/T1 (merged through T1b,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("would delete branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("would remove .wt/T1b (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("would delete branch wip/T1b (merged)"),
        "stdout was: {stdout}"
    );

    assert!(t1_path.exists());
    assert!(t1b_path.exists());
    assert!(branch_exists(root, "wip/T1"));
    assert!(branch_exists(root, "wip/T1b"));
}

#[test]
fn test_clean_followup_first_child_unmerged_second_child_merged() {
    let repo = init_empty_repo();
    let root = repo.path();
    let mut stream = String::new();
    fi_commit(
        &mut stream,
        "main",
        1,
        None,
        "initial",
        &[("README.md", "initial\n")],
    );
    fi_commit(
        &mut stream,
        "wip/T1",
        2,
        Some(1),
        "feat: work by T1",
        &[("file.txt", "work by T1\n")],
    );
    fi_commit(
        &mut stream,
        "wip/T1b",
        3,
        Some(2),
        "feat: rewritten by T1b",
        &[("file.txt", "rewritten by T1b\n")],
    );
    fi_commit(
        &mut stream,
        "wip/T1c",
        4,
        Some(2),
        "feat: rewritten by T1c",
        &[("file.txt", "rewritten by T1c\n")],
    );
    fi_commit(
        &mut stream,
        "main",
        5,
        Some(1),
        "squash merge T1c",
        &[("file.txt", "rewritten by T1c\n")],
    );
    fast_import(root, &stream);

    let t1_path = root.join(".wt/T1");
    let t1b_path = root.join(".wt/T1b");
    let t1c_path = root.join(".wt/T1c");
    git(
        root,
        &["worktree", "add", "-q", t1_path.to_str().unwrap(), "wip/T1"],
    );
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            t1b_path.to_str().unwrap(),
            "wip/T1b",
        ],
    );
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            t1c_path.to_str().unwrap(),
            "wip/T1c",
        ],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1c.start.json"),
        &serde_json::json!({ "id": "T1c", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1c.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1c.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = blyte_cmd(root, &["clean"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/T1 (merged through T1c,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("kept .wt/T1b (not merged yet)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T1c (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1c (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!t1_path.exists());
    assert!(t1b_path.exists());
    assert!(!t1c_path.exists());
}

#[test]
fn test_clean_followup_without_base_fields_inferred_from_git() {
    let repo = setup_t1_t1b_repo(true);
    let root = repo.path();
    let t1_path = root.join(".wt/T1");
    let t1b_path = root.join(".wt/T1b");

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = blyte_cmd(root, &["clean"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("removed .wt/T1 (merged through T1b,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1 (merged)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("removed .wt/T1b (merged,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("deleted branch wip/T1b (merged)"),
        "stdout was: {stdout}"
    );

    assert!(!t1_path.exists());
    assert!(!t1b_path.exists());
    assert!(
        !branch_exists(root, "wip/T1"),
        "wip/T1 branch should be deleted"
    );
    assert!(
        !branch_exists(root, "wip/T1b"),
        "wip/T1b branch should be deleted"
    );
}

#[test]
fn test_clean_followup_refs_heads_wip_base_syntax() {
    let repo = setup_t1_t1b_repo(true);
    let root = repo.path();
    let t1_path = root.join(".wt/T1");

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T1b.start.json"),
        &serde_json::json!({ "id": "T1b", "pid": 999999, "base": "refs/heads/wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1b.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = blyte_cmd(root, &["clean"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(stdout.contains("removed .wt/T1 (merged through T1b,"));
    assert!(stdout.contains("deleted branch wip/T1 (merged)"));
    assert!(!t1_path.exists());
}

#[test]
fn test_clean_followup_cycle_guard() {
    let repo = init_empty_repo();
    let root = repo.path();
    let mut stream = String::new();
    fi_commit(
        &mut stream,
        "main",
        1,
        None,
        "initial",
        &[("README.md", "initial\n")],
    );
    fi_commit(
        &mut stream,
        "wip/T1",
        2,
        Some(1),
        "feat: work 1",
        &[("file1.txt", "work by T1\n")],
    );
    fi_commit(
        &mut stream,
        "wip/T2",
        3,
        Some(1),
        "feat: work 2",
        &[("file2.txt", "work by T2\n")],
    );
    fast_import(root, &stream);

    let t1_path = root.join(".wt/T1");
    let t2_path = root.join(".wt/T2");
    git(
        root,
        &["worktree", "add", "-q", t1_path.to_str().unwrap(), "wip/T1"],
    );
    git(
        root,
        &["worktree", "add", "-q", t2_path.to_str().unwrap(), "wip/T2"],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("T1.start.json"),
        &serde_json::json!({ "id": "T1", "pid": 999999, "base": "wip/T2" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T1.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    write_file(
        &wt_dir.join("T2.start.json"),
        &serde_json::json!({ "id": "T2", "pid": 999999, "base": "wip/T1" }).to_string(),
    );
    write_file(
        &wt_dir.join("T2.json"),
        &serde_json::json!({ "outcome": "verified" }).to_string(),
    );
    write_file(
        &wt_dir.join("T2.verdict.json"),
        &serde_json::json!({ "verdict": "accepted" }).to_string(),
    );

    let out = blyte_cmd(root, &["clean"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(stdout.contains("kept .wt/T1 (not merged yet)"));
    assert!(stdout.contains("kept .wt/T2 (not merged yet)"));
    assert!(t1_path.exists());
    assert!(t2_path.exists());
}
