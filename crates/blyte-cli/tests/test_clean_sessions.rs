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
        .env("BLYTE_NO_UPDATE_CHECK", "1")
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

#[test]
fn test_clean_linked_worktree_reports_nothing_about_other_worktrees() {
    let (outer, root) = init_repo_with_parent();

    // Create a linked worktree
    git(
        &root,
        &["worktree", "add", "../my-repo--linked", "-b", "linked"],
    );
    let linked_path = outer.path().join("my-repo--linked");

    // Create a sibling worktree that is merged and clean
    git(
        &root,
        &["worktree", "add", "../my-repo--side", "-b", "side"],
    );
    let side_path = outer.path().join("my-repo--side");
    write_file(&side_path.join("feat.txt"), "feat\n");
    git(&side_path, &["add", "."]);
    git(&side_path, &["commit", "-qm", "feat", "--no-gpg-sign"]);
    let sha = git(&side_path, &["rev-parse", "HEAD"]);
    git(&root, &["cherry-pick", &sha]);

    // Clean from linked worktree root
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&linked_path)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        !stdout.contains("merged worktree"),
        "linked worktree clean should report nothing about other worktrees: {stdout}"
    );
    assert!(
        !stdout.contains("other worktrees are merged and clean"),
        "linked worktree clean should report nothing about other worktrees: {stdout}"
    );

    // Also verify with --dry-run
    let out_dry = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--dry-run"])
        .current_dir(&linked_path)
        .output()
        .unwrap();

    let stdout_dry = String::from_utf8_lossy(&out_dry.stdout);
    assert!(
        out_dry.status.success(),
        "clean --dry-run failed: {stdout_dry}"
    );
    assert!(
        !stdout_dry.contains("merged worktree"),
        "linked worktree clean --dry-run should report nothing about other worktrees: {stdout_dry}"
    );
    assert!(
        !stdout_dry.contains("other worktrees are merged and clean"),
        "linked worktree clean --dry-run should report nothing about other worktrees: {stdout_dry}"
    );
}

#[test]
fn test_clean_never_lists_main_worktree_from_main_or_all() {
    let (outer, root) = init_repo_with_parent();

    // Create a linked worktree
    git(
        &root,
        &["worktree", "add", "../my-repo--linked", "-b", "linked"],
    );
    let linked_path = outer.path().join("my-repo--linked");

    // Create a sibling worktree that is merged and clean
    git(
        &root,
        &["worktree", "add", "../my-repo--side", "-b", "side"],
    );
    let side_path = outer.path().join("my-repo--side");
    write_file(&side_path.join("feat.txt"), "feat\n");
    git(&side_path, &["add", "."]);
    git(&side_path, &["commit", "-qm", "feat", "--no-gpg-sign"]);
    let sha = git(&side_path, &["rev-parse", "HEAD"]);
    git(&root, &["cherry-pick", &sha]);

    // 1. Cleaning from the main checkout: main worktree must not be listed
    let out_main = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&root)
        .output()
        .unwrap();
    let stdout_main = String::from_utf8_lossy(&out_main.stdout);
    assert!(out_main.status.success(), "clean failed: {stdout_main}");
    assert!(
        !stdout_main.contains("(main,"),
        "main worktree must not be listed: {stdout_main}"
    );
    assert!(
        !stdout_main.contains("merged worktree . ("),
        "main worktree must not be listed: {stdout_main}"
    );
    assert!(
        !stdout_main.contains("merged worktree my-repo ("),
        "main worktree must not be listed: {stdout_main}"
    );
    assert!(
        stdout_main.contains("merged worktree ../my-repo--side (side,"),
        "side worktree should be reported: {stdout_main}"
    );

    // 2. Cleaning with --all with both root and linked registered
    let state_dir = tempfile::tempdir().unwrap();
    let root_canon = root.canonicalize().unwrap();
    let linked_canon = linked_path.canonicalize().unwrap();
    let repos_content = format!("{}\n{}\n", root_canon.display(), linked_canon.display());
    std::fs::create_dir_all(state_dir.path()).unwrap();
    std::fs::write(state_dir.path().join("repos"), repos_content).unwrap();

    let out_all = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean", "--all"])
        .env("BLYTE_STATE_DIR", state_dir.path())
        .current_dir(&root)
        .output()
        .unwrap();
    let stdout_all = String::from_utf8_lossy(&out_all.stdout);
    assert!(out_all.status.success(), "clean --all failed: {stdout_all}");
    assert!(
        !stdout_all.contains("(main,"),
        "main worktree must not be listed in --all: {stdout_all}"
    );
    assert!(
        !stdout_all.contains("merged worktree ../../.."),
        "relative parent of linked worktree must not be listed: {stdout_all}"
    );
    // Sibling should be reported exactly once (no duplicate report from linked worktree)
    let side_matches = stdout_all
        .matches("merged worktree ../my-repo--side (side,")
        .count();
    assert_eq!(
        side_matches, 1,
        "side worktree must only be reported once in --all: {stdout_all}"
    );
}

