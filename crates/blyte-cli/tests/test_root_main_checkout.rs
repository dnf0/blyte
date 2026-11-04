use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, OnceLock};

fn cwd_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct DirGuard(PathBuf);

impl DirGuard {
    fn chdir(target: &Path) -> Self {
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(target).unwrap();
        Self(prev)
    }
}

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn rev_parse(dir: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", rev])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git rev-parse {rev}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn blyte_in(dir: &Path, home: &Path, state_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("BLYTE_STATE_DIR", state_dir)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("BLYTE_ENV_FILE")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

#[test]
fn find_repo_root_returns_main_checkout_from_all_worktrees() {
    let _guard = cwd_lock().lock().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let main = parent.path().join("myrepo");
    std::fs::create_dir_all(&main).unwrap();

    git(&main, &["init", "-q"]);
    std::fs::write(main.join("f.txt"), "hello\n").unwrap();
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let main_canon = std::fs::canonicalize(&main).unwrap();

    let wt_plan = main.join(".wt/plan-x");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wip/plan-x",
            wt_plan.to_str().unwrap(),
        ],
    );

    let sibling = parent.path().join("myrepo--topic");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "topic",
            sibling.to_str().unwrap(),
        ],
    );

    let claude_wt = main.join(".claude/worktrees/x");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "claude-x",
            claude_wt.to_str().unwrap(),
        ],
    );

    for dir in [&main, &wt_plan, &sibling, &claude_wt] {
        let _d = DirGuard::chdir(dir);
        let resolved = blyte_cli::clean::find_repo_root().unwrap();
        assert_eq!(
            std::fs::canonicalize(&resolved).unwrap(),
            main_canon,
            "from {}",
            dir.display()
        );
    }
}

#[test]
fn find_repo_root_errors_when_linked_worktree_main_checkout_cannot_be_resolved() {
    let _guard = cwd_lock().lock().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let bare = parent.path().join("myrepo.git");
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare"]);

    let linked = parent.path().join("myrepo--from-bare");
    git(
        &bare,
        &[
            "worktree",
            "add",
            "-q",
            "--orphan",
            "-b",
            "main",
            linked.to_str().unwrap(),
        ],
    );

    let _d = DirGuard::chdir(&linked);
    let err = blyte_cli::clean::find_repo_root().unwrap_err();
    assert!(
        err.contains("myrepo--from-bare"),
        "expected error naming folder {}, got: {err}",
        linked.display()
    );
}

