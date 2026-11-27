#![cfg(unix)]

use std::time::{Duration, Instant};

use blyte_agent::shell::{LocalShell, PersistentShell, Shell};

const CMD: &str = "sleep 60 > /dev/null 2>&1 & echo $!";
const TIMEOUT: Duration = Duration::from_secs(60);
const RETURN_BOUND: Duration = Duration::from_secs(30);

struct PidGuard(i32);

impl Drop for PidGuard {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill")
            .args(["-9", &self.0.to_string()])
            .status();
    }
}

fn parse_pid(stdout: &str) -> i32 {
    stdout
        .trim()
        .parse::<i32>()
        .unwrap_or_else(|e| panic!("expected pid in stdout {stdout:?}: {e}"))
}

fn is_alive(pid: i32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn session_keeps_a_redirected_background_job_alive() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let shell = PersistentShell::local(root.clone());
    assert!(shell.is_persistent(), "expected persistent session");
    shell.exec_user("true", &root, TIMEOUT).unwrap();

    let start = Instant::now();
    let out = shell.exec_user(CMD, &root, TIMEOUT).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(elapsed < RETURN_BOUND, "took {elapsed:?}");
    let pid = parse_pid(&out.stdout);
    assert!(pid > 0, "unexpected pid {pid}");
    let _guard = PidGuard(pid);

    std::thread::sleep(Duration::from_millis(150));
    assert!(
        is_alive(pid),
        "expected background job {pid} to still be alive after 150ms in persistent session"
    );
}

#[test]
fn per_command_executor_keeps_a_redirected_background_job_alive() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let shell = LocalShell::new();

    let start = Instant::now();
    let out = shell.exec_user(CMD, &root, TIMEOUT).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(elapsed < RETURN_BOUND, "took {elapsed:?}");
    let pid = parse_pid(&out.stdout);
    assert!(pid > 0, "unexpected pid {pid}");
    let _guard = PidGuard(pid);

    std::thread::sleep(Duration::from_millis(150));
    assert!(
        is_alive(pid),
        "expected background job {pid} to still be alive after 150ms in per-command executor"
    );
}
