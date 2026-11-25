use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use blyte_cli::config::load;
use blyte_cli::setup::{run_setup, SETUP_TIMEOUT};

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

#[test]
fn test_1_load_reads_setup() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("blyte.toml"),
        "[dispatch]\ngate = \"true\"\nsetup = \"uv sync\"\n",
    )
    .unwrap();
    let cfg = load(d.path()).unwrap().unwrap();
    assert_eq!(cfg.dispatch.setup.as_deref(), Some("uv sync"));

    let d2 = tempfile::tempdir().unwrap();
    std::fs::write(
        d2.path().join("blyte.toml"),
        "[dispatch]\ngate = \"true\"\n",
    )
    .unwrap();
    let cfg2 = load(d2.path()).unwrap().unwrap();
    assert_eq!(cfg2.dispatch.setup, None);
}

#[test]
fn test_2_run_setup_success_with_env() {
    let wt = tempfile::tempdir().unwrap();
    let main = tempfile::tempdir().unwrap();
    let log_dir = tempfile::tempdir().unwrap();
    let log_path = log_dir.path().join("setup.log");

    let mut env = BTreeMap::new();
    env.insert("FOO".to_string(), "it's".to_string());

    let res = run_setup(
        "echo hi > made.txt && echo \"$BLYTE_MAIN_CHECKOUT|$FOO\"",
        wt.path(),
        main.path(),
        &env,
        &log_path,
        SETUP_TIMEOUT,
    );
    assert_eq!(res, Ok(()));
    assert!(wt.path().join("made.txt").exists());
    assert_eq!(
        std::fs::read_to_string(wt.path().join("made.txt"))
            .unwrap()
            .trim(),
        "hi"
    );

    let log = std::fs::read_to_string(&log_path).unwrap();
    let expected = format!("{}|it's", main.path().display());
    assert!(
        log.contains(&expected),
        "log did not contain {expected:?}: {log:?}"
    );
}

#[test]
fn test_3_run_setup_failure_exit_code() {
    let wt = tempfile::tempdir().unwrap();
    let main = tempfile::tempdir().unwrap();
    let log_dir = tempfile::tempdir().unwrap();
    let log_path = log_dir.path().join("setup.log");
    let env = BTreeMap::new();

    let res = run_setup(
        "echo broken; exit 3",
        wt.path(),
        main.path(),
        &env,
        &log_path,
        SETUP_TIMEOUT,
    );
    let err = res.expect_err("expected Err for exit 3");
    assert!(
        err.starts_with("setup failed (exit 3)"),
        "expected err starting with 'setup failed (exit 3)', got: {err}"
    );
    assert!(
        err.contains("broken"),
        "expected err containing 'broken', got: {err}"
    );

    assert!(log_path.exists(), "log path must exist");
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("broken"),
        "log must contain 'broken', got: {log}"
    );
}

#[test]
fn test_4_output_tail_20_lines() {
    let wt = tempfile::tempdir().unwrap();
    let main = tempfile::tempdir().unwrap();
    let log_dir = tempfile::tempdir().unwrap();
    let log_path = log_dir.path().join("setup.log");
    let env = BTreeMap::new();

    let res = run_setup(
        "seq 1 30; exit 1",
        wt.path(),
        main.path(),
        &env,
        &log_path,
        SETUP_TIMEOUT,
    );
    let err = res.expect_err("expected Err for exit 1");
    for i in 11..=30 {
        assert!(
            err.lines().any(|l| l == i.to_string()),
            "err should hold line {i}: {err}"
        );
    }
    assert!(
        !err.lines().any(|l| l == "10"),
        "err should not hold line 10: {err}"
    );
}

#[test]
fn test_5_run_setup_timeout() {
    let wt = tempfile::tempdir().unwrap();
    let main = tempfile::tempdir().unwrap();
    let log_dir = tempfile::tempdir().unwrap();
    let log_path = log_dir.path().join("setup.log");
    let env = BTreeMap::new();

    let res = run_setup(
        "sleep 5",
        wt.path(),
        main.path(),
        &env,
        &log_path,
        Duration::from_secs(1),
    );
    let err = res.expect_err("expected Err on timeout");
    assert!(
        err.starts_with("setup failed:"),
        "expected err starting with 'setup failed:', got: {err}"
    );
}

#[test]
fn test_6_end_to_end_setup_failure_blocks_dispatch() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();

    git(r, &["init", "-q"]);
    std::fs::write(r.join("initial.txt"), "hello\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "initial", "--no-gpg-sign"]);

    std::fs::write(
        r.join("blyte.toml"),
        "[dispatch]\ngate = \"true\"\nsetup = \"echo broken; exit 3\"\n",
    )
    .unwrap();
    std::fs::write(r.join("task.md"), "implement something\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["dispatch", "S1x", "task.md"])
        .current_dir(r)
        .env("BLYTE_USAGE_LOG", "off")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(5), "expected exit code 5 (blocked)");

    let summary_path = r.join(".wt/S1x.json");
    assert!(summary_path.exists(), ".wt/S1x.json must exist");
    let summary: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&summary_path).unwrap()).unwrap();
    assert_eq!(summary["outcome"], "blocked");
    let final_message = summary["final_message"].as_str().unwrap_or("");
    assert!(
        final_message.contains("broken"),
        "final_message must contain 'broken': {final_message}"
    );

    let setup_log = r.join(".wt/S1x.setup.log");
    assert!(setup_log.exists(), ".wt/S1x.setup.log must exist");

    let agent_log = r.join(".wt/S1x.log");
    assert!(
        !agent_log.exists(),
        ".wt/S1x.log must not exist (implementer never started)"
    );
}

