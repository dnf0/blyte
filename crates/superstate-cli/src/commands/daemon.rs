use std::fs;
use std::path::{Path, PathBuf};
use superstate_core::daemon::{resolve_daemon_slug_and_state_dir, DaemonConfig, DaemonRunner};
use superstate_core::state_machine::{load_state, sanitize_branch_slug, STATE_DIR};

/// Scoped filesystem paths allocated to a daemon executing over a discrete state file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonPaths {
    pub pid_file: PathBuf,
    pub log_file: PathBuf,
    pub prompt_file: PathBuf,
    pub slug: String,
}

/// Resolves scoped daemon paths for a given state machine path.
///
/// Default state (`state.json`) maps to slug `"default"`, PID `.agent-state/daemons/default.pid`,
/// logs `.agent-state/logs/default.log`, and prompts `.agent-state/prompts/default.md`.
/// Feature branches (`branches/<slug>.json`) scope to `.agent-state/daemons/<slug>.pid`.
pub fn daemon_paths_for_state(state_path: &Path) -> DaemonPaths {
    let (slug, base_dir) = resolve_daemon_slug_and_state_dir(state_path);
    let state_dir = if base_dir.file_name().and_then(|s| s.to_str()) == Some(STATE_DIR) {
        base_dir
    } else {
        base_dir.join(STATE_DIR)
    };
    DaemonPaths {
        pid_file: state_dir.join("daemons").join(format!("{}.pid", slug)),
        log_file: state_dir.join("logs").join(format!("{}.log", slug)),
        prompt_file: state_dir.join("prompts").join(format!("{}.md", slug)),
        slug,
    }
}

/// Checks whether an operating system PID is active and alive.
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

/// Sends SIGTERM, waits up to 500ms, and escalates to SIGKILL if process remains alive.
pub fn kill_pid(pid: i32) {
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
}

fn resolve_superstate_binary() -> PathBuf {
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
    PathBuf::from("superstate")
}

fn resolve_state_dir(repo_or_state: Option<&Path>) -> PathBuf {
    if let Some(p) = repo_or_state {
        if p.is_file() {
            let (_, base_dir) = resolve_daemon_slug_and_state_dir(p);
            if base_dir.file_name().and_then(|s| s.to_str()) == Some(STATE_DIR) {
                return base_dir;
            }
            return base_dir.join(STATE_DIR);
        }
        if p.file_name().and_then(|s| s.to_str()) == Some(STATE_DIR) {
            return p.to_path_buf();
        }
        if p.join(STATE_DIR).exists() {
            return p.join(STATE_DIR);
        }
        return p.to_path_buf();
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut cur = Some(cwd.as_path());
    while let Some(d) = cur {
        if d.join(STATE_DIR).exists() {
            return d.join(STATE_DIR);
        }
        if d.join(".git").exists() {
            return d.join(STATE_DIR);
        }
        cur = d.parent();
    }
    cwd.join(STATE_DIR)
}

/// Spawns or executes an autonomous daemon runner for a given state machine.
pub fn cmd_daemon(
    state_path: &Path,
    driver: Option<&str>,
    detach: bool,
    max_retries: usize,
    timeout_secs: u64,
) -> Result<(), String> {
    let paths = daemon_paths_for_state(state_path);

    // Enforce per-job PID lock
    if paths.pid_file.exists() {
        if let Ok(pid_str) = fs::read_to_string(&paths.pid_file) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                if pid != std::process::id() as i32 && is_pid_alive(pid) {
                    return Err(format!(
                        "A daemon for '{}' is already running (PID: {})",
                        paths.slug, pid
                    ));
                }
            }
        }
    }

    if detach {
        if let Some(parent) = paths.log_file.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if let Some(parent) = paths.pid_file.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let log_out = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&paths.log_file)
            .map_err(|e| {
                format!(
                    "Failed to open log file {}: {}",
                    paths.log_file.display(),
                    e
                )
            })?;
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
        fs::write(&paths.pid_file, pid.to_string()).map_err(|e| {
            format!(
                "Failed to write pid file {}: {}",
                paths.pid_file.display(),
                e
            )
        })?;

        if let Some(parent) = paths.pid_file.parent().and_then(|p| p.parent()) {
            let _ = fs::write(parent.join("daemon.pid"), pid.to_string());
        }
        if let Some(parent) = state_path.parent() {
            let _ = fs::write(parent.join("daemon.pid"), pid.to_string());
        }

        println!(
            "Superstate daemon spawned in background for '{}' (PID: {})",
            paths.slug, pid
        );
        println!("Logs: {}", paths.log_file.display());
        Ok(())
    } else {
        if let Some(parent) = paths.pid_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let pid = std::process::id();
        fs::write(&paths.pid_file, pid.to_string()).map_err(|e| {
            format!(
                "Failed to write pid file {}: {}",
                paths.pid_file.display(),
                e
            )
        })?;

        println!(
            "Daemon runner started for state: {} (slug: {})",
            state_path.display(),
            paths.slug
        );
        use std::io::Write;
        let _ = std::io::stdout().flush();

        let config = DaemonConfig {
            state_path: state_path.to_path_buf(),
            driver_cmd: driver.map(ToString::to_string),
            max_retries,
            timeout_secs,
        };

        let mut runner = DaemonRunner::new(config)
            .map_err(|e| e.to_string())?
            .with_step_advancer(Box::new(|p| {
                crate::commands::state::cmd_next(p, None, false, None)
            }));
        let res = runner.run_to_completion();

        let _ = fs::remove_file(&paths.pid_file);

        match res {
            Ok(report) => {
                println!(
                    "Daemon completed for '{}': {} milestones completed (status: {:?})",
                    paths.slug, report.milestones_completed, report.status
                );
                Ok(())
            }
            Err(e) => {
                eprintln!("Daemon execution error for '{}': {}", paths.slug, e);
                Err(e.to_string())
            }
        }
    }
}

