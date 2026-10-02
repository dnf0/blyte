//! Worktree cleanup for finished runs: `blyte clean`, on review, and at dispatch.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RemovalReason {
    Merged,
    MergedInto(String),
    MergedThrough(String),
    Verdict(String),
    NoChanges,
    ResearchReport,
    OlderThan(u64),
}

impl std::fmt::Display for RemovalReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Merged => write!(f, "merged"),
            Self::MergedInto(base) => write!(f, "merged into {base}"),
            Self::MergedThrough(child) => write!(f, "merged through {child}"),
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

/// Finds the root repository (resolving the owning repository if inside a `.wt` run).
pub fn find_repo_root() -> Result<PathBuf, String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("git not found: {e}"))?;
    if !out.status.success() {
        return Err("not a git repository".to_string());
    }
    let root = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    if has_wt_component(&root) {
        if let Some(wt_parent) = root.parent().and_then(|p| p.parent()) {
            if root
                .parent()
                .and_then(|p| p.file_name())
                .is_some_and(|n| n == ".wt")
            {
                return Ok(wt_parent.to_path_buf());
            }
        }
        if let Some(main) = crate::config::main_checkout(&root) {
            return Ok(main);
        }
    }
    Ok(root)
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

fn git_tree_sha(root: &Path, ref_name: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", &format!("{ref_name}^{{tree}}")])
        .current_dir(root)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn is_commit_merged_into_target(
    root: &Path,
    commit_sha: &str,
    target: &str,
    target_tree: Option<&str>,
) -> bool {
    let ancestor_check = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", commit_sha, target])
        .current_dir(root)
        .output();
    if ancestor_check.map(|c| c.status.success()).unwrap_or(false) {
        return true;
    }

    let cherry_out = std::process::Command::new("git")
        .args(["cherry", target, commit_sha])
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

    let target_tree_sha = match target_tree {
        Some(t) => Some(t.to_string()),
        None => git_tree_sha(root, target),
    };
    if let Some(target_tree_sha) = target_tree_sha {
        let merge_tree_out = std::process::Command::new("git")
            .args(["merge-tree", "--write-tree", target, commit_sha])
            .current_dir(root)
            .output();
        if let Ok(mt_o) = merge_tree_out {
            if mt_o.status.success() {
                let mt_stdout = String::from_utf8_lossy(&mt_o.stdout);
                if let Some(first_line) = mt_stdout.lines().next() {
                    if first_line.trim() == target_tree_sha {
                        return true;
                    }
                }
            }
        }
    }

    false
}

fn check_history_merged(
    root: &Path,
    tip_sha: &str,
    target: &str,
    started_at: Option<&serde_json::Value>,
) -> bool {
    let mb_out = std::process::Command::new("git")
        .args(["merge-base", target, tip_sha])
        .current_dir(root)
        .output();
    let Ok(mb_o) = mb_out else {
        return false;
    };
    if !mb_o.status.success() {
        return false;
    }
    let merge_base = String::from_utf8_lossy(&mb_o.stdout).trim().to_string();
    if merge_base.is_empty() {
        return false;
    }

    let diff_out = std::process::Command::new("git")
        .args(["diff", "--name-only", &merge_base, tip_sha])
        .current_dir(root)
        .output();
    let Ok(diff_o) = diff_out else {
        return false;
    };
    if !diff_o.status.success() {
        return false;
    }
    let files: Vec<String> = String::from_utf8_lossy(&diff_o.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    if files.is_empty() {
        return false;
    }

    let since_arg = started_at
        .and_then(|v| {
            if let Some(n) = v.as_i64() {
                if n >= 100_000_000 {
                    Some(format!("--since={n}"))
                } else {
                    Some("--since=1970-01-01T00:00:00Z".to_string())
                }
            } else if let Some(n) = v.as_u64() {
                if n >= 100_000_000 {
                    Some(format!("--since={n}"))
                } else {
                    Some("--since=1970-01-01T00:00:00Z".to_string())
                }
            } else {
                v.as_str().map(|s| format!("--since={s}"))
            }
        })
        .unwrap_or_else(|| "--since=1970-01-01T00:00:00Z".to_string());

    let mut cmd = std::process::Command::new("git");
    cmd.args([
        "rev-list",
        "--first-parent",
        &since_arg,
        "-n",
        "20",
        target,
        "--",
    ]);
    for f in &files {
        cmd.arg(f);
    }
    cmd.current_dir(root);
    let Ok(rev_o) = cmd.output() else {
        return false;
    };
    if !rev_o.status.success() {
        return false;
    }

    let commits: Vec<String> = String::from_utf8_lossy(&rev_o.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    for c in commits {
        let mt_out = std::process::Command::new("git")
            .args(["merge-tree", "--write-tree", &c, tip_sha])
            .current_dir(root)
            .output();
        let Ok(mt_o) = mt_out else {
            continue;
        };
        if !mt_o.status.success() {
            continue;
        }
        let mt_stdout = String::from_utf8_lossy(&mt_o.stdout);
        let Some(first_line) = mt_stdout.lines().next() else {
            continue;
        };
        let mt_tree = first_line.trim();

        if let Some(c_tree) = git_tree_sha(root, &c) {
            if mt_tree == c_tree {
                return true;
            }
        }
    }

    false
}

/// Context for cleaning a repository, caching metadata to avoid redundant git calls and disk reads.
pub struct CleanContext<'a> {
    pub root: &'a Path,
    pub start_files: HashMap<String, serde_json::Value>,
    pub all_run_ids: Vec<String>,
    pub wip_branch_tips: HashMap<String, String>,
    pub default_branch: String,
    pub origin_default: Option<String>,
    pub default_tree: Option<String>,
    pub origin_default_tree: Option<String>,
    pub wip_contains_cache: HashMap<String, Vec<String>>,
    pub merged_cache: HashMap<String, Option<RemovalReason>>,
    pub branch_contains_calls: usize,
}

impl<'a> CleanContext<'a> {
    pub fn new(root: &'a Path) -> Self {
        let mut start_files = HashMap::new();
        let mut all_run_ids = Vec::new();
        let wt_dir = root.join(".wt");
        if let Ok(entries) = std::fs::read_dir(&wt_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(id) = name.strip_suffix(".start.json") {
                    all_run_ids.push(id.to_string());
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                            start_files.insert(id.to_string(), val);
                        }
                    }
                }
            }
        }
        all_run_ids.sort();

        let mut wip_branch_tips = HashMap::new();
        let ref_out = std::process::Command::new("git")
            .args([
                "for-each-ref",
                "--format=%(refname:short) %(objectname)",
                "refs/heads/wip/",
            ])
            .current_dir(root)
            .output();
        if let Ok(o) = ref_out {
            if o.status.success() {
                for line in String::from_utf8_lossy(&o.stdout).lines() {
                    let mut parts = line.split_whitespace();
                    if let (Some(refname), Some(sha)) = (parts.next(), parts.next()) {
                        wip_branch_tips.insert(refname.to_string(), sha.to_string());
                    }
                }
            }
        }

        let default_branch = find_default_branch(root);
        let name = default_branch
            .strip_prefix("origin/")
            .unwrap_or(&default_branch);
        let remote = format!("origin/{name}");
        let origin_default = if remote != default_branch
            && git_ok(root, &["rev-parse", "--verify", "--quiet", &remote])
        {
            Some(remote)
        } else {
            None
        };

        let default_tree = git_tree_sha(root, &default_branch);
        let origin_default_tree = origin_default
            .as_deref()
            .and_then(|r| git_tree_sha(root, r));

        Self {
            root,
            start_files,
            all_run_ids,
            wip_branch_tips,
            default_branch,
            origin_default,
            default_tree,
            origin_default_tree,
            wip_contains_cache: HashMap::new(),
            merged_cache: HashMap::new(),
            branch_contains_calls: 0,
        }
    }

    pub fn get_branch_tip(&self, branch_name: &str) -> Option<String> {
        if let Some(sha) = self.wip_branch_tips.get(branch_name) {
            return Some(sha.clone());
        }
        get_branch_tip(self.root, branch_name)
    }

    pub fn is_research_run(&self, id: &str) -> bool {
        self.start_files
            .get(id)
            .and_then(|v| v.get("research"))
            .and_then(|r| r.as_bool())
            .unwrap_or_else(|| is_research(self.root, id))
    }

    pub fn get_wip_branches_containing(&mut self, tip_sha: &str) -> Vec<String> {
        if let Some(cached) = self.wip_contains_cache.get(tip_sha) {
            return cached.clone();
        }

        self.branch_contains_calls += 1;

        let out = std::process::Command::new("git")
            .args([
                "branch",
                "--list",
                "--format=%(refname:short)",
                "wip/*",
                "--contains",
                tip_sha,
            ])
            .current_dir(self.root)
            .output();

        let mut branches = Vec::new();
        if let Ok(o) = out {
            if o.status.success() {
                for line in String::from_utf8_lossy(&o.stdout).lines() {
                    let trimmed = line.trim().trim_start_matches(['*', '+', ' ']).trim();
                    if !trimmed.is_empty() {
                        branches.push(trimmed.to_string());
                    }
                }
            }
        }
        self.wip_contains_cache
            .insert(tip_sha.to_string(), branches.clone());
        branches
    }

    pub fn get_children_of(&mut self, id: &str, tip_sha: &str) -> Vec<String> {
        let mut children = Vec::new();
        let mut seen = HashSet::new();

        // 1. Edges from base fields directly
        for (candidate_id, start_val) in &self.start_files {
            if candidate_id == id {
                continue;
            }
            if let Some(base_str) = start_val.get("base").and_then(|v| v.as_str()) {
                let base_str = base_str.trim().trim_end_matches('/');
                let target1 = format!("wip/{id}");
                let target2 = format!("refs/heads/wip/{id}");
                if (base_str == target1 || base_str == target2) && seen.insert(candidate_id.clone())
                {
                    children.push(candidate_id.clone());
                }
            }
        }

        // 2. Edges for runs without base:
        if !tip_sha.is_empty() {
            let tip_sha_owned = tip_sha.to_string();
            let containing_branches = self.get_wip_branches_containing(&tip_sha_owned);
            for b in containing_branches {
                let clean_b = b.strip_prefix("refs/heads/").unwrap_or(&b);
                if let Some(candidate_id) = clean_b.strip_prefix("wip/") {
                    if candidate_id == id {
                        continue;
                    }
                    if !self.start_files.contains_key(candidate_id) {
                        continue;
                    }
                    let has_base = self
                        .start_files
                        .get(candidate_id)
                        .and_then(|val| val.get("base"))
                        .is_some_and(|b| !b.is_null());
                    if !has_base && seen.insert(candidate_id.to_string()) {
                        children.push(candidate_id.to_string());
                    }
                }
            }
        }

        children.sort();
        children
    }

    pub fn is_child_run_merged(&mut self, child: &str, visited: &mut HashSet<String>) -> bool {
        if self.is_research_run(child) {
            return false;
        }

        let branch_name = format!("wip/{child}");
        let tip = self.get_branch_tip(&branch_name);

        if tip.is_none() {
            let verdict_path = self.root.join(".wt").join(format!("{child}.verdict.json"));
            let (has_accepted_verdict, is_redone) = if verdict_path.is_file() {
                let v = std::fs::read_to_string(&verdict_path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .and_then(|v| {
                        v.get("verdict")
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                (v == "accepted" || v == "fixed", v == "redone")
            } else {
                (false, false)
            };

            if is_redone {
                return false;
            }

            let summary_path = self.root.join(".wt").join(format!("{child}.json"));
            let is_verified = std::fs::read_to_string(&summary_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| {
                    v.get("outcome")
                        .and_then(|s| s.as_str())
                        .map(|s| s == "verified")
                })
                .unwrap_or(false);

            let wt_gone = !self.root.join(".wt").join(child).is_dir();

            return has_accepted_verdict || (is_verified && wt_gone);
        }

        self.check_run_merged_internal(child, visited).is_some()
    }

    pub fn check_run_merged(&mut self, id: &str) -> Option<RemovalReason> {
        let mut visited = HashSet::new();
        self.check_run_merged_internal(id, &mut visited)
    }

    fn check_run_merged_internal(
        &mut self,
        id: &str,
        visited: &mut HashSet<String>,
    ) -> Option<RemovalReason> {
        if visited.contains(id) {
            return None;
        }

        if let Some(cached) = self.merged_cache.get(id) {
            return cached.clone();
        }

        visited.insert(id.to_string());

        let branch_name = format!("wip/{id}");
        let tip_sha = self.get_branch_tip(&branch_name);

        let result = self.decide_run_merged(id, tip_sha.as_deref(), visited);

        visited.remove(id);

        self.merged_cache.insert(id.to_string(), result.clone());
        result
    }

    fn decide_run_merged(
        &mut self,
        id: &str,
        tip_sha: Option<&str>,
        visited: &mut HashSet<String>,
    ) -> Option<RemovalReason> {
        let start_val = self.start_files.get(id);
        let base_opt = start_val.and_then(|s| s.get("base").and_then(|b| b.as_str()));
        let base_targets = base_opt.and_then(|b| resolve_base_targets(self.root, b));
        let started_at = start_val.and_then(|s| s.get("started_at"));

        if let Some(tip_sha) = tip_sha {
            // Step 1: Cheap checks on default branch & origin counterpart
            if is_commit_merged_into_target(
                self.root,
                tip_sha,
                &self.default_branch,
                self.default_tree.as_deref(),
            ) {
                return Some(RemovalReason::Merged);
            }
            if let Some(ref origin_def) = self.origin_default {
                if is_commit_merged_into_target(
                    self.root,
                    tip_sha,
                    origin_def,
                    self.origin_default_tree.as_deref(),
                ) {
                    return Some(RemovalReason::Merged);
                }
            }

            // Step 2: Cheap checks on base branch targets
            if let Some((ref display_name, ref targets)) = base_targets {
                for target in targets {
                    if is_commit_merged_into_target(self.root, tip_sha, target, None) {
                        return Some(RemovalReason::MergedInto(display_name.clone()));
                    }
                }
            }

            // Step 3: History walk on default branch & origin counterpart
            if check_history_merged(self.root, tip_sha, &self.default_branch, started_at) {
                return Some(RemovalReason::Merged);
            }
            if let Some(ref origin_def) = self.origin_default {
                if check_history_merged(self.root, tip_sha, origin_def, started_at) {
                    return Some(RemovalReason::Merged);
                }
            }

            // Step 4: History walk on base branch targets
            if let Some((ref display_name, ref targets)) = base_targets {
                for target in targets {
                    if check_history_merged(self.root, tip_sha, target, started_at) {
                        return Some(RemovalReason::MergedInto(display_name.clone()));
                    }
                }
            }
        }

        // Step 5: Follow-up search
        let children = self.get_children_of(id, tip_sha.unwrap_or(""));
        for child in children {
            if visited.contains(&child) {
                continue;
            }
            if self.is_child_run_merged(&child, visited) {
                return Some(RemovalReason::MergedThrough(child));
            }
        }

        None
    }

    pub fn find_merged_followup(&mut self, id: &str) -> Option<String> {
        let branch_name = format!("wip/{id}");
        let tip_sha = self.get_branch_tip(&branch_name);
        let mut visited = HashSet::new();
        visited.insert(id.to_string());
        let children = self.get_children_of(id, tip_sha.as_deref().unwrap_or(""));
        children
            .into_iter()
            .find(|child| self.is_child_run_merged(child, &mut visited))
    }

    pub fn check_removable(
        &mut self,
        id: &str,
        older_than_days: u64,
    ) -> Result<RemovalReason, KeptReason> {
        let wt_dir = self.root.join(".wt");
        let summary_path = wt_dir.join(format!("{id}.json"));
        let verdict_path = wt_dir.join(format!("{id}.verdict.json"));
        let report_path = wt_dir.join(format!("{id}.report.md"));

        let start_json = self.start_files.get(id);
        let summary_exists = summary_path.is_file();

        // Check if run is finished: no live process (pid not running) or summary exists.
        if !summary_exists {
            if let Some(start) = start_json {
                if let Some(pid) = start.get("pid").and_then(|p| p.as_i64()) {
                    if is_pid_alive(pid) {
                        return Err(KeptReason::Running);
                    }
                }
            }
        }

        let is_res = self.is_research_run(id);

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

        // Rule 1: Merged (its work is on the default branch or base branch or follow-up)
        if !is_res {
            if let Some(reason) = self.check_run_merged(id) {
                return Ok(reason);
            }
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
}

/// Evaluates whether a worktree is removable according to the clean rules.
pub fn check_removable(
    root: &Path,
    id: &str,
    older_than_days: u64,
) -> Result<RemovalReason, KeptReason> {
    let mut ctx = CleanContext::new(root);
    ctx.check_removable(id, older_than_days)
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

/// Determines if a string is a commit SHA (hex-only, length 7 to 64).
fn is_commit_id(s: &str) -> bool {
    (s.len() >= 7 && s.len() <= 64) && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Resolves candidate git refs for a run's base branch.
/// Returns (display_name, targets) if `base` is a branch name that resolves.
/// Returns None if `base` is not a branch name (e.g. HEAD, commit id, wip/ branch)
/// or if none of the candidate refs resolve in git.
pub fn resolve_base_targets(root: &Path, base: &str) -> Option<(String, Vec<String>)> {
    let base = base.trim();
    if base.is_empty() {
        return None;
    }

    if base == "HEAD"
        || base.starts_with("HEAD~")
        || base.starts_with("HEAD^")
        || base.starts_with("HEAD@")
        || base.starts_with("HEAD:")
    {
        return None;
    }

    if base == "FETCH_HEAD" || base == "ORIG_HEAD" || base == "MERGE_HEAD" {
        return None;
    }

    if is_commit_id(base) {
        return None;
    }

    let clean = if let Some(stripped) = base.strip_prefix("refs/heads/") {
        stripped
    } else if let Some(stripped) = base.strip_prefix("refs/remotes/") {
        stripped
    } else {
        base
    };

    let is_wip = clean == "wip"
        || clean.starts_with("wip/")
        || clean == "origin/wip"
        || clean.starts_with("origin/wip/");
    if is_wip {
        return None;
    }

    let (display_name, candidates) = if let Some(without_origin) = clean.strip_prefix("origin/") {
        if without_origin.is_empty()
            || without_origin == "wip"
            || without_origin.starts_with("wip/")
            || without_origin == "HEAD"
            || without_origin.starts_with("HEAD~")
            || without_origin.starts_with("HEAD^")
            || is_commit_id(without_origin)
        {
            return None;
        }
        let cand1 = format!("refs/remotes/{clean}");
        let cand2 = format!("refs/heads/{without_origin}");
        (without_origin.to_string(), vec![cand1, cand2])
    } else {
        let cand1 = format!("refs/heads/{clean}");
        let cand2 = format!("refs/remotes/origin/{clean}");
        (clean.to_string(), vec![cand1, cand2])
    };

    let mut resolved = Vec::new();
    for cand in candidates {
        if git_ok(root, &["rev-parse", "--verify", "--quiet", &cand]) {
            resolved.push(cand);
        }
    }

    if resolved.is_empty() {
        None
    } else {
        Some((display_name, resolved))
    }
}

/// Reads `base` from `.wt/<id>.start.json` if present.
pub fn read_run_base(root: &Path, id: &str) -> Option<String> {
    let start_path = root.join(".wt").join(format!("{id}.start.json"));
    std::fs::read_to_string(&start_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("base")
                .and_then(|b| b.as_str())
                .map(|s| s.to_string())
        })
}

/// Returns the commit SHA at the tip of `branch_name` if it exists.
pub fn get_branch_tip(root: &Path, branch_name: &str) -> Option<String> {
    let check = std::process::Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch_name}"),
        ])
        .current_dir(root)
        .output()
        .ok()?;
    if !check.status.success() {
        return None;
    }

    let tip_out = std::process::Command::new("git")
        .args(["rev-parse", &format!("refs/heads/{branch_name}")])
        .current_dir(root)
        .output()
        .ok()?;
    if tip_out.status.success() {
        Some(String::from_utf8_lossy(&tip_out.stdout).trim().to_string())
    } else {
        None
    }
}

/// Checks whether a branch's work is on the default branch when any of:
/// - `git merge-base --is-ancestor <tip> <default>` succeeds;
/// - every commit `git cherry <default> <branch>` lists is marked `-`;
/// - merging it would change nothing: `git merge-tree --write-tree <default> <branch>` exits 0
///   and the tree id it prints (first line) equals `git rev-parse <default>^{tree}`.
pub fn is_branch_merged(root: &Path, branch_name: &str) -> bool {
    let mut ctx = CleanContext::new(root);
    if let Some(id) = branch_name.strip_prefix("wip/") {
        ctx.check_run_merged(id).is_some()
    } else {
        is_branch_merged_with_targets(root, branch_name, &[])
    }
}

/// Checks whether a branch's work is on the default branch or any of the extra target refs.
pub fn is_branch_merged_with_targets(
    root: &Path,
    branch_name: &str,
    extra_targets: &[&str],
) -> bool {
    let Some(tip_sha) = get_branch_tip(root, branch_name) else {
        return false;
    };

    is_commit_merged_with_targets(root, &tip_sha, extra_targets)
}

/// Checks whether a branch's work is on the default branch or any of the extra target refs.
pub fn is_branch_merged_with_extra_targets(
    root: &Path,
    branch_name: &str,
    extra_targets: &[&str],
) -> bool {
    is_branch_merged_with_targets(root, branch_name, extra_targets)
}

/// Checks whether a commit's work is on the default branch when any of:
/// - `git merge-base --is-ancestor <commit> <default>` succeeds;
/// - every commit `git cherry <default> <commit>` lists is marked `-`;
/// - merging it would change nothing: `git merge-tree --write-tree <default> <commit>` exits 0
///   and the tree id it prints equals `git rev-parse <default>^{tree}`.
pub fn is_commit_merged(root: &Path, commit_sha: &str) -> bool {
    is_commit_merged_with_targets(root, commit_sha, &[])
}

/// Checks whether a commit's work is on the default branch or any of the extra target refs.
pub fn is_commit_merged_with_targets(
    root: &Path,
    commit_sha: &str,
    extra_targets: &[&str],
) -> bool {
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
    for extra in extra_targets {
        if git_ok(root, &["rev-parse", "--verify", "--quiet", extra]) {
            targets.push(extra.to_string());
        }
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

/// Lists all run IDs that have a corresponding `.wt/<id>.start.json`.
pub fn list_all_run_ids(root: &Path) -> Vec<String> {
    let wt_dir = root.join(".wt");
    let mut ids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&wt_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(id) = name.strip_suffix(".start.json") {
                ids.push(id.to_string());
            }
        }
    }
    ids.sort();
    ids
}

/// Finds a follow-up run that makes `<id>` merged.
pub fn find_merged_followup(root: &Path, id: &str) -> Option<String> {
    let mut ctx = CleanContext::new(root);
    ctx.find_merged_followup(id)
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

    let mut ctx = CleanContext::new(root);
    let ids = find_blyte_worktrees(root);
    for id in ids {
        if skip_id == Some(&id) {
            continue;
        }

        let wt_path = root.join(".wt").join(&id);
        match ctx.check_removable(&id, older_than_days) {
            Err(kept_reason) => {
                if !quiet {
                    println!("kept .wt/{id} ({kept_reason})");
                }
            }
            Ok(removal_reason) => {
                let is_res = ctx.is_research_run(&id);
                if !is_res && has_uncommitted_changes(&wt_path) {
                    if !quiet {
                        println!("kept .wt/{id} (uncommitted changes)");
                    }
                    continue;
                }

                let size = dir_size(&wt_path);
                let branch_name = format!("wip/{id}");
                let branch_exists = ctx.get_branch_tip(&branch_name).is_some();
                let is_merged = branch_exists
                    && (matches!(
                        removal_reason,
                        RemovalReason::Merged
                            | RemovalReason::MergedInto(_)
                            | RemovalReason::MergedThrough(_)
                    ) || ctx.check_run_merged(&id).is_some());

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
                            if del_out.map(|o| o.status.success()).unwrap_or(false) {
                                ctx.wip_branch_tips.remove(&branch_name);
                                if !quiet {
                                    println!("deleted branch {branch_name} (merged)");
                                }
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

/// Formats a worktree path for display and for git CLI commands:
/// relative to `root` if inside `root` or a sibling (same parent directory);
/// otherwise absolute.
pub fn display_worktree_path(target: &Path, root: &Path) -> String {
    let target_canon = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    // Inside the repository root
    if let Ok(rel) = target_canon.strip_prefix(&root_canon) {
        if !rel.as_os_str().is_empty() {
            return rel.to_string_lossy().replace('\\', "/");
        }
    }

    // Sibling (same parent directory, e.g. ../app--side)
    if target_canon != root_canon {
        if let (Some(root_parent), Some(target_parent)) =
            (root_canon.parent(), target_canon.parent())
        {
            if root_parent == target_parent {
                if let Some(file_name) = target_canon.file_name() {
                    let rel = Path::new("..").join(file_name);
                    return rel.to_string_lossy().replace('\\', "/");
                }
            }
        }
    }

    // Otherwise show the absolute path
    target_canon.to_string_lossy().replace('\\', "/")
}

/// Returns true if `root` is a linked worktree (its git-dir differs from its git-common-dir).
pub fn is_linked_worktree(root: &Path) -> bool {
    let git_dir_out = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(root)
        .output();
    let common_dir_out = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(root)
        .output();
    match (git_dir_out, common_dir_out) {
        (Ok(gd), Ok(cd)) if gd.status.success() && cd.status.success() => {
            let gd_str = String::from_utf8_lossy(&gd.stdout).trim().to_string();
            let cd_str = String::from_utf8_lossy(&cd.stdout).trim().to_string();
            if gd_str == cd_str {
                return false;
            }
            let gd_path = if Path::new(&gd_str).is_absolute() {
                PathBuf::from(&gd_str)
            } else {
                root.join(&gd_str)
            };
            let cd_path = if Path::new(&cd_str).is_absolute() {
                PathBuf::from(&cd_str)
            } else {
                root.join(&cd_str)
            };
            let gd_canon = gd_path.canonicalize().unwrap_or(gd_path);
            let cd_canon = cd_path.canonicalize().unwrap_or(cd_path);
            gd_canon != cd_canon
        }
        _ => false,
    }
}

/// Returns true if any path component is `.wt`.
pub fn has_wt_component(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == ".wt")
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

/// Inspects other worktrees (excluding main checkout, linked-worktree roots,
/// and blyte's `.wt/<id>` runs), and reports any that are both merged and clean.
/// Never deletes them.
pub fn report_other_worktrees(root: &Path) {
    // Only report from the main worktree. If the root being cleaned is a linked worktree,
    // print nothing about other worktrees for it.
    if is_linked_worktree(root) {
        return;
    }

    let worktrees = parse_worktree_list(root);
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let main_canon = worktrees
        .first()
        .map(|wt| wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone()));

    let mut other_count = 0;
    let mut other_bytes = 0;

    for (idx, wt) in worktrees.into_iter().enumerate() {
        // Never list the main worktree (first entry of git worktree list --porcelain)
        if idx == 0 {
            continue;
        }
        if wt.is_bare {
            continue;
        }
        if !wt.path.exists() {
            continue;
        }

        let wt_canon = wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone());
        // Skip the main checkout as well as the root itself
        if wt_canon == root_canon || wt.path == root {
            continue;
        }
        if Some(&wt_canon) == main_canon.as_ref() {
            continue;
        }
        // Never list blyte runs: exclude any worktree whose path has a .wt directory component anywhere
        if has_wt_component(&wt.path) || has_wt_component(&wt_canon) {
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
        let path_str = display_worktree_path(&wt.path, root);
        let branch_label = wt.branch.as_deref().unwrap_or_else(|| {
            wt.head_sha
                .as_deref()
                .map(|s| &s[..s.len().min(7)])
                .unwrap_or("detached")
        });

        println!(
            "merged worktree {path_str} ({branch_label}, {}): git worktree remove {path_str}",
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

    let mut ctx = CleanContext::new(root);
    let branch_name = format!("wip/{id}");
    let branch_exists = ctx.get_branch_tip(&branch_name).is_some();

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

    let is_res = ctx.is_research_run(id);
    let removable = match ctx.check_removable(id, 14) {
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
        let is_merged = ctx.check_run_merged(id).is_some();
        if is_merged {
            let del_out = std::process::Command::new("git")
                .args(["branch", "-D", &branch_name])
                .current_dir(root)
                .output();
            if del_out.map(|o| o.status.success()).unwrap_or(false) {
                ctx.wip_branch_tips.remove(&branch_name);
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