#[test]
fn test_7_failing_setup_preserves_stdout_and_stderr_tails_and_full_log_path() {
    let wt = tempfile::tempdir().unwrap();
    let main = tempfile::tempdir().unwrap();
    let log_dir = tempfile::tempdir().unwrap();
    let log_path = log_dir.path().join("setup.log");
    let env = BTreeMap::new();

    let script = "\
for i in $(seq 1 29); do echo \"out-line-$i\"; done; \
echo \"ROOT_CAUSE_ON_STDOUT_30\"; \
for i in $(seq 1 29); do echo \"err-line-$i\" >&2; done; \
echo \"LAST_STDERR_LINE_30\" >&2; \
exit 1";

    let err = run_setup(
        script,
        wt.path(),
        main.path(),
        &env,
        &log_path,
        SETUP_TIMEOUT,
    )
    .expect_err("expected setup failure");

    assert!(
        err.contains("stdout:\n"),
        "expected stdout: label in error:\n{err}"
    );
    assert!(
        err.contains("stderr:\n"),
        "expected stderr: label in error:\n{err}"
    );
    assert!(
        err.contains("ROOT_CAUSE_ON_STDOUT_30"),
        "expected stdout cause line in error:\n{err}"
    );
    assert!(
        !err.contains("out-line-10\n"),
        "expected only last 20 stdout lines:\n{err}"
    );
    assert!(
        err.contains("out-line-11"),
        "expected stdout line 11 in error:\n{err}"
    );
    assert!(
        err.contains("LAST_STDERR_LINE_30"),
        "expected last stderr line in error:\n{err}"
    );
    assert!(
        !err.contains("err-line-10\n"),
        "expected only last 20 stderr lines:\n{err}"
    );
    assert!(
        err.contains("err-line-11"),
        "expected stderr line 11 in error:\n{err}"
    );
    let expected_footer = format!("full log: {}", log_path.display());
    assert_eq!(
        err.lines().last(),
        Some(expected_footer.as_str()),
        "expected trailing full log line, got:\n{err}"
    );
}

#[test]
fn test_8_failing_setup_omits_empty_stream_labels() {
    let wt = tempfile::tempdir().unwrap();
    let main = tempfile::tempdir().unwrap();
    let log_dir = tempfile::tempdir().unwrap();
    let env = BTreeMap::new();

    let only_stdout_log = log_dir.path().join("stdout-only.log");
    let err_out = run_setup(
        "echo only-stdout; exit 2",
        wt.path(),
        main.path(),
        &env,
        &only_stdout_log,
        SETUP_TIMEOUT,
    )
    .expect_err("expected setup failure");
    assert!(err_out.contains("stdout:\n"), "{err_out}");
    assert!(!err_out.contains("stderr:\n"), "{err_out}");

    let only_stderr_log = log_dir.path().join("stderr-only.log");
    let err_err = run_setup(
        "echo only-stderr >&2; exit 2",
        wt.path(),
        main.path(),
        &env,
        &only_stderr_log,
        SETUP_TIMEOUT,
    )
    .expect_err("expected setup failure");
    assert!(!err_err.contains("stdout:\n"), "{err_err}");
    assert!(err_err.contains("stderr:\n"), "{err_err}");
}

#[test]
fn test_9_unwritable_setup_log_reports_error_on_success_and_failure() {
    let wt = tempfile::tempdir().unwrap();
    let main = tempfile::tempdir().unwrap();
    let dir_as_log = tempfile::tempdir().unwrap();
    let env = BTreeMap::new();

    let err_ok = run_setup(
        "echo ok",
        wt.path(),
        main.path(),
        &env,
        dir_as_log.path(),
        SETUP_TIMEOUT,
    )
    .expect_err("succeeded setup with unwritable log must error");
    assert!(
        err_ok.contains(&dir_as_log.path().display().to_string()),
        "error must name log path: {err_ok}"
    );

    let err_fail = run_setup(
        "echo boom; exit 4",
        wt.path(),
        main.path(),
        &env,
        dir_as_log.path(),
        SETUP_TIMEOUT,
    )
    .expect_err("failed setup with unwritable log must error");
    assert!(
        err_fail.contains("setup failed (exit 4)"),
        "error must include setup failure: {err_fail}"
    );
    assert!(
        err_fail.contains(&dir_as_log.path().display().to_string()),
        "error must name unwritable log path: {err_fail}"
    );
    assert!(
        !err_fail.contains("full log:"),
        "error must not advertise nonexistent log: {err_fail}"
    );
}
