use std::process::Command;
use std::time::{Duration, Instant};

use blyte_provider::gemini::output_with_timeout;

#[test]
fn output_with_timeout_times_out_and_kills_child() {
    let pid_file = std::env::temp_dir().join(format!(
        "blyte_test_gcloud_timeout_{}.pid",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&pid_file);

    let script = format!("echo $$ > '{}'; exec sleep 5", pid_file.display());
    let mut cmd = Command::new("sh");
    cmd.args(["-c", &script]);

    let started = Instant::now();
    let res = output_with_timeout(&mut cmd, Duration::from_millis(200));
    let elapsed = started.elapsed();

    let err = res.expect_err("expected timeout error");
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert!(
        elapsed < Duration::from_secs(2),
        "expected timeout in under 2s, took {elapsed:?}"
    );

    let pid_str = std::fs::read_to_string(&pid_file).expect("pid file should be written");
    let _ = std::fs::remove_file(&pid_file);
    let pid: i32 = pid_str.trim().parse().expect("valid pid");
    let status = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run kill -0");
    assert!(
        !status.success(),
        "child process {pid} should no longer be running"
    );
}

#[test]
fn output_with_timeout_succeeds_fast_command() {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "echo hi"]);

    let output =
        output_with_timeout(&mut cmd, Duration::from_secs(5)).expect("command should succeed");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hi\n");
}