#[test]
fn test_clean_outside_worktree_shows_absolute_path() {
    let (_outer, root) = init_repo_with_parent();

    // Create a separate tempdir (outside root's parent)
    let outside_dir = tempfile::tempdir().unwrap();
    let outside_path = outside_dir.path().join("outside-wt");

    git(
        &root,
        &[
            "worktree",
            "add",
            outside_path.to_str().unwrap(),
            "-b",
            "outside",
        ],
    );

    write_file(&outside_path.join("file.txt"), "outside work\n");
    git(&outside_path, &["add", "."]);
    git(
        &outside_path,
        &["commit", "-qm", "feat: outside", "--no-gpg-sign"],
    );
    let commit_sha = git(&outside_path, &["rev-parse", "HEAD"]);

    // Cherry pick onto main
    git(&root, &["cherry-pick", &commit_sha]);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");

    let outside_canon = outside_path.canonicalize().unwrap();
    let outside_canon_str = outside_canon.to_string_lossy().replace('\\', "/");

    assert!(
        stdout.contains(&format!("merged worktree {outside_canon_str} (outside,")),
        "stdout was: {stdout}, expected merged worktree with absolute path {outside_canon_str}"
    );
    assert!(
        stdout.contains(&format!(": git worktree remove {outside_canon_str}")),
        "stdout was: {stdout}, expected git worktree remove with absolute path {outside_canon_str}"
    );
    assert!(
        !stdout.contains("../../../"),
        "stdout should not have long relative path: {stdout}"
    );
}

#[test]
fn test_clean_worktree_under_wt_directory_not_listed() {
    let (outer, root) = init_repo_with_parent();

    // Create a linked worktree
    git(
        &root,
        &["worktree", "add", "../my-repo--linked", "-b", "linked"],
    );
    let linked_path = outer.path().join("my-repo--linked");

    // Inside linked worktree, create a worktree in a .wt subdirectory (like blyte's own runs of a linked-worktree root)
    let wt_sub = linked_path.join(".wt").join("SUB1");
    std::fs::create_dir_all(linked_path.join(".wt")).unwrap();
    git(
        &linked_path,
        &[
            "worktree",
            "add",
            "-b",
            "wip/SUB1",
            wt_sub.to_str().unwrap(),
        ],
    );

    write_file(&wt_sub.join("sub.txt"), "sub work\n");
    git(&wt_sub, &["add", "."]);
    git(
        &wt_sub,
        &["commit", "-qm", "feat: sub work", "--no-gpg-sign"],
    );
    let commit_sha = git(&wt_sub, &["rev-parse", "HEAD"]);
    git(&root, &["cherry-pick", &commit_sha]);

    // Clean from main root
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("clean")
        .current_dir(&root)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "clean failed: {stdout}");
    assert!(
        !stdout.contains("SUB1"),
        "worktree under .wt directory must not be listed: {stdout}"
    );
    assert!(
        !stdout.contains("wip/SUB1"),
        "worktree branch wip/SUB1 must not be listed: {stdout}"
    );
    assert!(
        !stdout.contains(".wt"),
        "stdout should not list any .wt worktree in other worktrees report: {stdout}"
    );
}
