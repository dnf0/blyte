use std::ffi::OsString;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use blyte_agent::fs::LocalFs;
use blyte_agent::redact::{parse_env_file, Redactor, MIN_SECRET_BYTES};
use blyte_agent::run::run_gate_step;
use blyte_agent::shell::{LocalShell, HARNESS_ENV};
use blyte_agent::tools::execute_with_timeout;
use serde_json::json;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: String,
    previous: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self {
            key: key.to_string(),
            previous,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(v) => std::env::set_var(&self.key, v),
            None => std::env::remove_var(&self.key),
        }
    }
}

#[test]
fn replaces_known_value_with_redacted_name() {
    let redactor = Redactor::from_pairs(vec![("A".into(), "supersecretvalue".into())]);
    assert_eq!(redactor.apply("x supersecretvalue y"), "x [redacted A] y");
}

#[test]
fn drops_values_shorter_than_min_secret_bytes() {
    assert_eq!(MIN_SECRET_BYTES, 8);
    let redactor = Redactor::from_pairs(vec![
        ("SHORT".into(), "1234567".into()),
        ("EXACT".into(), "12345678".into()),
    ]);
    assert_eq!(
        redactor.apply("keep 1234567 hide 12345678"),
        "keep 1234567 hide [redacted EXACT]"
    );
}

#[test]
fn longest_value_wins_before_shorter_substring() {
    let redactor = Redactor::from_pairs(vec![
        ("A".into(), "abcdefgh".into()),
        ("B".into(), "abcdefghijkl".into()),
    ]);
    assert_eq!(redactor.apply("abcdefghijkl"), "[redacted B]");
}

#[test]
fn duplicate_values_keep_first_sorted_name_and_do_not_re_redact_placeholders() {
    let redactor = Redactor::from_pairs(vec![
        ("Z_KEY".into(), "same-secret-value".into()),
        ("A_KEY".into(), "same-secret-value".into()),
        ("WORD".into(), "redacted".into()),
    ]);
    assert_eq!(
        redactor.apply("same-secret-value and redacted"),
        "[redacted A_KEY] and [redacted WORD]"
    );
}

#[test]
fn parse_env_file_handles_export_quotes_comments_and_plain_lines() {
    let input = "export API_TOKEN=\"tok_12345678\"\n# c\nPLAIN\nHOST=example";
    assert_eq!(
        parse_env_file(input),
        vec![
            ("API_TOKEN".to_string(), "tok_12345678".to_string()),
            ("HOST".to_string(), "example".to_string()),
        ]
    );
    assert_eq!(
        parse_env_file("\n  # indented\nSINGLE='val_12345678'\n"),
        vec![("SINGLE".to_string(), "val_12345678".to_string())]
    );
}

#[test]
fn for_root_redacts_credential_like_env_file_entries_only() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".env"),
        "API_TOKEN=tok_12345678\nPROJECT_NAME=demo-project-name\n",
    )
    .unwrap();

    let redactor = Redactor::for_root(dir.path()).unwrap();
    assert_eq!(
        redactor.apply("token=tok_12345678 project=demo-project-name"),
        "token=[redacted API_TOKEN] project=demo-project-name"
    );
}

#[test]
fn for_root_errors_naming_path_when_dotenv_is_a_directory() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let env_dir = dir.path().join(".env");
    std::fs::create_dir(&env_dir).unwrap();

    let err = Redactor::for_root(dir.path()).unwrap_err();
    assert!(
        err.contains(&env_dir.display().to_string()),
        "expected path in error, got: {err}"
    );
}

#[test]
fn execute_with_timeout_redacts_dotenv_token_from_read_tool() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), "API_TOKEN=tok_12345678\n").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "found tok_12345678 here\n").unwrap();

    let outcome = execute_with_timeout(
        dir.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({ "path": "notes.txt" }),
        Duration::from_secs(30),
    );
    assert!(!outcome.is_error, "unexpected error: {}", outcome.content);
    assert!(
        outcome.content.contains("[redacted API_TOKEN]"),
        "got: {}",
        outcome.content
    );
    assert!(
        !outcome.content.contains("tok_12345678"),
        "token leaked: {}",
        outcome.content
    );
}

#[test]
fn execute_with_timeout_errors_when_dotenv_cannot_be_read() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let env_dir = dir.path().join(".env");
    std::fs::create_dir(&env_dir).unwrap();
    std::fs::write(dir.path().join("notes.txt"), "hello\n").unwrap();

    let outcome = execute_with_timeout(
        dir.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({ "path": "notes.txt" }),
        Duration::from_secs(30),
    );
    assert!(outcome.is_error);
    assert!(
        outcome.content.contains(&env_dir.display().to_string()),
        "got: {}",
        outcome.content
    );
}

#[test]
fn run_gate_step_redacts_secrets_and_fails_on_unreadable_dotenv() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), "API_TOKEN=tok_12345678\n").unwrap();

    let (passed, out) = run_gate_step(
        &LocalShell,
        "echo leaked tok_12345678",
        dir.path(),
        None,
        &Instant::now,
    );
    assert!(passed);
    assert!(out.contains("[redacted API_TOKEN]"), "got: {out}");
    assert!(!out.contains("tok_12345678"), "token leaked: {out}");

    let bad_dir = tempfile::tempdir().unwrap();
    let env_dir = bad_dir.path().join(".env");
    std::fs::create_dir(&env_dir).unwrap();

    let (passed, out) = run_gate_step(&LocalShell, "echo ok", bad_dir.path(), None, &Instant::now);
    assert!(!passed);
    assert!(
        out.contains(&env_dir.display().to_string()),
        "expected path in gate output, got: {out}"
    );
}

#[test]
fn env_sources_redact_harness_and_withheld_values() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let harness_var = HARNESS_ENV[0];
    let _harness_guard = EnvGuard::set(harness_var, "key-abcdefgh-123456");
    let _withheld_guard = EnvGuard::set("ZZ_REDACT_TEST_TOKEN", "withheld-secret-999");

    let dir = tempfile::tempdir().unwrap();
    let redactor = Redactor::for_root(dir.path()).unwrap();
    assert_eq!(
        redactor.apply("a key-abcdefgh-123456 b withheld-secret-999 c"),
        format!("a [redacted {harness_var}] b [redacted ZZ_REDACT_TEST_TOKEN] c")
    );
}
