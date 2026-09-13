use std::fs;
use std::path::Path;
use superstate_core::daemon::{DaemonConfig, DaemonRunner};

pub fn is_pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let mut status: libc::c_int = 0;
    let wait_res = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    if wait_res == pid {
        // Child exited and was reaped
        return false;
    }
    if wait_res == 0 {
        // Child is still running
        return true;
    }
    // ECHILD (not our child) or other error: check kill(pid, 0)
    unsafe { libc::kill(pid, 0) == 0 }
}

fn resolve_superstate_binary() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let name = exe.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if (name == "superstate" || name == "superstate.exe") && !name.contains('-') {
            return exe;
        }
        if let Some(parent) = exe.parent() {
            if parent.file_name().and_then(|s| s.to_str()) == Some("deps") {
                if let Some(target_dir) = parent.parent() {
                    let cand = target_dir.join("superstate");
                    if cand.exists() {
                        return cand;
                    }
                }
            }
        }
        return exe;
    }
    std::path::PathBuf::from("superstate")
}

pub fn cmd_daemon(
    state_path: &Path,
    driver: Option<&str>,
    detach: bool,
    max_retries: usize,
    timeout_secs: u64,
) -> Result<(), String> {
    let state_dir = state_path.parent().unwrap_or_else(|| Path::new("."));
    let pid_file = state_dir.join("daemon.pid");
    let log_dir = state_dir.join("logs");
    let log_file = log_dir.join("daemon.log");

    if detach {
        fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
        let log_out = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .map_err(|e| format!("Failed to open log file {}: {}", log_file.display(), e))?;
        let log_err = log_out
            .try_clone()
            .map_err(|e| format!("Failed to clone log handle: {}", e))?;

        let current_exe = resolve_superstate_binary();

        let mut cmd = std::process::Command::new(current_exe);



        cmd.arg("state").arg("daemon");
        cmd.arg("--state").arg(state_path);
        if let Some(d) = driver {
            cmd.arg("--driver").arg(d);
        }
        cmd.arg("--max-retries").arg(max_retries.to_string());
        cmd.arg("--timeout").arg(timeout_secs.to_string());
        cmd.stdout(std::process::Stdio::from(log_out));
        cmd.stderr(std::process::Stdio::from(log_err));
        cmd.stdin(std::process::Stdio::null());

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }



        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn background daemon: {}", e))?;
        let pid = child.id();
        fs::write(&pid_file, pid.to_string())
            .map_err(|e| format!("Failed to write pid file {}: {}", pid_file.display(), e))?;

        println!("Superstate daemon spawned in background (PID: {})", pid);
        println!("Logs: {}", log_file.display());
        Ok(())
    } else {
        let pid = std::process::id();
        fs::write(&pid_file, pid.to_string())
            .map_err(|e| format!("Failed to write pid file {}: {}", pid_file.display(), e))?;

        println!("Daemon runner started for state: {}", state_path.display());
        use std::io::Write;
        let _ = std::io::stdout().flush();

        let config = DaemonConfig {


            state_path: state_path.to_path_buf(),
            driver_cmd: driver.map(ToString::to_string),
            max_retries,
            timeout_secs,
        };

        let mut runner = DaemonRunner::new(config).map_err(|e| e.to_string())?;
        let res = runner.run_to_completion();

        let _ = fs::remove_file(&pid_file);

        match res {
            Ok(report) => {
                println!(
                    "Daemon completed: {} milestones completed (status: {:?})",
                    report.milestones_completed, report.status
                );
                Ok(())
            }
            Err(e) => {
                eprintln!("Daemon execution error: {}", e);
                Err(e.to_string())
            }
        }
    }
}

pub fn cmd_cancel(state_path: &Path) -> Result<(), String> {
    let state_dir = state_path.parent().unwrap_or_else(|| Path::new("."));
    let pid_file = state_dir.join("daemon.pid");
    if !pid_file.exists() {
        println!("No active daemon PID file found at {}", pid_file.display());
        return Ok(());
    }

    let pid_str = fs::read_to_string(&pid_file).map_err(|e| e.to_string())?;
    let pid: i32 = pid_str
        .trim()
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;

    if is_pid_alive(pid) {
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if !is_pid_alive(pid) {
                break;
            }
        }
        if is_pid_alive(pid) {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
            for _ in 0..10 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                if !is_pid_alive(pid) {
                    break;
                }
            }
        }
    }

    let _ = fs::remove_file(&pid_file);
    println!("Daemon process {} terminated.", pid);
    Ok(())
}

pub fn cmd_logs(state_path: &Path, tail: Option<usize>) -> Result<String, String> {
    let state_dir = state_path.parent().unwrap_or_else(|| Path::new("."));
    let log_file = state_dir.join("logs").join("daemon.log");

    if !log_file.exists() {
        return Ok("No daemon logs found.".to_string());
    }

    let content = fs::read_to_string(&log_file)
        .map_err(|e| format!("Failed to read {}: {}", log_file.display(), e))?;

    if let Some(n) = tail {
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(n);
        let tail_lines = lines[start..].join("\n");
        println!("{}", tail_lines);
        Ok(tail_lines)
    } else {
        println!("{}", content);
        Ok(content)
    }
}
