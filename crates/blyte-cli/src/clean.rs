//! Worktree cleanup for finished runs: `blyte clean`, on review, and at dispatch.

use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum RemovalReason {
    Verdict(String),
    NoChanges,
    ResearchReport,
    OlderThan(u64),
}

impl std::fmt::Display for RemovalReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verdict(v) => {
                if v.is_empty() {
                    write!(f, "verdict")
                } else {
                    write!(f, "verdict {v}")
                }
            }
            Self::NoChanges => write!(f, "no changes"),
            Self::ResearchReport => write!(f, "research report"),
            Self::OlderThan(d) => {
                if *d == 1 {
                    write!(f, "older than 1 day")
                } else {
                    write!(f, "older than {d} days")
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum KeptReason {
    UncommittedChanges,
    Running,
    NoVerdict,
    NoReport,
    Custom(String),
}

impl std::fmt::Display for KeptReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UncommittedChanges => write!(f, "uncommitted changes"),
            Self::Running => write!(f, "running"),
            Self::NoVerdict => write!(f, "no verdict"),
            Self::NoReport => write!(f, "no report"),
            Self::Custom(s) => write!(f, "{s}"),
        }
    }
}

/// Recursively computes directory size in bytes without following symlinks.
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.file_type().is_symlink() {
        return 0;
    }
    if meta.is_file() {
        return meta.len();
    }
    if meta.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            } else if ft.is_dir() {
                total += dir_size(&entry.path());
            } else if ft.is_file() {
                if let Ok(m) = entry.metadata() {
                    total += m.len();
                }
            }
        }
    }
    total
}

/// Formats byte count into human-readable representation: e.g. "812 MB", "12.3 GB".
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    let (val, unit) = if b >= GB {
        (b / GB, "GB")
    } else if b >= MB {
        (b / MB, "MB")
    } else if b >= KB {
        (b / KB, "KB")
    } else {
        return format!("{bytes} B");
    };

    let s = if val >= 100.0 {
        format!("{val:.0}")
    } else {
        let formatted = format!("{val:.1}");
        formatted
            .strip_suffix(".0")
            .unwrap_or(&formatted)
            .to_string()
    };
    format!("{s} {unit}")
}

/// Checks whether a process with the given PID is alive.
pub fn is_pid_alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if ret == 0 {
        true
    } else {
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

/// Finds the root repository (resolving main checkout if in a worktree).
pub fn find_repo_root() -> Result<PathBuf, String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("git not found: {e}"))?;
    if !out.status.success() {
        return Err("not a git repository".to_string());
    }
    let root = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    if let Some(main) = crate::config::main_checkout(&root) {
        Ok(main)
    } else {
        Ok(root)
    }
}

/// Checks whether a run is a research run from its start file.
pub fn is_research(root: &Path, id: &str) -> bool {
    let start_path = root.join(".wt").join(format!("{id}.start.json"));
    if let Ok(s) = std::fs::read_to_string(&start_path) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&s) {
            return val
                .get("research")
                .and_then(|r| r.as_bool())
                .unwrap_or(false);
        }
    }
    false
}

/// Checks whether a worktree has uncommitted changes via `git status --porcelain`.
pub fn has_uncommitted_changes(wt_path: &Path) -> bool {
    let out = std::process::Command::new("git")
        .args([
            "-C",
            wt_path.to_str().unwrap_or(""),
            "status",
            "--porcelain",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            !o.stdout.is_empty() && !String::from_utf8_lossy(&o.stdout).trim().is_empty()
        }
        _ => true,
    }
}

