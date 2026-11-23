use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ directory")
        .parent()
        .expect("repository root")
        .to_path_buf()
}

fn run_script(args: &[&str]) -> Output {
    let root = repo_root();
    let script = root.join("scripts/test-changed.py");
    Command::new("python3")
        .arg(&script)
        .args(args)
        .current_dir(&root)
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", script.display()))
}

fn run_with_files(files: &[&str]) -> Output {
    let tmp = tempfile::tempdir().expect("tempdir");
    let list_path = tmp.path().join("changed.txt");
    let mut content = files.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    std::fs::write(&list_path, content).expect("write file list");
    run_script(&["--print", "--files-from", list_path.to_str().unwrap()])
}

fn assert_packages(files: &[&str], expected_first_line: &str) {
    let out = run_with_files(files);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "expected exit 0 for {files:?}\nstdout: {stdout}\nstderr: {stderr}"
    );
    let first_line = stdout.lines().next().unwrap_or("");
    assert_eq!(
        first_line, expected_first_line,
        "unexpected first line for {files:?}\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn cli_source_affects_only_blyte() {
    assert_packages(&["crates/blyte-cli/src/main.rs"], "packages: blyte");
}

#[test]
fn cli_embedded_markdown_affects_blyte() {
    assert_packages(
        &["crates/blyte-cli/assets/orchestrator/process.md"],
        "packages: blyte",
    );
}

#[test]
fn agent_source_affects_blyte_and_agent() {
    assert_packages(
        &["crates/blyte-agent/src/lib.rs"],
        "packages: blyte blyte-agent",
    );
}

#[test]
fn provider_source_affects_all() {
    assert_packages(&["crates/blyte-provider/src/lib.rs"], "packages: (all)");
}

#[test]
fn docs_and_root_readme_affect_nothing() {
    assert_packages(&["docs/usage.md", "README.md"], "packages: (none)");
}

#[test]
fn empty_list_affects_nothing() {
    assert_packages(&[], "packages: (none)");
}

#[test]
fn cargo_lock_affects_all() {
    assert_packages(&["Cargo.lock"], "packages: (all)");
}

#[test]
fn scripts_file_affects_all() {
    assert_packages(&["scripts/anything.sh"], "packages: (all)");
}

#[test]
fn docs_plus_cli_source_affects_only_blyte() {
    assert_packages(
        &["docs/usage.md", "crates/blyte-cli/src/main.rs"],
        "packages: blyte",
    );
}

#[test]
fn missing_files_from_path_exits_2_and_names_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("does-not-exist.txt");
    let missing_str = missing.to_str().unwrap();
    let out = run_script(&["--print", "--files-from", missing_str]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains(missing_str),
        "expected stderr to contain {missing_str:?}, got: {stderr}"
    );
}

#[test]
fn unknown_argument_exits_2() {
    let out = run_script(&["--nope"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "stderr: {stderr}");
}
