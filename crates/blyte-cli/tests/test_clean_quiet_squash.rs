use std::path::Path;
use std::process::Command;

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

fn write_file(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn init_repo() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    git(t.path(), &["init", "-q", "-b", "main"]);
    write_file(&t.path().join("README.md"), "initial\n");
    git(t.path(), &["add", "."]);
    git(t.path(), &["commit", "-qm", "initial", "--no-gpg-sign"]);
    t
}

fn setup_finished_run_a1(root: &Path) {
    let wt_path = root.join(".wt/A1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/A1",
            wt_path.to_str().unwrap(),
            "main",
        ],
    );

    write_file(&wt_path.join("a.txt"), "line 1\n");
    git(&wt_path, &["add", "."]);
    git(&wt_path, &["commit", "-qm", "feat: add a", "--no-gpg-sign"]);

    write_file(&wt_path.join("a.txt"), "line 1 tweaked\n");
    git(&wt_path, &["add", "."]);
    git(
        &wt_path,
        &["commit", "-qm", "fix: tweak a", "--no-gpg-sign"],
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
}

#[test]
fn test_squash_merged_quiet_clean() {
    let repo = init_repo();
    let root = repo.path();

    setup_finished_run_a1(root);

    write_file(&root.join("a.txt"), "line 1 tweaked\n");
    git(root, &["add", "."]);
    git(
        root,
        &[
            "commit",
            "-qm",
            "feat: x (#1)\n\n* feat: add a\n\n* fix: tweak a",
            "--no-gpg-sign",
        ],
    );

    // Later commit on main that edits the same lines so merge-tree does not match
    write_file(&root.join("a.txt"), "line 1 edited later on main\n");
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "-qm", "chore: edit a later", "--no-gpg-sign"],
    );

    blyte_cli::clean::clean_quiet(root, None);

    assert!(
        !root.join(".wt/A1").exists(),
        ".wt/A1 should be removed by clean_quiet"
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
    let repo = init_repo();
    let root = repo.path();

    setup_finished_run_a1(root);

    write_file(&root.join("a.txt"), "line 1 tweaked\n");
    git(root, &["add", "."]);
    git(
        root,
        &[
            "commit",
            "-qm",
            "feat: x (#1)\n\n* feat: add a",
            "--no-gpg-sign",
        ],
    );

    write_file(&root.join("a.txt"), "line 1 edited later on main\n");
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "-qm", "chore: edit a later", "--no-gpg-sign"],
    );

    blyte_cli::clean::clean_quiet(root, None);

    assert!(
        root.join(".wt/A1").exists(),
        ".wt/A1 should be kept when only partly listed"
    );
}

#[test]
fn test_review_sweep_removes_squash_merged_other_run() {
    let repo = init_repo();
    let root = repo.path();

    setup_finished_run_a1(root);

    write_file(&root.join("a.txt"), "line 1 tweaked\n");
    git(root, &["add", "."]);
    git(
        root,
        &[
            "commit",
            "-qm",
            "feat: x (#1)\n\n* feat: add a\n\n* fix: tweak a",
            "--no-gpg-sign",
        ],
    );

    write_file(&root.join("a.txt"), "line 1 edited later on main\n");
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "-qm", "chore: edit a later", "--no-gpg-sign"],
    );

    // Run B1: finished, not merged
    let wt_b1 = root.join(".wt/B1");
    git(
        root,
        &[
            "worktree",
            "add",
            "-b",
            "wip/B1",
            wt_b1.to_str().unwrap(),
            "main",
        ],
    );
    write_file(&wt_b1.join("b.txt"), "unmerged b\n");
    git(&wt_b1, &["add", "."]);
    git(&wt_b1, &["commit", "-qm", "feat: add b", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
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

    blyte_cli::clean::clean_on_review(root, "B1");

    assert!(
        wt_b1.exists(),
        ".wt/B1 should be kept because B1 is not merged"
    );
    assert!(
        !root.join(".wt/A1").exists(),
        ".wt/A1 should be removed by the review sweep"
    );
}