/// Evaluates whether a worktree is removable according to the clean rules.
pub fn check_removable(
    root: &Path,
    id: &str,
    older_than_days: u64,
) -> Result<RemovalReason, KeptReason> {
    let wt_dir = root.join(".wt");
    let start_path = wt_dir.join(format!("{id}.start.json"));
    let summary_path = wt_dir.join(format!("{id}.json"));
    let verdict_path = wt_dir.join(format!("{id}.verdict.json"));
    let report_path = wt_dir.join(format!("{id}.report.md"));

    let start_json: Option<serde_json::Value> = std::fs::read_to_string(&start_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    let summary_exists = summary_path.is_file();

    // Check if run is finished: no live process (pid not running) or summary exists.
    if !summary_exists {
        if let Some(ref start) = start_json {
            if let Some(pid) = start.get("pid").and_then(|p| p.as_i64()) {
                if is_pid_alive(pid) {
                    return Err(KeptReason::Running);
                }
            }
        }
    }

    // Now check the removal conditions:
    // 1. A verdict is recorded (.wt/<id>.verdict.json exists)
    if verdict_path.is_file() {
        let verdict_str = std::fs::read_to_string(&verdict_path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("verdict")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();
        return Ok(RemovalReason::Verdict(verdict_str));
    }

    // 2. The outcome in .wt/<id>.json is no_changes
    if summary_exists {
        if let Ok(summary_str) = std::fs::read_to_string(&summary_path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&summary_str) {
                if val.get("outcome").and_then(|o| o.as_str()) == Some("no_changes") {
                    return Ok(RemovalReason::NoChanges);
                }
            }
        }
    }

    // 3. Research run and its report was copied out (.wt/<id>.report.md exists)
    let is_res = start_json
        .as_ref()
        .and_then(|s| s.get("research").and_then(|r| r.as_bool()))
        .unwrap_or(false);

    if is_res && report_path.is_file() {
        return Ok(RemovalReason::ResearchReport);
    }

    // 4. Finished more than older_than days ago, by the summary file's mtime
    if summary_exists {
        if let Ok(meta) = std::fs::metadata(&summary_path) {
            if let Ok(mtime) = meta.modified() {
                let age = std::time::SystemTime::now()
                    .duration_since(mtime)
                    .unwrap_or(std::time::Duration::ZERO);
                if age >= std::time::Duration::from_secs(older_than_days * 86400) {
                    return Ok(RemovalReason::OlderThan(older_than_days));
                }
            }
        }
    }

    if is_res {
        Err(KeptReason::NoReport)
    } else {
        Err(KeptReason::NoVerdict)
    }
}

/// Identifies the default branch (origin/HEAD's target, else main, else master).
pub fn find_default_branch(root: &Path) -> String {
    let sym_out = std::process::Command::new("git")
        .args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .current_dir(root)
        .output();
    if let Ok(o) = sym_out {
        if o.status.success() {
            let target = String::from_utf8_lossy(&o.stdout);
            let target = target.trim();
            let branch_name = target.strip_prefix("origin/").unwrap_or(target);
            let local_check = std::process::Command::new("git")
                .args([
                    "rev-parse",
                    "--verify",
                    &format!("refs/heads/{branch_name}"),
                ])
                .current_dir(root)
                .output();
            if local_check.map(|c| c.status.success()).unwrap_or(false) {
                return branch_name.to_string();
            }
            let remote_check = std::process::Command::new("git")
                .args(["rev-parse", "--verify", target])
                .current_dir(root)
                .output();
            if remote_check.map(|c| c.status.success()).unwrap_or(false) {
                return target.to_string();
            }
            return branch_name.to_string();
        }
    }

    let main_check = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/main"])
        .current_dir(root)
        .output();
    if main_check.map(|c| c.status.success()).unwrap_or(false) {
        return "main".to_string();
    }

    let master_check = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/master"])
        .current_dir(root)
        .output();
    if master_check.map(|c| c.status.success()).unwrap_or(false) {
        return "master".to_string();
    }

    "main".to_string()
}

/// Checks whether a branch's work is on the default branch:
/// every commit `git cherry <default> wip/<id>` lists is marked `-`
/// or `git branch --contains <tip>` lists a branch not starting with `wip/`.
pub fn is_branch_merged(root: &Path, branch_name: &str) -> bool {
    let check = std::process::Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch_name}"),
        ])
        .current_dir(root)
        .output();
    if !check.map(|c| c.status.success()).unwrap_or(false) {
        return false;
    }

    let tip_out = std::process::Command::new("git")
        .args(["rev-parse", &format!("refs/heads/{branch_name}")])
        .current_dir(root)
        .output();
    let tip_sha = match tip_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return false,
    };

    let default_branch = find_default_branch(root);

    // Condition A: every commit `git cherry <default> wip/<id>` lists is marked `-`
    let cherry_out = std::process::Command::new("git")
        .args(["cherry", &default_branch, branch_name])
        .current_dir(root)
        .output();
    if let Ok(o) = cherry_out {
        if o.status.success() {
            let s = String::from_utf8_lossy(&o.stdout);
            let lines: Vec<&str> = s
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect();
            if !lines.is_empty() && lines.iter().all(|l| l.starts_with('-')) {
                return true;
            }
        }
    }

    // Condition B: `git branch --contains <tip>` lists a branch not starting with `wip/`
    let contains_out = std::process::Command::new("git")
        .args(["branch", "--contains", &tip_sha])
        .current_dir(root)
        .output();
    if let Ok(o) = contains_out {
        if o.status.success() {
            let s = String::from_utf8_lossy(&o.stdout);
            for line in s.lines() {
                let trimmed = line
                    .trim()
                    .trim_start_matches('*')
                    .trim_start_matches('+')
                    .trim();
                if !trimmed.is_empty()
                    && !trimmed.starts_with("wip/")
                    && !trimmed.starts_with("(HEAD detached")
                {
                    return true;
                }
            }
        }
    }

    false
}

