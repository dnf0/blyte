//! Worktree cleanup for finished runs: `blyte clean`, on review, and at dispatch.

use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum RemovalReason {
    Merged,
    Verdict(String),
    NoChanges,
    ResearchReport,
    OlderThan(u64),
}

impl std::fmt::Display for RemovalReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Merged => write!(f, "merged"),
            Self::Verdict(v) => {
                if v.is_empty() {
                    write!(f, "verdict")
                } else {
                    write!(f, "verdict {v}")
                }
            }
            Self::NoChanges => write!(f, "no changes"),
            Self::ResearchReport => write!(f, "research report"),
            Self::OlderThan(d) => write!(f, "finished over {d} days ago"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum KeptReason {
    UncommittedChanges,
    Running,
    NoVerdict,
    NotMergedYet,
    NoReport,
    Custom(String),
}

impl std::fmt::Display for KeptReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UncommittedChanges => write!(f, "uncommitted changes"),
            Self::Running => write!(f, "running"),
            Self::NoVerdict => write!(f, "no verdict"),
            Self::NotMergedYet => write!(f, "not merged yet"),
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

    let is_res = start_json
        .as_ref()
        .and_then(|s| s.get("research").and_then(|r| r.as_bool()))
        .unwrap_or(false);

    // Rule 2: Discarded (the verdict is redone, or the outcome is no_changes)
    let verdict_str = if verdict_path.is_file() {
        std::fs::read_to_string(&verdict_path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("verdict")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default()
    } else {
        String::new()
    };

    if verdict_str == "redone" {
        return Ok(RemovalReason::Verdict("redone".to_string()));
    }

    if summary_exists {
        if let Ok(summary_str) = std::fs::read_to_string(&summary_path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&summary_str) {
                if val.get("outcome").and_then(|o| o.as_str()) == Some("no_changes") {
                    return Ok(RemovalReason::NoChanges);
                }
            }
        }
    }

    // Rule 3: Research run and its report was copied out (.wt/<id>.report.md exists)
    if is_res && report_path.is_file() {
        return Ok(RemovalReason::ResearchReport);
    }

    // Rule 1: Merged (its work is on the default branch)
    let branch_name = format!("wip/{id}");
    if !is_res && is_branch_merged(root, &branch_name) {
        return Ok(RemovalReason::Merged);
    }

    // Rule 4: Finished more than older_than days ago, by the summary file's mtime
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

    // A verdict of accepted or fixed on its own no longer makes a worktree removable
    if !verdict_str.is_empty() {
        return Err(KeptReason::NotMergedYet);
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

/// Checks whether a branch's work is on the default branch when any of:
/// - `git merge-base --is-ancestor <tip> <default>` succeeds;
/// - every commit `git cherry <default> <branch>` lists is marked `-`;
/// - merging it would change nothing: `git merge-tree --write-tree <default> <branch>` exits 0
///   and the tree id it prints (first line) equals `git rev-parse <default>^{tree}`.
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

    is_commit_merged(root, &tip_sha)
}

/// Checks whether a commit's work is on the default branch when any of:
/// - `git merge-base --is-ancestor <commit> <default>` succeeds;
/// - every commit `git cherry <default> <commit>` lists is marked `-`;
/// - merging it would change nothing: `git merge-tree --write-tree <default> <commit>` exits 0
///   and the tree id it prints (first line) equals `git rev-parse <default>^{tree}`.
pub fn is_commit_merged(root: &Path, commit_sha: &str) -> bool {
    let default_branch = find_default_branch(root);
    // The local default branch can be far behind the remote one (1,292 commits in one repository,
    // 2026-10-01), so the remote-tracking branch, as of the last fetch, counts too.
    let name = default_branch
        .strip_prefix("origin/")
        .unwrap_or(&default_branch);
    let remote = format!("origin/{name}");
    let mut targets = vec![default_branch.clone()];
    if remote != default_branch && git_ok(root, &["rev-parse", "--verify", "--quiet", &remote]) {
        targets.push(remote);
    }
    targets
        .iter()
        .any(|target| merged_into(root, commit_sha, target))
}

fn git_ok(root: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Whether the commit's work is already on `default_branch`.
fn merged_into(root: &Path, commit_sha: &str, default_branch: &str) -> bool {
    // 1. `git merge-base --is-ancestor <tip> <default>` succeeds
    let ancestor_check = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", commit_sha, default_branch])
        .current_dir(root)
        .output();
    if ancestor_check.map(|c| c.status.success()).unwrap_or(false) {
        return true;
    }

    // 2. every commit `git cherry <default> <commit>` lists is marked `-`
    let cherry_out = std::process::Command::new("git")
        .args(["cherry", default_branch, commit_sha])
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

    // 3. merging it would change nothing: `git merge-tree --write-tree <default> <commit>` exits 0
    // and the tree id it prints equals `git rev-parse <default>^{tree}`
    let default_tree_out = std::process::Command::new("git")
        .args(["rev-parse", &format!("{default_branch}^{{tree}}")])
        .current_dir(root)
        .output();
    if let Ok(tree_o) = default_tree_out {
        if tree_o.status.success() {
            let default_tree = String::from_utf8_lossy(&tree_o.stdout).trim().to_string();
            let merge_tree_out = std::process::Command::new("git")
                .args(["merge-tree", "--write-tree", default_branch, commit_sha])
                .current_dir(root)
                .output();
            if let Ok(mt_o) = merge_tree_out {
                if mt_o.status.success() {
                    let mt_stdout = String::from_utf8_lossy(&mt_o.stdout);
                    if let Some(first_line) = mt_stdout.lines().next() {
                        if first_line.trim() == default_tree {
                            return true;
                        }
                    }
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

    if !quiet {
        report_other_worktrees(root);
    }

    CleanRepoStats {
        removed_worktrees,
        freed_bytes,
    }
}

#[derive(Debug)]
pub struct GitWorktreeInfo {
    pub path: PathBuf,
    pub head_sha: Option<String>,
    pub branch: Option<String>,
    pub is_detached: bool,
    pub is_bare: bool,
}

/// Parses git worktrees from `git worktree list --porcelain`.
pub fn parse_worktree_list(root: &Path) -> Vec<GitWorktreeInfo> {
    let out = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(root)
        .output();
    let Ok(o) = out else {
        return Vec::new();
    };
    if !o.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&o.stdout);

    let mut list = Vec::new();
    let mut cur_path: Option<PathBuf> = None;
    let mut cur_head: Option<String> = None;
    let mut cur_branch: Option<String> = None;
    let mut cur_detached = false;
    let mut cur_bare = false;

    let mut finish_entry = |path: &mut Option<PathBuf>,
                            head: &mut Option<String>,
                            branch: &mut Option<String>,
                            detached: &mut bool,
                            bare: &mut bool| {
        if let Some(p) = path.take() {
            list.push(GitWorktreeInfo {
                path: p,
                head_sha: head.take(),
                branch: branch.take(),
                is_detached: *detached,
                is_bare: *bare,
            });
            *detached = false;
            *bare = false;
        }
    };

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            finish_entry(
                &mut cur_path,
                &mut cur_head,
                &mut cur_branch,
                &mut cur_detached,
                &mut cur_bare,
            );
        } else if let Some(p) = line.strip_prefix("worktree ") {
            finish_entry(
                &mut cur_path,
                &mut cur_head,
                &mut cur_branch,
                &mut cur_detached,
                &mut cur_bare,
            );
            cur_path = Some(PathBuf::from(p.trim()));
        } else if let Some(h) = line.strip_prefix("HEAD ") {
            cur_head = Some(h.trim().to_string());
        } else if let Some(b) = line.strip_prefix("branch ") {
            let b_str = b.trim();
            let b_name = b_str.strip_prefix("refs/heads/").unwrap_or(b_str);
            cur_branch = Some(b_name.to_string());
        } else if line == "detached" {
            cur_detached = true;
        } else if line == "bare" {
            cur_bare = true;
        }
    }
    finish_entry(
        &mut cur_path,
        &mut cur_head,
        &mut cur_branch,
        &mut cur_detached,
        &mut cur_bare,
    );
    list
}

/// Computes the path of `target` relative to `base`.
pub fn relative_path(target: &Path, base: &Path) -> PathBuf {
    let target_canon = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let base_canon = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());

    if let Ok(rel) = target_canon.strip_prefix(&base_canon) {
        return rel.to_path_buf();
    }

    let target_comps: Vec<_> = target_canon.components().collect();
    let base_comps: Vec<_> = base_canon.components().collect();

    let mut common = 0;
    while common < target_comps.len()
        && common < base_comps.len()
        && target_comps[common] == base_comps[common]
    {
        common += 1;
    }

    if common == 0 {
        return target_canon;
    }

    let mut rel = PathBuf::new();
    for _ in common..base_comps.len() {
        rel.push("..");
    }
    for comp in &target_comps[common..] {
        rel.push(comp);
    }
    rel
}

/// Checks whether a worktree's working tree is clean:
/// `git status --porcelain` lists nothing other than `.DS_Store` files, at any depth.
pub fn is_worktree_clean(wt_path: &Path) -> bool {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(wt_path.as_os_str())
        .args(["status", "--porcelain", "-u"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            for line in stdout.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if line.len() < 3 {
                    return false;
                }
                let mut path_part = line[3..].trim();
                if let Some((_, new_path)) = path_part.split_once(" -> ") {
                    path_part = new_path.trim();
                }
                let unquoted = path_part
                    .strip_prefix('"')
                    .and_then(|p| p.strip_suffix('"'))
                    .unwrap_or(path_part);
                if Path::new(unquoted).file_name() != Some(std::ffi::OsStr::new(".DS_Store")) {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

/// Inspects other worktrees (excluding main checkout and blyte's `.wt/<id>` runs),
/// and reports any that are both merged and clean. Never deletes them.
pub fn report_other_worktrees(root: &Path) {
    let worktrees = parse_worktree_list(root);
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let wt_dir = root_canon.join(".wt");

    let mut other_count = 0;
    let mut other_bytes = 0;

    for wt in worktrees {
        if wt.is_bare {
            continue;
        }
        if !wt.path.exists() {
            continue;
        }

        let wt_canon = wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone());
        // Skip the main checkout
        if wt_canon == root_canon || wt.path == root {
            continue;
        }
        // Skip blyte's .wt/<id> worktrees
        if wt_canon.starts_with(&wt_dir) || wt.path.starts_with(root.join(".wt")) {
            continue;
        }

        // Check if merged
        let is_merged = if let Some(ref branch) = wt.branch {
            is_branch_merged(root, branch)
        } else if let Some(ref sha) = wt.head_sha {
            is_commit_merged(root, sha)
        } else {
            false
        };

        if !is_merged {
            continue;
        }

        // Check if clean (lists nothing other than .DS_Store files)
        if !is_worktree_clean(&wt.path) {
            continue;
        }

        let size = dir_size(&wt.path);
        let rel_path = relative_path(&wt.path, root);
        let rel_str = rel_path.to_string_lossy().replace('\\', "/");
        let branch_label = wt.branch.as_deref().unwrap_or_else(|| {
            wt.head_sha
                .as_deref()
                .map(|s| &s[..s.len().min(7)])
                .unwrap_or("detached")
        });

        println!(
            "merged worktree {rel_str} ({branch_label}, {}): git worktree remove {rel_str}",
            format_size(size)
        );

        other_count += 1;
        other_bytes += size;
    }

    if other_count > 0 {
        println!(
            "{other_count} other worktrees are merged and clean ({}); remove them with the commands above",
            format_size(other_bytes)
        );
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
    let stats = clean_repo(root, 14, false, skip_id, true);
    if stats.removed_worktrees > 0 {
        eprintln!(
            "blyte: removed {} finished worktrees ({})",
            stats.removed_worktrees,
            format_size(stats.freed_bytes)
        );
    }
}

/// Removes a run's worktree on review after recording the verdict under the same rules.
/// Removes the worktree only when the rules allow it now (e.g. redone, or work already merged);
/// otherwise prints nothing extra beyond the verdict line, except for accepted/fixed prints
/// `worktree .wt/<id> kept until wip/<id> is merged`.
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

    let verdict_path = root.join(".wt").join(format!("{id}.verdict.json"));
    let verdict_str = std::fs::read_to_string(&verdict_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("verdict")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();

    let is_res = is_research(root, id);
    let removable = match check_removable(root, id, 14) {
        Ok(_) => is_res || !has_uncommitted_changes(&wt_path),
        Err(_) => false,
    };

    if !removable {
        if verdict_str == "accepted" || verdict_str == "fixed" {
            println!("worktree .wt/{id} kept until {branch_name} is merged");
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
        if verdict_str == "accepted" || verdict_str == "fixed" {
            println!("worktree .wt/{id} kept until {branch_name} is merged");
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
