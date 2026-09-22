use std::fs;
use std::path::{Path, PathBuf};
use superstate_core::daemon::{resolve_daemon_slug_and_state_dir, DaemonConfig, DaemonRunner};
use superstate_core::state_machine::{load_state, sanitize_branch_slug, STATE_DIR};

pub use superstate_core::daemon::{daemon_paths_for_state, is_pid_alive, DaemonPaths};

/// Sends SIGTERM, waits up to 500ms, and escalates to SIGKILL if process remains alive.
#[cfg(unix)]
pub fn kill_pid(pid: i32) -> Result<(), String> {
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
    Ok(())
}

#[cfg(not(unix))]
pub fn kill_pid(pid: i32) -> Result<(), String> {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .output();
    Ok(())
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

/// Resolves the `.agent-state` directory given an optional repository root, state file path,
/// or ancestor directory.
///
/// If a target directory does not directly contain `.agent-state`, walks upward looking for
/// an ancestor containing `.agent-state` or `.git` to support running daemon commands
/// from subdirectories.
pub fn resolve_state_dir(repo_or_state: Option<&Path>) -> PathBuf {
    let target = if let Some(p) = repo_or_state {
        // If pointing to a json state file or existing file, resolve its state directory
        if p.is_file() || p.extension().and_then(|s| s.to_str()) == Some("json") {
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
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };

    // Walk upward looking for a directory containing .agent-state or .git
    let mut cur = Some(target.as_path());
    while let Some(d) = cur {
        if d.join(STATE_DIR).exists() {
            return d.join(STATE_DIR);
        }
        if d.join(".git").exists() {
            return d.join(STATE_DIR);
        }
        cur = d.parent();
    }
    target.join(STATE_DIR)
}

/// Spawns or executes an autonomous daemon runner for a given state machine.
#[allow(clippy::too_many_arguments)]
pub fn cmd_daemon(
    state_path: &Path,
    driver: Option<&str>,
    detach: bool,
    max_retries: usize,
    timeout_secs: u64,
    advisory_review: bool,
    review_blocking: bool,
    auto_rollback: bool,
    rollback_policy: superstate_core::daemon::RollbackPolicy,
    shunt_driver_output: bool,
    guard_file_reads: bool,
    read_threshold_lines: Option<usize>,
    read_cache: bool,
    guard_commands: bool,
    impact_slicing: bool,
) -> Result<(), String> {
    // Precedence: explicit flag, then environment (so a sweep can vary it per run without a
    // rebuild), then the measured default. A malformed env value is rejected rather than
    // silently ignored -- a swept parameter that quietly reverts to its default produces a
    // run that looks like a data point and is not one.
    let read_threshold_lines = match read_threshold_lines {
        Some(n) => n,
        None => match std::env::var("SUPERSTATE_READ_THRESHOLD_LINES") {
            Ok(val) => val.trim().parse::<usize>().map_err(|_| {
                format!(
                    "SUPERSTATE_READ_THRESHOLD_LINES must be a non-negative integer, got '{}'",
                    val.trim()
                )
            })?,
            Err(_) => superstate_core::guard::DEFAULT_READ_THRESHOLD_LINES,
        },
    };

    let auto_rollback = if !auto_rollback {
        false
    } else if let Ok(val) = std::env::var("SUPERSTATE_DAEMON_AUTO_ROLLBACK") {
        let s = val.trim();
        s != "0" && !s.eq_ignore_ascii_case("false")
    } else if let Ok(val) = std::env::var("SUPERSTATE_AUTO_ROLLBACK") {
        let s = val.trim();
        s != "0" && !s.eq_ignore_ascii_case("false")
    } else {
        true
    };

    let paths = daemon_paths_for_state(state_path);

    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }

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
        if advisory_review {
            cmd.arg("--advisory-review");
        }
        if review_blocking {
            cmd.arg("--review-blocking");
        }
        if rollback_policy != superstate_core::daemon::RollbackPolicy::default() {
            cmd.arg("--rollback-policy").arg(rollback_policy.as_str());
        }
        if !auto_rollback {
            cmd.arg("--no-auto-rollback");
        }
        if !shunt_driver_output {
            cmd.arg("--no-shunt");
        }
        if !guard_file_reads && !guard_commands {
            cmd.arg("--no-guard");
        } else {
            if !guard_file_reads {
                cmd.arg("--no-file-guard");
            }
            if !guard_commands {
                cmd.arg("--no-command-guard");
            }
        }
        if impact_slicing {
            cmd.arg("--impact-slicing");
        }
        // Forwarded ALWAYS, not only when non-default.
        //
        // This block re-builds the child's argv by hand, so any flag not written here is
        // silently dropped the moment the daemon detaches -- which is how the eval always
        // runs it. Both of these were added and both were lost: EXP-081's ablation arm
        // provisioned `--read-cache true` on BOTH sides and would have measured nothing, and
        // a `--read-threshold-lines` sweep would have quietly run at the default throughout.
        // A swept parameter that reverts without saying so produces a run that looks like a
        // data point and is not one.
        //
        // See `test_detach_argv_round_trips`, which parses this argv back and compares the
        // resulting config field by field, so the next addition cannot be forgotten silently.
        cmd.arg("--read-threshold-lines")
            .arg(read_threshold_lines.to_string());
        if !read_cache {
            cmd.arg("--no-read-cache");
        }
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

        // Write fallback daemon.pid only if this is the default state machine daemon.
        // Branch-scoped daemons must never overwrite or write legacy daemon.pid.
        if paths.slug == "default" {
            if let Some(parent) = paths.pid_file.parent().and_then(|p| p.parent()) {
                let _ = fs::write(parent.join("daemon.pid"), pid.to_string());
            }
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
            // Experimental: advance on a still-failing verification command rather than
            // blocking, mirroring superstate_mcp's turn-bounded loop. Opt-in, because it
            // trades away the guarantee that completion means verified.
            advisory_gate: std::env::var("SUPERSTATE_GATE_MODE")
                .map(|v| v.trim().eq_ignore_ascii_case("advisory"))
                .unwrap_or(false),
            advisory_review,
            review_blocking,
            auto_rollback,
            rollback_policy,
            shunt_driver_output,
            guard_file_reads,
            read_threshold_lines,
            read_cache,
            guard_commands,
            impact_slicing,
        };

        let mut runner = DaemonRunner::new(config)
            .map_err(|e| e.to_string())?
            .with_step_advancer(Box::new(|p| {
                crate::commands::state::cmd_next(p, None, false, None)
            }));
        let res = runner.run_to_completion();

        let _ = fs::remove_file(&paths.pid_file);
        // Clean up fallback daemon.pid if this was the default daemon
        if paths.slug == "default" {
            if let Some(parent) = paths.pid_file.parent().and_then(|p| p.parent()) {
                let _ = fs::remove_file(parent.join("daemon.pid"));
            }
        }

        match res {
            Ok(report) => {
                // WHY: Provide clear operator feedback on daemon milestone completion and artifact location.
                // WHAT: Print milestone tally, completion status, and the filesystem path of the completion report.
                // HOW: Query report fields and display via stdout.
                println!(
                    "Daemon completed for '{}': {} milestones completed (status: {:?})",
                    paths.slug, report.milestones_completed, report.status
                );
                println!("Completion artifact: {}", report.completion_file.display());
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
        let mut killed_pids = std::collections::HashSet::new();

        // Scan and terminate active daemons registered in daemons/
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
                                // Deduplicate PIDs so a process tracked by multiple PID files is only killed and counted once
                                if killed_pids.insert(pid) {
                                    let _ = kill_pid(pid);
                                    println!("Terminated daemon '{}' (PID: {})", slug, pid);
                                    cancelled_count += 1;
                                }
                                let _ = fs::remove_file(&path);
                            }
                        }
                    }
                }
            }
        }

        // Clean up legacy daemon.pid fallback if present
        let legacy_pid = state_dir.join("daemon.pid");
        if legacy_pid.exists() {
            if let Ok(pid_str) = fs::read_to_string(&legacy_pid) {
                if let Ok(pid) = pid_str.trim().parse::<i32>() {
                    // Deduplicate against PIDs already terminated from daemons/
                    if killed_pids.insert(pid) {
                        let _ = kill_pid(pid);
                        println!("Terminated legacy daemon (PID: {})", pid);
                        cancelled_count += 1;
                    }
                    let _ = fs::remove_file(&legacy_pid);
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
        let mut pid_file = state_dir.join("daemons").join(format!("{}.pid", slug));

        // If branch is "main" or "master" and branch pid file doesn't exist, fallback to default.pid
        if !pid_file.exists() && (slug == "main" || slug == "master") {
            let default_pid = state_dir.join("daemons").join("default.pid");
            if default_pid.exists() {
                pid_file = default_pid;
            }
        }

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
        let _ = kill_pid(pid);
        let _ = fs::remove_file(&pid_file);

        // If cancelling default daemon via main/master branch, also clean up legacy daemon.pid
        if slug == "main" || slug == "master" {
            let _ = fs::remove_file(state_dir.join("daemon.pid"));
        }

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
        let _ = kill_pid(pid);
        let _ = fs::remove_file(&paths.pid_file);

        // Remove legacy fallback daemon.pid if this was the default daemon
        if paths.slug == "default" {
            let _ = fs::remove_file(state_dir.join("daemon.pid"));
        }

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
        let _ = kill_pid(pid);
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