#[test]
fn dispatch_from_sibling_worktree_uses_main_wt_and_registry_and_sibling_base() {
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().join("home");
    let state_dir = parent.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();

    let main = parent.path().join("myrepo");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q"]);
    std::fs::write(main.join("f.txt"), "main\n").unwrap();
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "main commit", "--no-gpg-sign"]);
    let main_sha = rev_parse(&main, "HEAD");

    let sibling = parent.path().join("myrepo--topic");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "topic",
            sibling.to_str().unwrap(),
        ],
    );
    std::fs::write(sibling.join("topic.txt"), "topic\n").unwrap();
    git(&sibling, &["add", "."]);
    git(
        &sibling,
        &["commit", "-qm", "topic commit", "--no-gpg-sign"],
    );
    let topic_sha = rev_parse(&sibling, "HEAD");
    assert_ne!(main_sha, topic_sha);

    let task_path = sibling.join("task.md");
    std::fs::write(&task_path, "Files: `f.txt`\nCommit: `feat: d1`\n").unwrap();

    // 1. Dispatch without --base from the sibling worktree.
    let out = blyte_in(
        &sibling,
        &home,
        &state_dir,
        &[
            "dispatch",
            "D1",
            task_path.to_str().unwrap(),
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    assert!(
        !out.stdout.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Run folder is <main>/.wt/D1, not <sibling>/.wt/D1.
    let run_wt = main.join(".wt/D1");
    assert!(run_wt.exists(), "expected {} to exist", run_wt.display());
    assert!(
        !sibling.join(".wt").exists(),
        "sibling should not have its own .wt"
    );

    // Registry contains <main>.
    let repos_file = state_dir.join("repos");
    let repos_content = std::fs::read_to_string(&repos_file).unwrap();
    let main_canon = std::fs::canonicalize(&main).unwrap();
    let sibling_canon = std::fs::canonicalize(&sibling).unwrap();
    let registered: Vec<PathBuf> = repos_content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| std::fs::canonicalize(l.trim()).unwrap())
        .collect();
    assert!(
        registered.contains(&main_canon),
        "expected registry to contain {}, got {:?}",
        main_canon.display(),
        registered
    );
    assert!(
        !registered.contains(&sibling_canon),
        "registry should not contain sibling {}",
        sibling_canon.display()
    );

    // Run's base defaults to the sibling worktree's HEAD (topic_sha), not main's HEAD.
    let start_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(main.join(".wt/D1.start.json")).unwrap())
            .unwrap();
    assert_eq!(start_json["base_sha"], topic_sha);
    assert_eq!(rev_parse(&run_wt, "HEAD"), topic_sha);

    // 2. Dispatch with explicit --base from the sibling worktree still honours --base.
    let out2 = blyte_in(
        &sibling,
        &home,
        &state_dir,
        &[
            "dispatch",
            "D2",
            task_path.to_str().unwrap(),
            "--base",
            &main_sha,
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    assert!(
        !out2.stdout.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let start2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(main.join(".wt/D2.start.json")).unwrap())
            .unwrap();
    assert_eq!(start2["base_sha"], main_sha);
    assert_eq!(rev_parse(&main.join(".wt/D2"), "HEAD"), main_sha);
}

#[test]
fn review_status_and_wait_from_linked_worktree_find_run_under_main_wt() {
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().join("home");
    let state_dir = parent.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();

    let main = parent.path().join("myrepo");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q"]);
    std::fs::write(main.join("f.txt"), "base\n").unwrap();
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "init", "--no-gpg-sign"]);
    let base_sha = rev_parse(&main, "HEAD");

    // Create a linked sibling worktree and a .claude/worktrees/x worktree.
    let sibling = parent.path().join("myrepo--topic");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "topic",
            sibling.to_str().unwrap(),
        ],
    );

    // Set up a finished run R1 under <main>/.wt/.
    let run_wt = main.join(".wt/R1");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wip/R1",
            run_wt.to_str().unwrap(),
        ],
    );
    std::fs::write(run_wt.join("f.txt"), "changed\n").unwrap();
    git(&run_wt, &["add", "f.txt"]);
    git(&run_wt, &["commit", "-qm", "feat: r1", "--no-gpg-sign"]);

    std::fs::write(
        main.join(".wt/R1.start.json"),
        serde_json::json!({
            "id": "R1",
            "pid": 0,
            "started_at": 1000,
            "base_sha": base_sha,
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        main.join(".wt/R1.report.json"),
        serde_json::json!({
            "outcome": "verified",
            "turns": 3,
            "cost_usd": 0.05,
            "duration_seconds": 12.0,
            "final_message": "done"
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        main.join(".wt/R1.json"),
        serde_json::json!({
            "outcome": "verified",
            "turns": 3,
            "cost_usd": 0.05
        })
        .to_string(),
    )
    .unwrap();

    // 1. `blyte review R1` started from the linked worktree finds <main>/.wt/R1.
    let rev_out = blyte_in(&sibling, &home, &state_dir, &["review", "R1"]);
    assert_eq!(
        rev_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&rev_out.stderr)
    );
    let rev_stdout = String::from_utf8_lossy(&rev_out.stdout);
    assert!(rev_stdout.contains("feat: r1"), "{rev_stdout}");
    assert!(rev_stdout.contains("outcome: verified"), "{rev_stdout}");

    // 2. `blyte status R1` started from the linked worktree finds <main>/.wt/R1.
    let status_out = blyte_in(&sibling, &home, &state_dir, &["status", "R1"]);
    assert_eq!(
        status_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&status_out.stderr)
    );
    let status_stdout = String::from_utf8_lossy(&status_out.stdout);
    assert!(status_stdout.contains("id: R1"), "{status_stdout}");
    assert!(status_stdout.contains("state: finished"), "{status_stdout}");
    assert!(
        status_stdout.contains("outcome: verified"),
        "{status_stdout}"
    );

    // 3. `blyte wait R1` started from the linked worktree finds <main>/.wt/R1.
    let wait_out = blyte_in(&sibling, &home, &state_dir, &["wait", "R1"]);
    assert_eq!(
        wait_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&wait_out.stderr)
    );
    let wait_stdout = String::from_utf8_lossy(&wait_out.stdout);
    assert_eq!(wait_stdout.trim(), "R1 verified 3 $0.05");
}

#[test]
fn find_repo_root_treats_a_submodule_as_its_own_main_checkout() {
    let _guard = cwd_lock().lock().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let sub_src = parent.path().join("sub-src");
    std::fs::create_dir_all(&sub_src).unwrap();
    git(&sub_src, &["init", "-q", "-b", "main"]);
    git(&sub_src, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let superproject = parent.path().join("super");
    std::fs::create_dir_all(&superproject).unwrap();
    git(&superproject, &["init", "-q", "-b", "main"]);
    git(
        &superproject,
        &["commit", "-q", "--allow-empty", "-m", "init"],
    );
    git(
        &superproject,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            sub_src.to_str().unwrap(),
            "sub",
        ],
    );
    let sub = superproject.join("sub");
    assert!(
        sub.join(".git").is_file(),
        "submodule should have a .git file"
    );

    let _d = DirGuard::chdir(&sub);
    let root = blyte_cli::clean::find_repo_root().unwrap();
    assert_eq!(
        root.canonicalize().unwrap(),
        sub.canonicalize().unwrap(),
        "a submodule is its own root"
    );
}
