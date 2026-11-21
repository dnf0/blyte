use std::process::Command;
use std::time::{Duration, Instant};

use blyte_provider::gemini::{output_with_timeout, output_with_timeout_with_clock};

#[test]
fn output_with_timeout_times_out_and_kills_child() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let pid_file = dir.path().join("child.pid");

    let script = format!(
        "printf '%s\\n' $$ > '{}'; exec sleep 300",
        pid_file.display()
    );
    let mut cmd = Command::new("sh");
    cmd.args(["-c", &script]);

    let base = Instant::now();
    let mut started_recorded = false;
    let pid_for_clock = pid_file.clone();
    let res = output_with_timeout_with_clock(&mut cmd, Duration::from_millis(200), move || {
        if !started_recorded {
            started_recorded = true;
            return base;
        }
        let ready = std::fs::read_to_string(&pid_for_clock)
            .ok()
            .is_some_and(|s| s.ends_with('\n') && s.trim().parse::<i32>().is_ok());
        if ready {
            base + Duration::from_secs(1)
        } else {
            base
        }
    });

    let err = res.expect_err("expected timeout error");
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);

    let pid_str = std::fs::read_to_string(&pid_file).expect("pid file should be written");
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
fn output_with_timeout_uses_real_clock_for_timeout() {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "exec sleep 300"]);

    let err = output_with_timeout(&mut cmd, Duration::from_millis(50))
        .expect_err("expected timeout error");
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn output_with_timeout_succeeds_fast_command() {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "echo hi"]);

    let output =
        output_with_timeout(&mut cmd, Duration::from_secs(60)).expect("command should succeed");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hi\n");
}