/// Metadata entry representing a running or registered daemon process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DaemonListEntry {
    pub slug: String,
    pub pid: i32,
    pub alive: bool,
    pub branch: Option<String>,
    pub status: Option<String>,
    pub active_task: Option<String>,
    pub state_file: Option<PathBuf>,
    pub log_file: PathBuf,
}

/// Scans `.agent-state/daemons/*.pid` and correlates with state files to display active fleet.
pub fn cmd_daemon_list(repo_root: &Path, as_json: bool) -> Result<(), String> {
    let state_dir = resolve_state_dir(Some(repo_root));
    let daemons_dir = state_dir.join("daemons");

    let mut entries = Vec::new();

    if daemons_dir.exists() {
        if let Ok(dir_entries) = fs::read_dir(&daemons_dir) {
            for entry in dir_entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("pid") {
                    let slug = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    if let Ok(pid_str) = fs::read_to_string(&path) {
                        if let Ok(pid) = pid_str.trim().parse::<i32>() {
                            let alive = is_pid_alive(pid);
                            let log_file = state_dir.join("logs").join(format!("{}.log", slug));

                            // Correlate with state file
                            let (state_file, branch, status, active_task) = if slug == "default" {
                                let sf = state_dir.join("state.json");
                                if let Ok(st) = load_state(&sf) {
                                    (
                                        Some(sf),
                                        st.active_branch
                                            .or(st.base_branch)
                                            .or_else(|| Some("main".to_string())),
                                        Some(format!("{:?}", st.status)),
                                        st.active_task
                                            .as_ref()
                                            .map(|t| format!("{}: {}", t.id, t.title)),
                                    )
                                } else {
                                    (Some(sf), None, None, None)
                                }
                            } else {
                                let sf = state_dir.join("branches").join(format!("{}.json", slug));
                                if let Ok(st) = load_state(&sf) {
                                    (
                                        Some(sf),
                                        st.active_branch
                                            .or(st.base_branch)
                                            .or_else(|| Some(slug.clone())),
                                        Some(format!("{:?}", st.status)),
                                        st.active_task
                                            .as_ref()
                                            .map(|t| format!("{}: {}", t.id, t.title)),
                                    )
                                } else {
                                    (Some(sf), Some(slug.clone()), None, None)
                                }
                            };

                            entries.push(DaemonListEntry {
                                slug,
                                pid,
                                alive,
                                branch,
                                status,
                                active_task,
                                state_file,
                                log_file,
                            });
                        }
                    }
                }
            }
        }
    }

    // Sort entries by slug
    entries.sort_by(|a, b| a.slug.cmp(&b.slug));

    if as_json {
        let rendered = serde_json::to_string_pretty(&entries)
            .map_err(|e| format!("Failed to serialize daemon list: {}", e))?;
        println!("{}", rendered);
        return Ok(());
    }

    if entries.is_empty() {
        println!(
            "No running or registered daemons found in {}",
            state_dir.display()
        );
        return Ok(());
    }

    println!(
        "{:<16} {:<8} {:<8} {:<12} {:<16} ACTIVE TASK",
        "SLUG", "PID", "ALIVE", "STATUS", "BRANCH"
    );
    println!("{}", "-".repeat(80));
    for entry in &entries {
        let alive_str = if entry.alive { "yes" } else { "no (stale)" };
        let status_str = entry.status.as_deref().unwrap_or("-");
        let branch_str = entry.branch.as_deref().unwrap_or("-");
        let task_str = entry.active_task.as_deref().unwrap_or("-");
        println!(
            "{:<16} {:<8} {:<8} {:<12} {:<16} {}",
            entry.slug, entry.pid, alive_str, status_str, branch_str, task_str
        );
    }
    Ok(())
}

