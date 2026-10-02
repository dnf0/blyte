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

fn init_repo_with_parent() -> (tempfile::TempDir, std::path::PathBuf) {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("my-repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    write_file(&root.join("README.md"), "initial\n");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "initial", "--no-gpg-sign"]);
    (outer, root)
}

#[test]
fn test_clean_reports_merged_clean_sibling_worktree_with_ds_store() {
    let (outer, root) = init_repo_with_parent();

    git(
        &root,
        &["worktree", "add", "../my-repo--side", "-b", "side"],
    );
    let side_path = outer.path().join("my-repo--side");

    write_file(&side_path.join("file.txt"), "feature work\n");
    git(&side_path, &["add", "."]);
    git(
        &side_path,
        &["commit", "-qm", "feat: feature work", "--no-gpg-sign"],
    );
    let commit_sha = git(&side_path, &["rev-parse", "HEAD"]);

    // Cherry pick onto main
    git(&root, &["cherry-pick", &commit_sha]);

    // Add untracked .DS_Store
    write_file(&side_path.join(".DS_Store"), "finder data\n");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains("merged worktree ../my-repo--side (side,"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("): git worktree remove ../my-repo--side"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("1 other worktrees are merged and clean ("),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("; remove them with the commands above"),
        "stdout was: {stdout}"
    );

    // Sibling worktree must still be present afterwards
    assert!(
        side_path.exists(),
        "sibling worktree must not be removed by clean"
    );

    // Test --dry-run produces the same output
    let out_dry = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--dry-run"])
        .current_dir(&root)
        .output()
        .unwrap();

    let stdout_dry = String::from_utf8_lossy(&out_dry.stdout);
    assert!(out_dry.status.success());
    assert!(stdout_dry.contains("merged worktree ../my-repo--side (side,"));
    assert!(stdout_dry.contains("1 other worktrees are merged and clean ("));
    assert!(side_path.exists());
}

#[test]
fn test_clean_sibling_with_unmerged_commit_not_reported() {
    let (outer, root) = init_repo_with_parent();

    git(
        &root,
        &["worktree", "add", "../my-repo--side", "-b", "side"],
    );
    let side_path = outer.path().join("my-repo--side");

    write_file(&side_path.join("file.txt"), "unmerged work\n");
    git(&side_path, &["add", "."]);
    git(
        &side_path,
        &["commit", "-qm", "feat: unmerged", "--no-gpg-sign"],
    );

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        !stdout.contains("my-repo--side"),
        "unmerged sibling should not be reported: {stdout}"
    );
    assert!(
        !stdout.contains("other worktrees are merged and clean"),
        "total line should not appear: {stdout}"
    );

    assert!(side_path.exists());
}

#[test]
fn test_clean_sibling_with_modified_tracked_file_not_reported() {
    let (outer, root) = init_repo_with_parent();

    git(
        &root,
        &["worktree", "add", "../my-repo--side", "-b", "side"],
    );
    let side_path = outer.path().join("my-repo--side");

    write_file(&side_path.join("file.txt"), "work\n");
    git(&side_path, &["add", "."]);
    git(
        &side_path,
        &["commit", "-qm", "feat: work", "--no-gpg-sign"],
    );
    let commit_sha = git(&side_path, &["rev-parse", "HEAD"]);

    git(&root, &["cherry-pick", &commit_sha]);

    // Modify a tracked file without committing
    write_file(&side_path.join("file.txt"), "modified content\n");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        !stdout.contains("my-repo--side"),
        "dirty sibling should not be reported: {stdout}"
    );
    assert!(
        !stdout.contains("other worktrees are merged and clean"),
        "total line should not appear: {stdout}"
    );

    assert!(side_path.exists());
}

#[test]
fn test_dispatch_quiet_clean_prints_nothing_about_sibling_worktrees() {
    let (outer, root) = init_repo_with_parent();

    // Create merged and clean sibling worktree
    git(
        &root,
        &["worktree", "add", "../my-repo--side", "-b", "side"],
    );
    let side_path = outer.path().join("my-repo--side");
    write_file(&side_path.join("file.txt"), "side work\n");
    git(&side_path, &["add", "."]);
    git(
        &side_path,
        &["commit", "-qm", "feat: side work", "--no-gpg-sign"],
    );
    let commit_sha = git(&side_path, &["rev-parse", "HEAD"]);
    git(&root, &["cherry-pick", &commit_sha]);
    write_file(&side_path.join(".DS_Store"), "finder data\n");

    // Create a finished worktree with a verdict that should be cleaned up by dispatch
    let wt_old = root.join(".wt/OLD");
    git(
        &root,
        &["worktree", "add", "-b", "wip/OLD", wt_old.to_str().unwrap()],
    );

    let wt_dir = root.join(".wt");
    write_file(
        &wt_dir.join("OLD.start.json"),
        &serde_json::json!({ "id": "OLD", "pid": 999999 }).to_string(),
    );
    write_file(
        &wt_dir.join("OLD.verdict.json"),
        &serde_json::json!({ "verdict": "redone" }).to_string(),
    );

    write_file(&root.join("task.md"), "implement something\n");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "dispatch",
            "D1",
            "task.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ])
        .current_dir(&root)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stderr.contains("blyte: removed 1 finished worktrees"),
        "stderr was: {stderr}"
    );
    assert!(!wt_old.exists(), "old worktree should have been removed");

    // Sibling worktree must not be mentioned in stdout or stderr
    assert!(
        !stdout.contains("my-repo--side"),
        "stdout should not mention sibling: {stdout}"
    );
    assert!(
        !stderr.contains("my-repo--side"),
        "stderr should not mention sibling: {stderr}"
    );
    assert!(
        !stdout.contains("other worktrees"),
        "stdout should not mention other worktrees: {stdout}"
    );
    assert!(
        !stderr.contains("other worktrees"),
        "stderr should not mention other worktrees: {stderr}"
    );

    assert!(side_path.exists());
}

#[test]
fn test_clean_sibling_detached_head_merged_reported() {
    let (outer, root) = init_repo_with_parent();

    write_file(&root.join("main_commit.txt"), "main commit\n");
    git(&root, &["add", "."]);
    git(
        &root,
        &["commit", "-qm", "feat: main commit", "--no-gpg-sign"],
    );
    let head_sha = git(&root, &["rev-parse", "HEAD"]);
    let short_sha = &head_sha[..7];

    // Create detached worktree at HEAD
    git(
        &root,
        &[
            "worktree",
            "add",
            "--detach",
            "../my-repo--detached",
            "HEAD",
        ],
    );
    let detached_path = outer.path().join("my-repo--detached");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        stdout.contains(&format!(
            "merged worktree ../my-repo--detached ({short_sha},"
        )),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("1 other worktrees are merged and clean ("),
        "stdout was: {stdout}"
    );
    assert!(detached_path.exists());
}
