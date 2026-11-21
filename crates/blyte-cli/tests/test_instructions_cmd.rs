use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use blyte_agent::orchestrator::{ANSWERS_NOBODY, ANSWERS_PERSON};
use blyte_agent::rules::MAX_RULES_CHARS;
use blyte_agent::run::ORCHESTRATOR_FIXED;
use blyte_cli::orchestrator_text::{DEFAULT_PROCESS, REFERENCE, REFERENCE_DISPATCH_TOOL};

fn blyte_bin() -> &'static str {
    env!("CARGO_BIN_EXE_blyte")
}

fn init_git_repo(dir: &Path) -> PathBuf {
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed");
    dir.canonicalize().expect("canonicalize repo")
}

fn run_instructions(dir: &Path, xdg: &Path, args: &[&str]) -> Output {
    Command::new(blyte_bin())
        .arg("instructions")
        .args(args)
        .current_dir(dir)
        .env("XDG_CONFIG_HOME", xdg)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("run blyte instructions")
}

#[test]
fn default_process_flag_prints_the_file_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let user_file = xdg.path().join("blyte").join("orchestrator.md");
    std::fs::create_dir_all(user_file.parent().unwrap()).unwrap();
    std::fs::write(&user_file, "x".repeat(MAX_RULES_CHARS + 100)).unwrap();

    let out = run_instructions(dir.path(), xdg.path(), &["--default-process"]);
    assert!(
        out.status.success(),
        "expected exit 0, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, DEFAULT_PROCESS);

    let conflict_one_shot =
        run_instructions(dir.path(), xdg.path(), &["--default-process", "--one-shot"]);
    assert!(!conflict_one_shot.status.success());

    let conflict_dispatch = run_instructions(
        dir.path(),
        xdg.path(),
        &["--default-process", "--dispatch-tool"],
    );
    assert!(!conflict_dispatch.status.success());
}

#[test]
fn prints_start_lines_then_the_reference_and_the_default_process() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_git_repo(repo_dir.path());
    let xdg = tempfile::tempdir().unwrap();

    let out = run_instructions(repo_dir.path(), xdg.path(), &[]);
    assert!(
        out.status.success(),
        "expected exit 0, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.starts_with(
            "process: default (built in)\nrepository rules: none\nsandbox: on (a command leaves it only with your approval; 0 allowed in advance)\n\n"
        ),
        "unexpected prefix:\n{stdout}"
    );
    let ref_pos = stdout
        .find(REFERENCE.trim())
        .expect("stdout should contain reference");
    let answers_pos = stdout
        .find(ANSWERS_PERSON)
        .expect("stdout should contain ANSWERS_PERSON");
    let process_pos = stdout
        .find(DEFAULT_PROCESS.trim())
        .expect("stdout should contain default process");
    let fixed_header_pos = stdout
        .find("## Always sent\n")
        .expect("stdout should contain ## Always sent");
    let fixed_pos = stdout
        .find(ORCHESTRATOR_FIXED)
        .expect("stdout should contain ORCHESTRATOR_FIXED");
    let tools_pos = stdout
        .find("## Tool descriptions\n")
        .expect("stdout should contain ## Tool descriptions");
    let footer_pos = stdout
        .find("The request and a description of the environment are added when a run starts.")
        .expect("stdout should contain footer line");

    assert!(ref_pos < answers_pos);
    assert!(answers_pos < process_pos);
    assert!(process_pos < fixed_header_pos);
    assert!(fixed_header_pos < fixed_pos);
    assert!(fixed_pos < tools_pos);
    assert!(tools_pos < footer_pos);
}