/// Cancels running daemon processes by state path, feature branch slug, or across the whole fleet.
pub fn cmd_cancel(
    state_path: Option<&Path>,
    branch: Option<&str>,
    all: bool,
) -> Result<(), String> {
    let state_dir = resolve_state_dir(state_path);

    if all {
        let daemons_dir = state_dir.join("daemons");
        let mut cancelled_count = 0;
        if daemons_dir.exists() {
            if let Ok(dir_entries) = fs::read_dir(&daemons_dir) {
                for entry in dir_entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("pid") {
                        let slug = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string();
                        if let Ok(pid_str) = fs::read_to_string(&path) {
                            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                                kill_pid(pid);
                                let _ = fs::remove_file(&path);
                                println!("Terminated daemon '{}' (PID: {})", slug, pid);
                                cancelled_count += 1;
                            }
                        }
                    }
                }
            }
        }
        let legacy_pid = state_dir.join("daemon.pid");
        if legacy_pid.exists() {
            if let Ok(pid_str) = fs::read_to_string(&legacy_pid) {
                if let Ok(pid) = pid_str.trim().parse::<i32>() {
                    kill_pid(pid);
                    let _ = fs::remove_file(&legacy_pid);
                    println!("Terminated legacy daemon (PID: {})", pid);
                    cancelled_count += 1;
                }
            }
        }
        if cancelled_count == 0 {
            println!("No active daemons found to cancel.");
        }
        return Ok(());
    }

    if let Some(branch_name) = branch {
        let slug = sanitize_branch_slug(branch_name);
        let pid_file = state_dir.join("daemons").join(format!("{}.pid", slug));
        if !pid_file.exists() {
            println!(
                "No active daemon PID file found for branch '{}' ({})",
                branch_name,
                pid_file.display()
            );
            return Ok(());
        }
        let pid_str = fs::read_to_string(&pid_file).map_err(|e| e.to_string())?;
        let pid: i32 = pid_str
            .trim()
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?;
        kill_pid(pid);
        let _ = fs::remove_file(&pid_file);
        println!(
            "Daemon for branch '{}' (PID: {}) terminated.",
            branch_name, pid
        );
        return Ok(());
    }

    // Single state path target
    let effective_path = state_path.unwrap_or_else(|| Path::new(".agent-state/state.json"));
    let paths = daemon_paths_for_state(effective_path);

    if paths.pid_file.exists() {
        let pid_str = fs::read_to_string(&paths.pid_file).map_err(|e| e.to_string())?;
        let pid: i32 = pid_str
            .trim()
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?;
        kill_pid(pid);
        let _ = fs::remove_file(&paths.pid_file);
        println!("Daemon process {} terminated.", pid);
        return Ok(());
    }

    // Fallback to legacy daemon.pid
    let legacy_file = state_dir.join("daemon.pid");
    if legacy_file.exists() {
        let pid_str = fs::read_to_string(&legacy_file).map_err(|e| e.to_string())?;
        let pid: i32 = pid_str
            .trim()
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?;
        kill_pid(pid);
        let _ = fs::remove_file(&legacy_file);
        println!("Daemon process {} terminated.", pid);
        return Ok(());
    }

    println!(
        "No active daemon PID file found at {}",
        paths.pid_file.display()
    );
    Ok(())
}

/// Convenience function to cancel daemon for a single state path.
pub fn cmd_cancel_state(state_path: &Path) -> Result<(), String> {
    cmd_cancel(Some(state_path), None, false)
}

/// Tails or displays background daemon execution logs for a branch or state path.
pub fn cmd_logs(
    state_path: Option<&Path>,
    branch: Option<&str>,
    tail: Option<usize>,
) -> Result<String, String> {
    let state_dir = resolve_state_dir(state_path);

    let log_file = if let Some(branch_name) = branch {
        let slug = sanitize_branch_slug(branch_name);
        state_dir.join("logs").join(format!("{}.log", slug))
    } else {
        let effective_path = state_path.unwrap_or_else(|| Path::new(".agent-state/state.json"));
        let paths = daemon_paths_for_state(effective_path);
        if paths.log_file.exists() {
            paths.log_file
        } else {
            let legacy = state_dir.join("logs").join("daemon.log");
            if legacy.exists() {
                legacy
            } else {
                paths.log_file
            }
        }
    };

    if !log_file.exists() {
        let msg = format!("No daemon logs found at {}.", log_file.display());
        println!("{}", msg);
        return Ok(msg);
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
