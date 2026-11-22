use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use blyte_cli::cli::EditFinishArgs;
use blyte_cli::combine::{CombineCheck, CombineRecord, CombineState};
use blyte_cli::requests::Readiness;
use tempfile::TempDir;

fn blyte_bin() -> &'static str {
    env!("CARGO_BIN_EXE_blyte")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git failed to launch");
    assert!(
        out.status.success(),
        "git {:?} failed in {}:\nstdout: {}\nstderr: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn run_blyte(dir: &Path, args: &[&str]) -> Output {
    Command::new(blyte_bin())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("blyte failed to launch")
}

fn setup_repo(toml_body: &str) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Test User"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    std::fs::write(root.join("blyte.toml"), toml_body).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial commit"]);
    dir
}

fn branch_exists(root: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(root)
        .status()
        .expect("git show-ref")
        .success()
}

#[test]
fn edit_start_creates_worktree_branch_task_and_edit_record() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();
    let head_sha = git(root, &["rev-parse", "HEAD"]);

    let out = run_blyte(
        root,
        &["edit-start", "e-one", "--task", "Fix README greeting"],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("edit-start stdout json");
    assert_eq!(parsed["id"], "e-one");
    assert_eq!(parsed["base_sha"], head_sha);
    let printed_path = PathBuf::from(parsed["path"].as_str().expect("path string"));
    assert!(printed_path.is_absolute());
    assert_eq!(
        std::fs::canonicalize(&printed_path).unwrap(),
        std::fs::canonicalize(root.join(".wt/e-one")).unwrap()
    );

    assert!(root.join(".wt/e-one").is_dir());
    assert!(branch_exists(root, "wip/e-one"));
    assert_eq!(
        std::fs::read_to_string(root.join(".wt/e-one.task.md")).unwrap(),
        "Fix README greeting"
    );

    let edit_raw = std::fs::read_to_string(root.join(".wt/e-one.edit.json")).unwrap();
    let edit_val: serde_json::Value = serde_json::from_str(&edit_raw).unwrap();
    assert_eq!(edit_val["id"], "e-one");
    assert_eq!(edit_val["base"], "HEAD");
    assert_eq!(edit_val["base_sha"], head_sha);
    assert!(edit_val["started_at"].as_u64().unwrap() > 0);
    assert_eq!(git(root, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
}

#[test]
fn edit_start_refuses_existing_id_across_all_four_cases() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();
    std::fs::create_dir_all(root.join(".wt")).unwrap();

    std::fs::create_dir_all(root.join(".wt/has-dir")).unwrap();
    let out = run_blyte(root, &["edit-start", "has-dir", "--task", "x"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("has-dir"), "stderr: {err}");

    std::fs::write(root.join(".wt/has-start.start.json"), "{}\n").unwrap();
    let out = run_blyte(root, &["edit-start", "has-start", "--task", "x"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("has-start.start.json"), "stderr: {err}");

    std::fs::write(root.join(".wt/has-edit.edit.json"), "{}\n").unwrap();
    let out = run_blyte(root, &["edit-start", "has-edit", "--task", "x"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("has-edit.edit.json"), "stderr: {err}");

    git(root, &["branch", "wip/has-branch"]);
    let out = run_blyte(root, &["edit-start", "has-branch", "--task", "x"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("wip/has-branch"), "stderr: {err}");
}

#[test]
fn edit_start_refuses_empty_task_bad_base_and_cleans_up_failed_setup() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let out = run_blyte(root, &["edit-start", "empty-task", "--task", "   \n\t "]);
    assert!(!out.status.success());
    assert!(!root.join(".wt/empty-task").exists());
    assert!(!branch_exists(root, "wip/empty-task"));

    let out = run_blyte(
        root,
        &[
            "edit-start",
            "bad-base",
            "--task",
            "change something",
            "--base",
            "does-not-exist-ref",
        ],
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("does-not-exist-ref"), "stderr: {err}");
    assert!(!root.join(".wt/bad-base").exists());
    assert!(!branch_exists(root, "wip/bad-base"));

    let setup_repo_dir =
        setup_repo("[dispatch]\ngate = \"true\"\nsetup = \"echo setup-boom >&2; exit 1\"\n");
    let sroot = setup_repo_dir.path();
    let out = run_blyte(
        sroot,
        &["edit-start", "setup-fail", "--task", "change something"],
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("setup"), "stderr: {err}");
    assert!(!sroot.join(".wt/setup-fail").exists());
    assert!(!sroot.join(".wt/setup-fail.edit.json").exists());
    assert!(!branch_exists(sroot, "wip/setup-fail"));
}

#[test]
fn edit_finish_errors_on_missing_edit_dirty_worktree_and_no_commits() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let out = run_blyte(root, &["edit-finish", "never-started"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("never-started.edit.json"), "stderr: {err}");
    assert!(!root.join(".wt/never-started.start.json").exists());
    assert!(!root.join(".wt/never-started.json").exists());

    let start_out = run_blyte(root, &["edit-start", "e-checks", "--task", "edit readme"]);
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-checks");

    let out = run_blyte(root, &["edit-finish", "e-checks"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no commits"), "stderr: {err}");
    assert!(!root.join(".wt/e-checks.start.json").exists());
    assert!(!root.join(".wt/e-checks.json").exists());

    std::fs::write(wt.join("untracked.txt"), "hi\n").unwrap();
    let out = run_blyte(root, &["edit-finish", "e-checks"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("uncommitted") || err.contains("untracked"),
        "stderr: {err}"
    );
    assert!(!root.join(".wt/e-checks.start.json").exists());
    assert!(!root.join(".wt/e-checks.json").exists());

    std::fs::remove_file(wt.join("untracked.txt")).unwrap();
    std::fs::write(wt.join("README.md"), "modified\n").unwrap();
    let out = run_blyte(root, &["edit-finish", "e-checks"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("uncommitted") || err.contains("untracked"),
        "stderr: {err}"
    );
    assert!(!root.join(".wt/e-checks.start.json").exists());
    assert!(!root.join(".wt/e-checks.json").exists());
}

#[test]
fn edit_finish_gate_failure_and_resume_after_fix() {
    let repo = setup_repo("[dispatch]\ngate = \"test -f ok.txt\"\n");
    let root = repo.path();

    let start_out = run_blyte(root, &["edit-start", "e-resume", "--task", "Add ok.txt"]);
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-resume");

    std::fs::write(wt.join("wrong.txt"), "not ok yet\n").unwrap();
    git(&wt, &["add", "wrong.txt"]);
    git(&wt, &["commit", "-m", "add wrong file"]);

    let fail_out = run_blyte(root, &["edit-finish", "e-resume"]);
    assert_eq!(fail_out.status.code(), Some(1));
    let failed: serde_json::Value =
        serde_json::from_slice(&fail_out.stdout).expect("gate failure json");
    assert_eq!(failed["id"], "e-resume");
    assert_eq!(failed["outcome"], "gate_failed");
    assert_eq!(failed["failing_step"], "test -f ok.txt");
    assert!(failed["output_tail"].is_string());
    assert!(!root.join(".wt/e-resume.start.json").exists());
    assert!(!root.join(".wt/e-resume.json").exists());

    std::fs::write(wt.join("ok.txt"), "now passing\n").unwrap();
    git(&wt, &["add", "ok.txt"]);
    git(&wt, &["commit", "-m", "add ok.txt"]);

    let pass_out = run_blyte(root, &["edit-finish", "e-resume"]);
    assert_eq!(
        pass_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&pass_out.stderr)
    );
    let summary: serde_json::Value =
        serde_json::from_slice(&pass_out.stdout).expect("summary json");
    assert_eq!(summary["outcome"], "verified");
    assert_eq!(summary["exit"], 0);
    assert!(root.join(".wt/e-resume.start.json").is_file());
    assert!(root.join(".wt/e-resume.json").is_file());
}

#[test]
fn edit_finish_pass_records_artifacts_tree_and_refuses_second_finish() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();
    let main_before = git(root, &["rev-parse", "HEAD"]);

    let start_out = run_blyte(
        root,
        &["edit-start", "e-pass", "--task", "Update README.md text"],
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-pass");

    std::fs::write(wt.join("README.md"), "hello world\n").unwrap();
    git(&wt, &["add", "README.md"]);
    git(&wt, &["commit", "-m", "update readme"]);

    let out = run_blyte(root, &["edit-finish", "e-pass", "--request", "req-42"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(summary["id"], "e-pass");
    assert_eq!(summary["outcome"], "verified");
    assert_eq!(summary["exit"], 0);
    assert_eq!(summary["branch"], "wip/e-pass");
    assert_eq!(summary["turns"], 0);
    assert_eq!(summary["cost_usd"], 0.0);
    assert_eq!(summary["final_message"], "Update README.md text");
    assert_eq!(summary["commits"].as_array().unwrap().len(), 1);
    assert_eq!(summary["commits"][0]["subject"], "update readme");
    assert_eq!(summary["changed_files"], serde_json::json!(["README.md"]));

    let start_raw = std::fs::read_to_string(root.join(".wt/e-pass.start.json")).unwrap();
    let start_val: serde_json::Value = serde_json::from_str(&start_raw).unwrap();
    assert_eq!(start_val["kind"], "edit");
    assert_eq!(start_val["request"], "req-42");
    assert_eq!(start_val["base_sha"], main_before);

    assert!(root.join(".wt/e-pass.json").is_file());
    assert!(root.join(".wt/e-pass.checks.json").is_file());

    let head_tree = blyte_cli::passed_trees::tree_of(&wt, "HEAD").unwrap();
    let key = blyte_cli::passed_trees::check_key("true", &BTreeMap::new());
    let now = blyte_cli::passed_trees::now_secs();
    assert!(blyte_cli::passed_trees::has_passed(root, &head_tree, &key, now).unwrap());

    assert_eq!(git(root, &["rev-parse", "HEAD"]), main_before);

    let again = run_blyte(root, &["edit-finish", "e-pass", "--request", "req-42"]);
    assert!(!again.status.success());
    let err = String::from_utf8_lossy(&again.stderr);
    assert!(err.contains("e-pass.json"), "stderr: {err}");
}

#[test]
fn edit_finish_skips_gate_when_tree_already_passed() {
    let counter_dir = TempDir::new().unwrap();
    let counter_file = counter_dir.path().join("gate.count");
    let gate_cmd = format!("echo run >> '{}'", counter_file.display());
    let repo = setup_repo(&format!("[dispatch]\ngate = \"{gate_cmd}\"\n"));
    let root = repo.path();

    let start1 = run_blyte(root, &["edit-start", "e-first", "--task", "Add note"]);
    assert!(start1.status.success());
    let wt1 = root.join(".wt/e-first");
    std::fs::write(wt1.join("note.txt"), "same content\n").unwrap();
    git(&wt1, &["add", "note.txt"]);
    git(&wt1, &["commit", "-m", "add note"]);
    let finish1 = run_blyte(root, &["edit-finish", "e-first"]);
    assert_eq!(finish1.status.code(), Some(0));

    let count_after_first = std::fs::read_to_string(&counter_file)
        .unwrap()
        .lines()
        .count();
    assert_eq!(count_after_first, 1);

    let start2 = run_blyte(root, &["edit-start", "e-second", "--task", "Add same note"]);
    assert!(start2.status.success());
    let wt2 = root.join(".wt/e-second");
    std::fs::write(wt2.join("note.txt"), "same content\n").unwrap();
    git(&wt2, &["add", "note.txt"]);
    git(&wt2, &["commit", "-m", "add same note"]);
    let finish2 = run_blyte(root, &["edit-finish", "e-second"]);
    assert_eq!(finish2.status.code(), Some(0));

    let count_after_second = std::fs::read_to_string(&counter_file)
        .unwrap()
        .lines()
        .count();
    assert_eq!(count_after_second, 1);
}

#[test]
fn edit_finish_recovers_after_crash_between_start_record_and_summary() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let start_out = run_blyte(root, &["edit-start", "e-crash", "--task", "Add file"]);
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-crash");
    std::fs::write(wt.join("added.txt"), "data\n").unwrap();
    git(&wt, &["add", "added.txt"]);
    git(&wt, &["commit", "-m", "add file"]);

    let args = EditFinishArgs {
        id: "e-crash".to_string(),
        request: Some("req-crash".to_string()),
    };
    let crash_hook = || Err("simulated crash before summary".to_string());
    let code = blyte_cli::direct_edit::run_edit_finish_in(
        root,
        root,
        Path::new(blyte_bin()),
        &args,
        Some(&crash_hook),
    );
    assert_eq!(code, 2);
    assert!(root.join(".wt/e-crash.start.json").is_file());
    assert!(!root.join(".wt/e-crash.json").exists());

    let retry = run_blyte(root, &["edit-finish", "e-crash", "--request", "req-crash"]);
    assert_eq!(
        retry.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert!(root.join(".wt/e-crash.json").is_file());
    let summary: serde_json::Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(summary["outcome"], "verified");
}

#[test]
fn edit_finish_integrates_with_request_readiness_and_combine() {
    let repo = setup_repo("[dispatch]\ngate = \"true\"\n");
    let root = repo.path();

    let start_out = run_blyte(
        root,
        &["edit-start", "e-comb", "--task", "Add greeting file"],
    );
    assert!(start_out.status.success());
    let wt = root.join(".wt/e-comb");
    std::fs::write(wt.join("greeting.txt"), "hello\n").unwrap();
    git(&wt, &["add", "greeting.txt"]);
    git(&wt, &["commit", "-m", "add greeting file"]);
    let edit_commit_msg = git(&wt, &["show", "-s", "--format=%s", "HEAD"]);

    let finish_out = run_blyte(root, &["edit-finish", "e-comb", "--request", "req-comb"]);
    assert_eq!(
        finish_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&finish_out.stderr)
    );

    let readiness = blyte_cli::requests::request_readiness(root, "req-comb").unwrap();
    match &readiness {
        Readiness::Ready => {}
        Readiness::Waiting { reasons } => {
            assert!(
                reasons.iter().all(|r| r.contains("flagged")),
                "unexpected waiting reasons: {reasons:?}"
            );
        }
    }

    let mut out_buf = Vec::new();
    let mut err_buf = Vec::new();
    let code = blyte_cli::combine::combine_in(root, "req-comb", &mut out_buf, &mut err_buf);
    assert_eq!(
        code,
        0,
        "combine stderr: {}",
        String::from_utf8_lossy(&err_buf)
    );

    let record: CombineRecord = serde_json::from_slice(&out_buf).expect("combine record json");
    assert_eq!(record.state, CombineState::Combined);
    assert_eq!(record.check, CombineCheck::AlreadyPassed);
    assert_eq!(record.commits.len(), 1);
    let combined_subject = git(root, &["show", "-s", "--format=%s", &record.branch]);
    assert_eq!(combined_subject, edit_commit_msg);
}

#[test]
fn edit_start_honours_explicit_base_and_edit_finish_runs_comment_step_and_env() {
    let repo = setup_repo(
        "[dispatch]\ngate = \"test \\\"$EDIT_ENV_FLAG\\\" = ok\"\n\n[dispatch.env]\nEDIT_ENV_FLAG = \"ok\"\n",
    );
    let root = repo.path();
    let first_sha = git(root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("second.txt"), "second\n").unwrap();
    git(root, &["add", "second.txt"]);
    git(root, &["commit", "-m", "second commit"]);

    let start_out = run_blyte(
        root,
        &[
            "edit-start",
            "e-env",
            "--task",
            "Add lib.rs without comments",
            "--base",
            &first_sha,
        ],
    );
    assert!(
        start_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&start_out.stderr)
    );
    let started: serde_json::Value = serde_json::from_slice(&start_out.stdout).unwrap();
    assert_eq!(started["base_sha"], first_sha);
    let wt = root.join(".wt/e-env");
    assert!(!wt.join("second.txt").exists());

    std::fs::write(
        wt.join("sample.rs"),
        "pub fn f() -> i32 {\n    // bad comment\n    1\n}\n",
    )
    .unwrap();
    git(&wt, &["add", "sample.rs"]);
    git(&wt, &["commit", "-m", "add commented rust file"]);

    let fail_out = run_blyte(root, &["edit-finish", "e-env"]);
    assert_eq!(fail_out.status.code(), Some(1));
    let failed: serde_json::Value = serde_json::from_slice(&fail_out.stdout).unwrap();
    assert_eq!(failed["outcome"], "gate_failed");
    assert!(
        failed["failing_step"]
            .as_str()
            .unwrap()
            .contains("comments"),
        "expected comment step failure, got: {failed}"
    );

    std::fs::write(wt.join("sample.rs"), "pub fn f() -> i32 {\n    1\n}\n").unwrap();
    git(&wt, &["add", "sample.rs"]);
    git(&wt, &["commit", "-m", "remove comment"]);

    let pass_out = run_blyte(root, &["edit-finish", "e-env"]);
    assert_eq!(
        pass_out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&pass_out.stderr)
    );
}
