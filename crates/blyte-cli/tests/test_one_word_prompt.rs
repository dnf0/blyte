use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn blyte_cmd(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("BLYTE_USAGE_LOG", "off")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .output()
        .expect("failed to execute blyte")
}

fn get_version(dir: &Path) -> String {
    let out = blyte_cmd(dir, &["--version"]);
    let stdout = String::from_utf8(out.stdout).expect("version output not utf-8");
    stdout
        .trim()
        .strip_prefix("blyte ")
        .expect("version output should start with 'blyte '")
        .to_string()
}

#[test]
fn nosuchcommand_exits_2_with_error_and_version() {
    let tmp = tempfile::tempdir().unwrap();
    let version = get_version(tmp.path());

    let start = Instant::now();
    let out = blyte_cmd(tmp.path(), &["nosuchcommand"]);
    let elapsed = start.elapsed();

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("`nosuchcommand` is not a blyte command"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains(&version),
        "stderr was: {stderr}, expected version: {version}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "command took {:?}, expected < 5s",
        elapsed
    );
}

#[test]
fn trimmed_one_word_prompt_behaves_the_same() {
    let tmp = tempfile::tempdir().unwrap();
    let version = get_version(tmp.path());

    let start = Instant::now();
    let out = blyte_cmd(tmp.path(), &["  cost  "]);
    let elapsed = start.elapsed();

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("`cost` is not a blyte command"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains(&version),
        "stderr was: {stderr}, expected version: {version}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "command took {:?}, expected < 5s",
        elapsed
    );
}

#[test]
fn real_subcommand_cost_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let out = blyte_cmd(tmp.path(), &["cost"]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn multi_word_prompt_is_not_refused_by_command_check() {
    let tmp = tempfile::tempdir().unwrap();
    let out = blyte_cmd(tmp.path(), &["fix the bug"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("is not a blyte command"),
        "stderr should not contain 'is not a blyte command': {stderr}"
    );
}