#[test]
fn a_user_file_is_named_and_replaces_the_default() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_git_repo(repo_dir.path());
    let xdg = tempfile::tempdir().unwrap();
    let user_file = xdg.path().join("blyte").join("orchestrator.md");
    std::fs::create_dir_all(user_file.parent().unwrap()).unwrap();
    std::fs::write(&user_file, "Custom process body.\n").unwrap();

    let out = run_instructions(repo_dir.path(), xdg.path(), &[]);
    assert!(
        out.status.success(),
        "expected exit 0, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    let expected_prefix = format!(
        "process: {}\nrepository rules: none\nsandbox: on (a command leaves it only with your approval; 0 allowed in advance)\n\n",
        user_file.display()
    );
    assert!(
        stdout.starts_with(&expected_prefix),
        "expected prefix {expected_prefix:?}, got:\n{stdout}"
    );
    assert!(stdout.contains("Custom process body."));
    assert!(!stdout.contains(DEFAULT_PROCESS.trim()));
}

#[test]
fn a_repository_file_is_named_and_included() {
    let repo_dir = tempfile::tempdir().unwrap();
    let repo_root = init_git_repo(repo_dir.path());
    let xdg = tempfile::tempdir().unwrap();
    let repo_file = repo_root.join(".blyte").join("orchestrator.md");
    std::fs::create_dir_all(repo_file.parent().unwrap()).unwrap();
    std::fs::write(&repo_file, "Repository-specific orchestrator rule.\n").unwrap();

    let out = run_instructions(repo_dir.path(), xdg.path(), &[]);
    assert!(
        out.status.success(),
        "expected exit 0, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    let expected_line = format!("repository rules: {}\n", repo_file.display());
    assert!(
        stdout.contains(&expected_line),
        "expected {expected_line:?} in stdout:\n{stdout}"
    );
    assert!(stdout.contains("Repository-specific orchestrator rule."));
}

#[test]
fn one_shot_says_nobody_will_answer() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_git_repo(repo_dir.path());
    let xdg = tempfile::tempdir().unwrap();

    let out = run_instructions(repo_dir.path(), xdg.path(), &["--one-shot"]);
    assert!(
        out.status.success(),
        "expected exit 0, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains(ANSWERS_NOBODY));
    assert!(!stdout.contains(ANSWERS_PERSON));
}

#[test]
fn dispatch_tool_prints_the_tool_reference_and_process_none() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_git_repo(repo_dir.path());
    let xdg = tempfile::tempdir().unwrap();

    let out = run_instructions(repo_dir.path(), xdg.path(), &["--dispatch-tool"]);
    assert!(
        out.status.success(),
        "expected exit 0, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.starts_with(
            "process: none\nrepository rules: none\nsandbox: on (a command leaves it only with your approval; 0 allowed in advance)\n\n"
        ),
        "unexpected prefix:\n{stdout}"
    );
    assert!(stdout.contains(REFERENCE_DISPATCH_TOOL.trim()));
    assert!(!stdout.contains("## Process"));
    assert!(!stdout.contains(REFERENCE.trim()));
}

#[test]
fn lists_every_tool_description() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_git_repo(repo_dir.path());
    let xdg = tempfile::tempdir().unwrap();

    let out = run_instructions(repo_dir.path(), xdg.path(), &[]);
    assert!(
        out.status.success(),
        "expected exit 0, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("## Tool descriptions\n"));
    for spec in blyte_agent::tools::specs() {
        let heading = format!("### {}\n{}", spec.name, spec.description);
        assert!(
            stdout.contains(&heading),
            "missing tool description for `{}` in stdout",
            spec.name
        );
    }
}

#[test]
fn an_oversized_user_file_exits_1_and_names_it() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_git_repo(repo_dir.path());
    let xdg = tempfile::tempdir().unwrap();
    let user_file = xdg.path().join("blyte").join("orchestrator.md");
    std::fs::create_dir_all(user_file.parent().unwrap()).unwrap();
    std::fs::write(&user_file, "x".repeat(MAX_RULES_CHARS + 1)).unwrap();

    let out = run_instructions(repo_dir.path(), xdg.path(), &[]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("error: "),
        "expected error prefix, got: {stderr}"
    );
    assert!(
        stderr.contains(&user_file.display().to_string()),
        "expected user file path in stderr, got: {stderr}"
    );
}

#[test]
fn outside_a_repository_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();

    let out = run_instructions(dir.path(), xdg.path(), &[]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error: not inside a git repository"),
        "unexpected stderr: {stderr}"
    );
}