/// Lists all `.wt/<id>` directories that have a corresponding `.wt/<id>.start.json`.
pub fn find_blyte_worktrees(root: &Path) -> Vec<String> {
    let wt_dir = root.join(".wt");
    if !wt_dir.is_dir() {
        return Vec::new();
    }
    let mut ids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&wt_dir) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let start_file = wt_dir.join(format!("{name}.start.json"));
                    if start_file.is_file() {
                        ids.push(name);
                    }
                }
            }
        }
    }
    ids.sort();
    ids
}

pub struct CleanRepoStats {
    pub removed_worktrees: usize,
    pub freed_bytes: u64,
}

/// Cleans one repository.
pub fn clean_repo(
    root: &Path,
    older_than_days: u64,
    dry_run: bool,
    skip_id: Option<&str>,
    quiet: bool,
) -> CleanRepoStats {
    let mut removed_worktrees = 0;
    let mut freed_bytes = 0;

    let ids = find_blyte_worktrees(root);
    for id in ids {
        if skip_id == Some(&id) {
            continue;
        }

        let wt_path = root.join(".wt").join(&id);
        match check_removable(root, &id, older_than_days) {
            Err(kept_reason) => {
                if !quiet {
                    println!("kept .wt/{id} ({kept_reason})");
                }
            }
            Ok(removal_reason) => {
                let is_res = is_research(root, &id);
                if !is_res && has_uncommitted_changes(&wt_path) {
                    if !quiet {
                        println!("kept .wt/{id} (uncommitted changes)");
                    }
                    continue;
                }

                let size = dir_size(&wt_path);
                let branch_name = format!("wip/{id}");
                let is_merged = is_branch_merged(root, &branch_name);

                if dry_run {
                    if !quiet {
                        println!(
                            "would remove .wt/{id} ({removal_reason}, {})",
                            format_size(size)
                        );
                        if is_merged {
                            println!("would delete branch {branch_name} (merged)");
                        }
                    }
                    freed_bytes += size;
                    removed_worktrees += 1;
                } else {
                    let removed_ok = if is_res {
                        std::fs::remove_dir_all(&wt_path).is_ok()
                    } else {
                        let rm_out = std::process::Command::new("git")
                            .args(["worktree", "remove", wt_path.to_str().unwrap_or("")])
                            .current_dir(root)
                            .output();
                        let _ = std::process::Command::new("git")
                            .args(["worktree", "prune"])
                            .current_dir(root)
                            .output();
                        rm_out.map(|o| o.status.success()).unwrap_or(false)
                    };

                    if removed_ok {
                        freed_bytes += size;
                        removed_worktrees += 1;
                        if !quiet {
                            println!("removed .wt/{id} ({removal_reason}, {})", format_size(size));
                        }
                        if is_merged {
                            let del_out = std::process::Command::new("git")
                                .args(["branch", "-D", &branch_name])
                                .current_dir(root)
                                .output();
                            if del_out.map(|o| o.status.success()).unwrap_or(false) && !quiet {
                                println!("deleted branch {branch_name} (merged)");
                            }
                        }
                    } else if !quiet {
                        println!("kept .wt/{id} (worktree remove failed)");
                    }
                }
            }
        }
    }

    CleanRepoStats {
        removed_worktrees,
        freed_bytes,
    }
}

/// Runs the `blyte clean` command.
pub fn run(args: &crate::cli::CleanArgs) -> i32 {
    let repos = if args.all {
        crate::registry::registered()
    } else {
        match find_repo_root() {
            Ok(root) => vec![root],
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    };

    let mut total_freed = 0;
    for repo in &repos {
        let stats = clean_repo(repo, args.older_than, args.dry_run, None, false);
        total_freed += stats.freed_bytes;
    }

    if args.dry_run {
        println!("would free {}", format_size(total_freed));
    } else {
        println!("freed {}", format_size(total_freed));
    }

    0
}

/// Quiet cleanup at dispatch start: no output unless something was removed,
/// then one line `blyte: removed N finished worktrees (X GB)` on stderr.
/// Never fails the dispatch: logs errors to stderr and goes on.
pub fn clean_quiet(root: &Path, skip_id: Option<&str>) {
    let stats = clean_repo(root, 7, false, skip_id, true);
    if stats.removed_worktrees > 0 {
        eprintln!(
            "blyte: removed {} finished worktrees ({})",
            stats.removed_worktrees,
            format_size(stats.freed_bytes)
        );
    }
}

/// Removes a run's worktree on review after recording the verdict under the same rules.
/// Prints `removed .wt/<id>; branch wip/<id> kept` (or the kept reason).
/// Deletes the branch only if it is merged.
pub fn clean_on_review(root: &Path, id: &str) {
    let wt_path = root.join(".wt").join(id);
    if !wt_path.exists() {
        return;
    }

    let branch_name = format!("wip/{id}");
    let branch_exists = std::process::Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch_name}"),
        ])
        .current_dir(root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if let Err(kept_reason) = check_removable(root, id, 7) {
        if branch_exists {
            println!("kept .wt/{id} ({kept_reason}); branch {branch_name} kept");
        } else {
            println!("kept .wt/{id} ({kept_reason})");
        }
        return;
    }

    let is_res = is_research(root, id);
    if !is_res && has_uncommitted_changes(&wt_path) {
        if branch_exists {
            println!("kept .wt/{id} (uncommitted changes); branch {branch_name} kept");
        } else {
            println!("kept .wt/{id} (uncommitted changes)");
        }
        return;
    }

    let removed_ok = if is_res {
        std::fs::remove_dir_all(&wt_path).is_ok()
    } else {
        let rm_out = std::process::Command::new("git")
            .args(["worktree", "remove", wt_path.to_str().unwrap_or("")])
            .current_dir(root)
            .output();
        let _ = std::process::Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(root)
            .output();
        rm_out.map(|o| o.status.success()).unwrap_or(false)
    };

    if !removed_ok {
        if branch_exists {
            println!("kept .wt/{id} (worktree remove failed); branch {branch_name} kept");
        } else {
            println!("kept .wt/{id} (worktree remove failed)");
        }
        return;
    }

    if branch_exists {
        if is_branch_merged(root, &branch_name) {
            let del_out = std::process::Command::new("git")
                .args(["branch", "-D", &branch_name])
                .current_dir(root)
                .output();
            if del_out.map(|o| o.status.success()).unwrap_or(false) {
                println!("removed .wt/{id}; branch {branch_name} deleted");
            } else {
                println!("removed .wt/{id}; branch {branch_name} kept");
            }
        } else {
            println!("removed .wt/{id}; branch {branch_name} kept");
        }
    } else {
        println!("removed .wt/{id}");
    }
}
