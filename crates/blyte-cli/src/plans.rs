//! Plan parser and plans collector for the dashboard (plan 2026-10-01).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, RwLock};

use crate::dashboard::{self, Job, Sources};

static IS_MERGED_GIT_CALLS: AtomicUsize = AtomicUsize::new(0);

type IsMergedKey = (PathBuf, String, String);
type IsMergedCache = LazyLock<RwLock<HashMap<IsMergedKey, bool>>>;

static IS_MERGED_CACHE: IsMergedCache = LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn is_merged_git_call_count() -> usize {
    IS_MERGED_GIT_CALLS.load(Ordering::SeqCst)
}

pub fn reset_is_merged_git_call_count() {
    IS_MERGED_GIT_CALLS.store(0, Ordering::SeqCst);
}

pub fn clear_is_merged_cache() {
    if let Ok(mut cache) = IS_MERGED_CACHE.write() {
        cache.clear();
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlanDoc {
    pub title: String,
    pub tasks: Vec<PlanTask>,
    pub decisions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    pub heading: String,
    pub deps: Vec<String>,
    pub lane: String,
    pub files: Vec<String>,
}

fn parse_task_deps(lines: &[&str], plan_task_ids: &HashSet<String>) -> Vec<String> {
    let dep_line = lines
        .iter()
        .find(|l| {
            let plain = l.replace("**", "").replace("__", "");
            let trimmed = plain
                .trim_start()
                .trim_start_matches(['-', '*', '>'])
                .trim_start();
            trimmed.to_lowercase().starts_with("depends on:")
        })
        .or_else(|| {
            lines.iter().find(|l| {
                let plain = l.replace("**", "").replace("__", "");
                plain.to_lowercase().contains("depends on:")
            })
        });

    let target_line = if let Some(l) = dep_line {
        *l
    } else if let Some(l) = lines.iter().find(|l| {
        let plain = l.replace("**", "").replace("__", "");
        plain.trim_start().to_lowercase().starts_with("needs task")
    }) {
        *l
    } else {
        return Vec::new();
    };

    let plain = target_line.replace("**", "").replace("__", "");
    let lower = plain.to_lowercase();

    let Some(pos) = lower.find("depends on:") else {
        if lower.contains("none") {
            return Vec::new();
        }
        return extract_task_ids_from_line(&plain);
    };

    let after = &plain[pos + "depends on:".len()..];

    let mut cut_end = after.len();
    if let Some(p) = after.find('(') {
        cut_end = cut_end.min(p);
    }
    if let Some(p) = after.find(';') {
        cut_end = cut_end.min(p);
    }
    if let Some(p) = after.find(". ") {
        cut_end = cut_end.min(p);
    }
    for (i, _) in after.char_indices() {
        if let Some(sub) = after[i..].get(..9) {
            if sub.eq_ignore_ascii_case("siblings:") {
                cut_end = cut_end.min(i);
                break;
            }
        }
    }
    let trimmed_end = after.trim_end();
    if trimmed_end.ends_with('.') {
        let dot_pos = trimmed_end.len() - 1;
        cut_end = cut_end.min(dot_pos);
    }

    let clause = &after[..cut_end];

    let lower_clause = clause.to_lowercase();
    let trimmed = lower_clause
        .trim()
        .trim_matches(['.', '*', '`', '_', ';'])
        .trim();
    if trimmed == "none" || trimmed.starts_with("none ") || trimmed.starts_with("none,") {
        return Vec::new();
    }

    let mut result = extract_task_ids_from_line(clause);

    for token in clause.split([',', ' ', '\t', '\r', '\n']) {
        let t = token
            .trim()
            .trim_matches(['`', '*', '_', '.', ':', '[', ']'])
            .trim();
        if t.is_empty() || t.eq_ignore_ascii_case("and") {
            continue;
        }
        if plan_task_ids.contains(t) && !result.contains(&t.to_string()) {
            result.push(t.to_string());
        }
    }

    result
}

fn extract_task_ids_from_line(line: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i + 4 <= len {
        let slice: String = chars[i..i + 4].iter().collect();
        if slice.eq_ignore_ascii_case("task") {
            let prev_ok = i == 0 || !chars[i - 1].is_alphanumeric();
            let next_ok = i + 4 < len && (chars[i + 4] == ' ' || chars[i + 4] == '\t');
            if prev_ok && next_ok {
                let mut j = i + 4;
                while j < len && (chars[j] == ' ' || chars[j] == '\t') {
                    j += 1;
                }
                let id_start = j;
                while j < len && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '-')
                {
                    j += 1;
                }
                let token: String = chars[id_start..j].iter().collect();
                if !token.is_empty() {
                    let id = if token.chars().all(|c| c.is_ascii_digit()) {
                        format!("T{token}")
                    } else {
                        token
                    };
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    ids
}

fn parse_task_files(lines: &[&str]) -> Vec<String> {
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("**Files:**")
            || trimmed.starts_with("Files:")
            || trimmed.contains("**Files:**")
            || trimmed.contains("Files:")
        {
            let mut files = Vec::new();
            let mut in_backtick = false;
            let mut current = String::new();
            for c in line.chars() {
                if c == '`' {
                    if in_backtick {
                        let path = current.trim().to_string();
                        if (path.contains('/') || path.contains('.'))
                            && !path.contains(' ')
                            && !path.is_empty()
                        {
                            files.push(path);
                        }
                        current.clear();
                        in_backtick = false;
                    } else {
                        in_backtick = true;
                    }
                } else if in_backtick {
                    current.push(c);
                }
            }
            return files;
        }
    }
    Vec::new()
}

fn parse_task_lane(lines: &[&str], files: &[String]) -> String {
    for line in lines {
        let trimmed = line.trim();
        let plain = trimmed.replace("**", "");
        if let Some(rest) = plain.strip_prefix("Lane:") {
            let l = rest.trim();
            if !l.is_empty() {
                return l.to_string();
            }
        }
    }
    if let Some(first_file) = files.first() {
        if let Some(rest) = first_file.strip_prefix("crates/") {
            let x = rest.split('/').next().unwrap_or(rest);
            x.to_string()
        } else {
            let first_comp = first_file.split('/').next().unwrap_or(first_file);
            first_comp.to_string()
        }
    } else {
        "Tasks".to_string()
    }
}

fn parse_task_heading_id_title(h_text: &str) -> Option<(String, String)> {
    let after_task = h_text.strip_prefix("Task ")?;
    let (n_part, title_part) = after_task.split_once(':')?;
    let n = n_part.trim();
    let (first_word, rest_of_n) = match n.split_once(char::is_whitespace) {
        Some((w, r)) => (w, r.trim()),
        None => (n, ""),
    };
    if first_word.is_empty() {
        return None;
    }
    let id = if first_word.chars().all(|c| c.is_ascii_digit()) {
        format!("T{first_word}")
    } else {
        first_word.to_string()
    };

    let raw_title = if rest_of_n.is_empty() {
        title_part.trim().to_string()
    } else if title_part.trim().is_empty() {
        rest_of_n.to_string()
    } else {
        format!("{rest_of_n} {}", title_part.trim())
    };

    let task_title = raw_title.replace('`', "").trim().to_string();
    if task_title.is_empty() {
        return None;
    }

    Some((id, task_title))
}

pub fn parse_plan(text: &str) -> PlanDoc {
    let mut lines = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            lines.push(line);
        }
    }

    let mut title = String::new();
    for line in &lines {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let t = rest.trim();
            let t = t
                .strip_suffix(" — Plan")
                .or_else(|| t.strip_suffix(" – Plan"))
                .or_else(|| t.strip_suffix(" - Plan"))
                .unwrap_or(t);
            title = t.trim().to_string();
            break;
        }
    }

    struct Heading {
        level: usize,
        text: String,
        line_idx: usize,
    }

    let mut headings = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=4).contains(&hashes) {
            let after = &trimmed[hashes..];
            if after.starts_with(' ') || after.starts_with('\t') {
                headings.push(Heading {
                    level: hashes,
                    text: after.trim().to_string(),
                    line_idx: idx,
                });
            }
        }
    }

    let mut plan_task_ids = HashSet::new();
    for h in &headings {
        if (2..=4).contains(&h.level) {
            if let Some((id, _)) = parse_task_heading_id_title(&h.text) {
                plan_task_ids.insert(id);
            }
        }
    }

    let mut tasks = Vec::new();
    let mut decisions = Vec::new();

    for (h_idx, h) in headings.iter().enumerate() {
        let mut end_line = lines.len();
        for next_h in &headings[h_idx + 1..] {
            if next_h.level <= h.level {
                end_line = next_h.line_idx;
                break;
            }
        }
        let section_lines = &lines[h.line_idx + 1..end_line];

        if (2..=4).contains(&h.level) {
            if let Some((id, task_title)) = parse_task_heading_id_title(&h.text) {
                let heading = h.text.clone();
                let deps = parse_task_deps(section_lines, &plan_task_ids);
                let files = parse_task_files(section_lines);
                let lane = parse_task_lane(section_lines, &files);

                tasks.push(PlanTask {
                    id,
                    title: task_title,
                    heading,
                    deps,
                    lane,
                    files,
                });
            }
        }

        if h.text.eq_ignore_ascii_case("Decisions") {
            for line in section_lines {
                let trimmed = line.trim();
                if let Some(bullet) = trimmed.strip_prefix("- ") {
                    let b = bullet.trim();
                    if !b.is_empty() {
                        decisions.push(b.to_string());
                    }
                }
            }
        }
    }

    PlanDoc {
        title,
        tasks,
        decisions,
    }
}

fn matches_task(task: &PlanTask, pt: &str) -> bool {
    let pt = pt.trim();
    if pt.is_empty() {
        return false;
    }
    let pt_clean = pt.replace('`', "");
    let heading_clean = task.heading.replace('`', "");
    if pt == task.heading
        || pt == task.title
        || pt == task.id
        || pt_clean == task.title
        || pt_clean == heading_clean
    {
        return true;
    }
    if let Some(after_task) = task.heading.strip_prefix("Task ") {
        let n = after_task
            .split_once(':')
            .map(|(n, _)| n.trim())
            .unwrap_or(after_task.trim());
        let prefix = format!("Task {n}");
        if let Some(rest) = pt.strip_prefix(&prefix) {
            if rest.is_empty() || rest.starts_with(|c: char| !c.is_alphanumeric() && c != '_') {
                return true;
            }
        }
    }
    let id_num = task.id.strip_prefix('T').unwrap_or(&task.id);
    let id_prefix = format!("Task {id_num}");
    if let Some(rest) = pt.strip_prefix(&id_prefix) {
        if rest.is_empty() || rest.starts_with(|c: char| !c.is_alphanumeric() && c != '_') {
            return true;
        }
    }
    false
}

fn parse_added_task_id_and_title(
    name: &str,
    fallback_job_id: &str,
    fallback_job_title: &str,
) -> (String, String) {
    let clean = name.replace('`', "");
    let trimmed = clean.trim();
    let after_task = if let Some(rest) = trimmed.strip_prefix("Task ") {
        rest.trim_start()
    } else {
        trimmed
    };

    let first_token = after_task
        .split(|c: char| c.is_whitespace() || c == ':')
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let id = if first_token.is_empty() {
        fallback_job_id.to_string()
    } else {
        first_token.to_string()
    };

    let title_candidate = if let Some(rest) = after_task.strip_prefix(&id) {
        let rest = rest.trim_start();
        let rest = rest.strip_prefix(':').unwrap_or(rest);
        rest.trim()
    } else {
        ""
    };

    let title = if title_candidate.is_empty() {
        fallback_job_title.to_string()
    } else {
        title_candidate.to_string()
    };

    (id, title)
}

fn first_backticked_plan_path(text: &str) -> Option<String> {
    let mut in_code_fence = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }
        let mut in_backtick = false;
        let mut current = String::new();
        for c in line.chars() {
            if c == '`' {
                if in_backtick {
                    let s = current.trim();
                    if s.ends_with(".md") && s.contains("plans/") {
                        return Some(s.to_string());
                    }
                    current.clear();
                    in_backtick = false;
                } else {
                    in_backtick = true;
                }
            } else if in_backtick {
                current.push(c);
            }
        }
    }
    None
}

fn find_tag_plan_stem(job: &Job) -> Option<String> {
    for tag in &job.file_tags {
        if let Some(rest) = tag.strip_prefix("plan:") {
            let stem = rest.trim();
            if !stem.is_empty() {
                return Some(stem.to_string());
            }
        }
    }
    let synth = job.plan.as_ref().and_then(|p| {
        Path::new(p)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| format!("plan: {s}"))
    });
    for tag in &job.tags {
        if let Some(ref s) = synth {
            if tag == s {
                continue;
            }
        }
        if let Some(rest) = tag.strip_prefix("plan:") {
            let stem = rest.trim();
            if !stem.is_empty() {
                return Some(stem.to_string());
            }
        }
    }
    None
}

fn resolve_bare_stem(
    root: &Path,
    stem: &str,
    git_names_cache: &mut HashMap<PathBuf, Vec<String>>,
) -> Option<PathBuf> {
    let target = format!("{stem}.md");
    let names = git_names_cache
        .entry(root.to_path_buf())
        .or_insert_with(|| {
            let Ok(out) = std::process::Command::new("git")
                .args(["log", "--all", "--format=", "--name-only"])
                .current_dir(root)
                .output()
            else {
                return Vec::new();
            };
            if !out.status.success() {
                return Vec::new();
            }
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        });

    for name in names {
        if Path::new(name).file_name().and_then(|n| n.to_str()) == Some(&target) {
            return Some(PathBuf::from(name.as_str()));
        }
    }

    if root.join(&target).is_file() {
        return Some(PathBuf::from(&target));
    }
    for sub in &["docs/plans", "docs", "plans"] {
        let candidate = Path::new(sub).join(&target);
        if root.join(&candidate).is_file() {
            return Some(candidate);
        }
    }

    None
}

fn is_plan_task_file(path: &Path) -> bool {
    let s = path.to_string_lossy();
    if !s.ends_with(".md") {
        return false;
    }
    let norm = s.replace('\\', "/");
    let norm_path = Path::new(&norm);
    let comps: Vec<&std::ffi::OsStr> = norm_path.components().map(|c| c.as_os_str()).collect();
    for window in comps.windows(3) {
        if window[0] == "docs" && window[1] == "superpowers" && window[2] == "plans" {
            return true;
        }
    }
    for window in comps.windows(2) {
        if window[0] == "docs" && window[1] == "plans" {
            return true;
        }
    }
    false
}

fn extract_relpath(root: &Path, plan_path: &Path, job_wt: Option<&str>) -> PathBuf {
    if plan_path.is_relative() {
        let p_str = plan_path.to_string_lossy();
        if let Some(stripped) = p_str.strip_prefix("./") {
            return PathBuf::from(stripped);
        }
        return plan_path.to_path_buf();
    }

    // 1. inside <root>/.wt/<id>/
    let wt_base = root.join(".wt");
    if let Ok(rest) = plan_path.strip_prefix(&wt_base) {
        let mut comps = rest.components();
        if comps.next().is_some() {
            let after_id = comps.as_path();
            if !after_id.as_os_str().is_empty() {
                return after_id.to_path_buf();
            }
        }
    }

    // 2. inside <root>/.claude/worktrees/<x>/
    let claude_base = root.join(".claude").join("worktrees");
    if let Ok(rest) = plan_path.strip_prefix(&claude_base) {
        let mut comps = rest.components();
        if comps.next().is_some() {
            let after_x = comps.as_path();
            if !after_x.as_os_str().is_empty() {
                return after_x.to_path_buf();
            }
        }
    }

    // 3. sibling <root>--<x>/
    if let (Some(root_parent), Some(root_name)) =
        (root.parent(), root.file_name().and_then(|n| n.to_str()))
    {
        if let Ok(rest) = plan_path.strip_prefix(root_parent) {
            let mut comps = rest.components();
            if let Some(first) = comps.next() {
                if let Some(first_str) = first.as_os_str().to_str() {
                    let prefix = format!("{root_name}--");
                    if first_str.starts_with(&prefix) {
                        let after_prefix = comps.as_path();
                        if !after_prefix.as_os_str().is_empty() {
                            return after_prefix.to_path_buf();
                        }
                    }
                }
            }
        }
    }

    // 4. inside root
    if let Ok(rel) = plan_path.strip_prefix(root) {
        if !rel.as_os_str().is_empty() {
            return rel.to_path_buf();
        }
    }

    // 5. inside job's worktree
    if let Some(wt) = job_wt {
        if let Ok(rel) = plan_path.strip_prefix(wt) {
            if !rel.as_os_str().is_empty() {
                return rel.to_path_buf();
            }
        }
    }

    plan_path.to_path_buf()
}

fn first_heading(text: &str) -> Option<String> {
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence && trimmed.starts_with('#') {
            let without_hashes = trimmed.trim_start_matches('#').trim();
            let without_suffix = without_hashes
                .strip_suffix(" — Plan")
                .or_else(|| without_hashes.strip_suffix(" – Plan"))
                .or_else(|| without_hashes.strip_suffix(" - Plan"))
                .unwrap_or(without_hashes);
            let cleaned = without_suffix.trim().replace('`', "");
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }
    None
}

fn find_home_repo(plan_file_abs: &Path, jobs: &[&Job]) -> String {
    // 1. For a plan inside a repository root, that root
    for job in jobs {
        if let Some(ref root) = job.root {
            if plan_file_abs.starts_with(root) {
                return dashboard::repo_name(root);
            }
        }
    }

    // 2. For an absolute plan path, the nearest ancestor directory containing .git
    let mut curr = plan_file_abs.parent();
    while let Some(dir) = curr {
        if dir.join(".git").exists() {
            return dashboard::repo_name(dir);
        }
        curr = dir.parent();
    }

    // 3. If none is found, the first job's repository
    if let Some(first) = jobs.first() {
        return first.repo.clone();
    }

    String::new()
}

fn read_plan_content(
    plan_file_abs: &Path,
    jobs: &[&Job],
    cache: &mut HashMap<PathBuf, Option<String>>,
) -> Option<String> {
    if let Some(cached) = cache.get(plan_file_abs) {
        return cached.clone();
    }

    // 1. <plan_file_abs> on disk
    if let Ok(content) = std::fs::read_to_string(plan_file_abs) {
        cache.insert(plan_file_abs.to_path_buf(), Some(content.clone()));
        return Some(content);
    }

    // 2. the job's own worktree + relpath
    for job in jobs {
        if let Some(ref wt) = job.worktree {
            let wt_base = Path::new(wt);
            if let Some(ref r) = job.root {
                if let Ok(rel) = plan_file_abs.strip_prefix(r) {
                    let wt_path = wt_base.join(rel);
                    if let Ok(content) = std::fs::read_to_string(&wt_path) {
                        cache.insert(plan_file_abs.to_path_buf(), Some(content.clone()));
                        return Some(content);
                    }
                }
            }
        }
    }

    // 3. each group job's absolute plan source path (the task_file, or an absolute start_plan) directly from disk
    for job in jobs {
        if let Some(ref tf) = job.task_file {
            let p = Path::new(tf);
            let matches_plan = p == plan_file_abs
                || job.root.as_ref().is_some_and(|r| {
                    r.join(extract_relpath(r, p, job.worktree.as_deref())) == *plan_file_abs
                });
            if matches_plan && p.is_file() {
                if let Ok(content) = std::fs::read_to_string(p) {
                    cache.insert(plan_file_abs.to_path_buf(), Some(content.clone()));
                    return Some(content);
                }
            }
        }
        if let Some(ref sp) = job.start_plan {
            let p = Path::new(sp);
            let matches_plan = p == plan_file_abs
                || job.root.as_ref().is_some_and(|r| {
                    r.join(extract_relpath(r, p, job.worktree.as_deref())) == *plan_file_abs
                });
            if matches_plan && p.is_file() {
                if let Ok(content) = std::fs::read_to_string(p) {
                    cache.insert(plan_file_abs.to_path_buf(), Some(content.clone()));
                    return Some(content);
                }
            }
        }
    }

    // 4. git: git -C <root> log --all -1 --format=%H -- <relpath> and git -C <root> show <sha>:<relpath>
    let mut git_roots = Vec::new();
    for job in jobs {
        if let Some(ref r) = job.root {
            if !git_roots.contains(r) {
                git_roots.push(r.clone());
            }
        }
    }
    let mut curr = plan_file_abs.parent();
    while let Some(dir) = curr {
        if dir.join(".git").exists() {
            let d_buf = dir.to_path_buf();
            if !git_roots.contains(&d_buf) {
                git_roots.push(d_buf);
            }
            break;
        }
        curr = dir.parent();
    }

    for git_root in &git_roots {
        if let Ok(rel) = plan_file_abs.strip_prefix(git_root) {
            let rel_str = rel.to_string_lossy().to_string();
            if let Ok(out) = std::process::Command::new("git")
                .args(["log", "--all", "-1", "--format=%H", "--", &rel_str])
                .current_dir(git_root)
                .output()
            {
                if out.status.success() {
                    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !sha.is_empty() {
                        if let Ok(show_out) = std::process::Command::new("git")
                            .args(["show", &format!("{sha}:{rel_str}")])
                            .current_dir(git_root)
                            .output()
                        {
                            if show_out.status.success() {
                                let content = String::from_utf8_lossy(&show_out.stdout).to_string();
                                cache.insert(plan_file_abs.to_path_buf(), Some(content.clone()));
                                return Some(content);
                            }
                        }
                    }
                }
            }
        }
    }

    cache.insert(plan_file_abs.to_path_buf(), None);
    None
}

pub fn is_merged(root: &Path, branch: &str, head: &str) -> bool {
    let key = (root.to_path_buf(), branch.to_string(), head.to_string());
    if let Ok(cache) = IS_MERGED_CACHE.read() {
        if let Some(&res) = cache.get(&key) {
            return res;
        }
    }

    IS_MERGED_GIT_CALLS.fetch_add(1, Ordering::SeqCst);

    let clean_branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
    let mut merged = false;
    if !head.is_empty() {
        if let Ok(output) = std::process::Command::new("git")
            .args(["branch", "--contains", head, "--format=%(refname:short)"])
            .current_dir(root)
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let b = line.trim();
                    if !b.is_empty()
                        && b != clean_branch
                        && !b.starts_with("wip/")
                        && !b.starts_with('(')
                    {
                        merged = true;
                        break;
                    }
                }
            }
        }
    }

    if let Ok(mut cache) = IS_MERGED_CACHE.write() {
        cache.insert(key, merged);
    }
    merged
}

fn is_job_merged(job: &Job) -> bool {
    let Some(ref root) = job.root else {
        return false;
    };
    let branch = format!("wip/{}", job.id);
    if let Some(last_commit) = job.commits.last() {
        if is_merged(root, &branch, &last_commit.sha) {
            return true;
        }
    }

    let is_accepted_or_fixed = job
        .verdict
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("accepted") || v.eq_ignore_ascii_case("fixed"));

    if is_accepted_or_fixed && !job.id.is_empty() {
        let branch_ref = format!("refs/heads/wip/{}", job.id);
        let Ok(output) = std::process::Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", &branch_ref])
            .current_dir(root)
            .output()
        else {
            return false;
        };
        if !output.status.success() {
            return true;
        }
    }

    false
}

fn choose_task_job<'a>(jobs: &[&'a Job]) -> Option<&'a Job> {
    // 1. a job that is running (the latest running one);
    if let Some(j) = jobs
        .iter()
        .copied()
        .filter(|j| j.status == "running")
        .max_by_key(|j| j.started_at.unwrap_or(0))
    {
        return Some(j);
    }

    // 2. else a verified job for which is_job_merged(job) is true (the latest such one);
    if let Some(j) = jobs
        .iter()
        .copied()
        .filter(|j| j.status == "verified" && is_job_merged(j))
        .max_by_key(|j| j.started_at.unwrap_or(0))
    {
        return Some(j);
    }

    // 3. else a job whose verdict is accepted or fixed (the latest such one);
    if let Some(j) = jobs
        .iter()
        .copied()
        .filter(|j| {
            j.verdict.as_deref().is_some_and(|v| {
                v.eq_ignore_ascii_case("accepted") || v.eq_ignore_ascii_case("fixed")
            })
        })
        .max_by_key(|j| j.started_at.unwrap_or(0))
    {
        return Some(j);
    }

    // 4. else the latest job, as today.
    jobs.iter()
        .copied()
        .max_by_key(|j| j.started_at.unwrap_or(0))
}

struct PlanItem {
    key: String,
    has_running: bool,
    latest_activity: u64,
    val: serde_json::Value,
}

struct PlanEvent {
    at: u64,
    task: String,
    text: String,
}

fn job_explicit_plan_relpath(
    job: &Job,
    git_names_cache: &mut HashMap<PathBuf, Vec<String>>,
) -> Option<PathBuf> {
    let root = job.root.as_ref()?;
    if let Some(ref p) = job.start_plan {
        if !p.contains('/') && !p.ends_with(".md") {
            if let Some(rp) = resolve_bare_stem(root, p, git_names_cache) {
                Some(rp)
            } else {
                Some(extract_relpath(root, Path::new(p), job.worktree.as_deref()))
            }
        } else {
            Some(extract_relpath(root, Path::new(p), job.worktree.as_deref()))
        }
    } else if let Some(stem) = find_tag_plan_stem(job) {
        if stem.contains('/') {
            Some(extract_relpath(
                root,
                Path::new(&stem),
                job.worktree.as_deref(),
            ))
        } else {
            let stem_clean = stem.strip_suffix(".md").unwrap_or(&stem);
            resolve_bare_stem(root, stem_clean, git_names_cache)
        }
    } else if let Some(ref p) = job.plan {
        Some(extract_relpath(root, Path::new(p), job.worktree.as_deref()))
    } else if let Some(ref tf) = job
        .task_file
        .as_ref()
        .filter(|tf| is_plan_task_file(Path::new(tf)))
    {
        Some(extract_relpath(
            root,
            Path::new(tf),
            job.worktree.as_deref(),
        ))
    } else {
        first_backticked_plan_path(&job.task)
            .map(|p| extract_relpath(root, Path::new(&p), job.worktree.as_deref()))
    }
}

fn build_standalone_plan(
    job: &Job,
    plan_file_abs: &Path,
    content: &str,
    doc: &PlanDoc,
    now: u64,
) -> Option<PlanItem> {
    let seven_days = 7 * 24 * 3600;
    let is_active = job.status == "running"
        || job
            .started_at
            .is_some_and(|st| st >= now.saturating_sub(seven_days));
    if !is_active {
        return None;
    }

    let heading_title = first_heading(content);
    let title = heading_title.unwrap_or_else(|| job.id.clone());
    let task_title = title.clone();
    let task_heading = title.clone();

    let state = if job.status == "running" {
        "running".to_string()
    } else if job.verdict.as_deref() == Some("redone") {
        "redone".to_string()
    } else if job.status == "verified" {
        if is_job_merged(job) {
            "merged".to_string()
        } else {
            "verified".to_string()
        }
    } else {
        "failed".to_string()
    };

    let files = if !job.changed_files.is_empty() {
        job.changed_files.clone()
    } else {
        Vec::new()
    };

    let task_val = serde_json::json!({
        "id": job.id,
        "title": task_title,
        "heading": task_heading,
        "lane": "tasks",
        "deps": [],
        "state": state,
        "dispatch": job.key,
        "now": job.now,
        "gate_running": job.gate_running,
        "gate_attempts": job.gate_attempts,
        "elapsed_s": job.elapsed_s,
        "cost_usd": job.cost_usd,
        "turns": job.turns,
        "files": files,
        "commits": job.commits,
        "verdict": job.verdict,
        "notes": job.verdict_notes,
        "added": false,
    });

    let mut events = Vec::new();
    if let Some(st) = job.started_at {
        events.push(PlanEvent {
            at: st,
            task: job.id.clone(),
            text: "dispatched".to_string(),
        });
    }
    if job.status != "running" {
        if let Some(fin) = job.finished_at.or(job.log_mtime) {
            events.push(PlanEvent {
                at: fin,
                task: job.id.clone(),
                text: job.status.clone(),
            });
        }
    }
    if let Some(ref v) = job.verdict {
        if let Some(ts) = job.verdict_ts {
            events.push(PlanEvent {
                at: ts,
                task: job.id.clone(),
                text: format!("verdict {v}"),
            });
        }
    }
    events.sort_by_key(|e| std::cmp::Reverse(e.at));
    let events_json: Vec<serde_json::Value> = events
        .into_iter()
        .map(|e| {
            serde_json::json!({
                "at": e.at,
                "task": e.task,
                "text": e.text,
            })
        })
        .collect();

    let mut decisions_json = Vec::new();
    for d in &doc.decisions {
        decisions_json.push(serde_json::json!({
            "text": d,
            "source": "plan",
        }));
    }
    if let Some(ref notes) = job.verdict_notes {
        if !notes.trim().is_empty() {
            decisions_json.push(serde_json::json!({
                "text": notes,
                "source": format!("{} verdict note", job.id),
            }));
        }
    }

    let (merged, verified, running, not_started) = match state.as_str() {
        "merged" => (1, 0, 0, 0),
        "verified" => (0, 1, 0, 0),
        "running" => (0, 0, 1, 0),
        "ready" | "waiting" => (0, 0, 0, 1),
        _ => (0, 0, 0, 0),
    };
    let cost_usd = job.cost_usd.unwrap_or(0.0);
    let cost_usd = (cost_usd * 100.0).round() / 100.0;
    let stats_json = serde_json::json!({
        "tasks": 1,
        "merged": merged,
        "verified": verified,
        "running": running,
        "not_started": not_started,
        "cost_usd": cost_usd,
        "first_dispatch_at": job.started_at,
        "max_parallel": if running > 0 { 1 } else { 0 },
    });

    let key = format!("{}/{}", job.repo, job.id);
    let repo = job.repo.clone();
    let repos = vec![job.repo.clone()];
    let lanes = vec![serde_json::json!({
        "id": "tasks",
        "label": "Tasks",
    })];
    let path_str = plan_file_abs.to_string_lossy().to_string();

    let has_running = running > 0;
    let latest_activity = events_json
        .first()
        .and_then(|e| e["at"].as_u64())
        .or(job.started_at)
        .unwrap_or(0);

    let plan_val = serde_json::json!({
        "key": key,
        "repo": repo,
        "repos": repos,
        "path": path_str,
        "title": title,
        "lanes": lanes,
        "tasks": [task_val],
        "events": events_json,
        "decisions": decisions_json,
        "stats": stats_json,
    });

    Some(PlanItem {
        key,
        has_running,
        latest_activity,
        val: plan_val,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollectPlansOptions {
    pub running_only: bool,
}

pub fn collect_plans(src: &Sources, now: u64) -> serde_json::Value {
    collect_plans_with_options(src, now, CollectPlansOptions::default())
}

pub fn collect_plans_running(src: &Sources, now: u64) -> serde_json::Value {
    collect_plans_with_options(src, now, CollectPlansOptions { running_only: true })
}

pub fn collect_plans_filtered(src: &Sources, now: u64, running_only: bool) -> serde_json::Value {
    collect_plans_with_options(src, now, CollectPlansOptions { running_only })
}

pub fn collect_plans_with_options(
    src: &Sources,
    now: u64,
    opts: CollectPlansOptions,
) -> serde_json::Value {
    let mut plan_src = src.clone();
    plan_src.hours = plan_src.hours.max(7 * 24);
    let jobs = dashboard::collect_jobs(&plan_src, now);

    let mut git_names_cache: HashMap<PathBuf, Vec<String>> = HashMap::new();
    let mut plan_content_cache: HashMap<PathBuf, Option<String>> = HashMap::new();

    let (active_plan_files, active_standalone_job_ids) = if opts.running_only {
        let running_jobs: Vec<&Job> = jobs
            .iter()
            .filter(|j| !j.research && j.root.is_some() && j.status == "running")
            .collect();
        if running_jobs.is_empty() {
            return serde_json::json!({
                "generated_at": now,
                "plans": [],
            });
        }

        let mut plan_files = HashSet::new();
        let mut standalone_ids = HashSet::new();

        for job in &running_jobs {
            let root = job.root.as_ref().unwrap();
            if let Some(rp) = job_explicit_plan_relpath(job, &mut git_names_cache) {
                let plan_file_abs = root.join(rp);
                plan_files.insert(plan_file_abs);
            } else if let Some(ref tf) = job.task_file {
                let tf_path = Path::new(tf);
                let relpath = extract_relpath(root, tf_path, job.worktree.as_deref());
                let plan_file_abs = root.join(&relpath);
                if let Some(content) =
                    read_plan_content(&plan_file_abs, &[job], &mut plan_content_cache)
                {
                    let doc = parse_plan(&content);
                    let pt = job.plan_task.as_deref().unwrap_or(&job.id);
                    if doc.tasks.iter().any(|t| matches_task(t, pt)) {
                        plan_files.insert(plan_file_abs);
                    } else {
                        standalone_ids.insert(job.id.clone());
                    }
                } else {
                    standalone_ids.insert(job.id.clone());
                }
            }
        }

        if plan_files.is_empty() && standalone_ids.is_empty() {
            return serde_json::json!({
                "generated_at": now,
                "plans": [],
            });
        }

        (Some(plan_files), Some(standalone_ids))
    } else {
        (None, None)
    };

    let mut groups: HashMap<PathBuf, Vec<&Job>> = HashMap::new();
    let mut standalone_plans: Vec<PlanItem> = Vec::new();

    for job in &jobs {
        if job.research {
            continue;
        }
        let Some(ref root) = job.root else {
            continue;
        };

        if let Some(ref active_files) = active_plan_files {
            if let Some(rp) = job_explicit_plan_relpath(job, &mut git_names_cache) {
                let plan_file_abs = root.join(rp);
                if active_files.contains(&plan_file_abs) {
                    groups.entry(plan_file_abs).or_default().push(job);
                }
            } else if let Some(ref tf) = job.task_file {
                let tf_path = Path::new(tf);
                let relpath = extract_relpath(root, tf_path, job.worktree.as_deref());
                let plan_file_abs = root.join(&relpath);
                if active_files.contains(&plan_file_abs) {
                    if let Some(content) =
                        read_plan_content(&plan_file_abs, &[job], &mut plan_content_cache)
                    {
                        let doc = parse_plan(&content);
                        let pt = job.plan_task.as_deref().unwrap_or(&job.id);
                        if doc.tasks.iter().any(|t| matches_task(t, pt)) {
                            groups.entry(plan_file_abs).or_default().push(job);
                            continue;
                        }
                    }
                }
                if let Some(ref standalone_ids) = active_standalone_job_ids {
                    if standalone_ids.contains(&job.id) {
                        if let Some(content) =
                            read_plan_content(&plan_file_abs, &[job], &mut plan_content_cache)
                        {
                            let doc = parse_plan(&content);
                            if let Some(item) =
                                build_standalone_plan(job, &plan_file_abs, &content, &doc, now)
                            {
                                standalone_plans.push(item);
                            }
                        }
                    }
                }
            }
        } else {
            if let Some(rp) = job_explicit_plan_relpath(job, &mut git_names_cache) {
                let plan_file_abs = root.join(rp);
                groups.entry(plan_file_abs).or_default().push(job);
            } else {
                let Some(ref tf) = job.task_file else {
                    continue;
                };
                let tf_path = Path::new(tf);
                let relpath = extract_relpath(root, tf_path, job.worktree.as_deref());
                let plan_file_abs = root.join(&relpath);
                let Some(content) =
                    read_plan_content(&plan_file_abs, &[job], &mut plan_content_cache)
                else {
                    continue;
                };
                let doc = parse_plan(&content);
                let pt = job.plan_task.as_deref().unwrap_or(&job.id);
                let has_matching_task = doc.tasks.iter().any(|t| matches_task(t, pt));
                if has_matching_task {
                    groups.entry(plan_file_abs).or_default().push(job);
                    continue;
                }

                if let Some(item) = build_standalone_plan(job, &plan_file_abs, &content, &doc, now)
                {
                    standalone_plans.push(item);
                }
            }
        }
    }

    let mut plan_items = standalone_plans;

    for (plan_file_abs, jobs_in_group) in groups {
        let Some(content) =
            read_plan_content(&plan_file_abs, &jobs_in_group, &mut plan_content_cache)
        else {
            continue;
        };
        let doc = parse_plan(&content);
        let repo = find_home_repo(&plan_file_abs, &jobs_in_group);
        let stem = plan_file_abs
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("plan");
        let key = format!("{repo}/{stem}");
        let path_str = plan_file_abs.to_string_lossy().to_string();

        let mut task_jobs: HashMap<String, Vec<&Job>> = HashMap::new();
        let mut all_matched_jobs: Vec<&Job> = Vec::new();

        for task in &doc.tasks {
            let mut matched = Vec::new();
            for job in &jobs_in_group {
                // A plan file dispatched as the task, with no --plan-task: the run id names the task
                // (`blyte dispatch S1 <plan>.md`, 2026-10-02).
                let pt = job
                    .plan_task
                    .as_deref()
                    .or_else(|| {
                        job.task_file
                            .as_deref()
                            .filter(|tf| {
                                is_plan_task_file(Path::new(tf))
                                    || job.root.as_ref().is_some_and(|r| {
                                        r.join(extract_relpath(
                                            r,
                                            Path::new(tf),
                                            job.worktree.as_deref(),
                                        )) == plan_file_abs
                                    })
                            })
                            .map(|_| job.id.as_str())
                    })
                    .or(Some(job.id.as_str()));
                if let Some(pt) = pt {
                    if matches_task(task, pt) {
                        matched.push(*job);
                        if !all_matched_jobs.iter().any(|j| j.id == job.id) {
                            all_matched_jobs.push(*job);
                        }
                    }
                }
            }
            task_jobs.insert(task.id.clone(), matched);
        }

        let mut added_groups: Vec<(String, Vec<&Job>)> = Vec::new();
        for job in &jobs_in_group {
            if all_matched_jobs.iter().any(|j| j.id == job.id) {
                continue;
            }
            let is_explicit = job.start_plan.is_some() || find_tag_plan_stem(job).is_some();
            if !is_explicit {
                continue;
            }
            let name = job
                .plan_task
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&job.id)
                .to_string();

            if let Some((_, group_jobs)) = added_groups.iter_mut().find(|(n, _)| *n == name) {
                group_jobs.push(*job);
            } else {
                added_groups.push((name, vec![*job]));
            }
        }

        struct AddedTaskInfo<'a> {
            id: String,
            title: String,
            heading: String,
            jobs: Vec<&'a Job>,
            chosen: &'a Job,
            state: String,
        }

        let mut added_tasks: Vec<AddedTaskInfo> = Vec::new();
        for (name, group_jobs) in added_groups {
            let Some(chosen) = choose_task_job(&group_jobs) else {
                continue;
            };
            let (id, title) = parse_added_task_id_and_title(&name, &chosen.id, &chosen.title);
            let state = if chosen.status == "running" {
                "running".to_string()
            } else if chosen.verdict.as_deref() == Some("redone") {
                "redone".to_string()
            } else if chosen.status == "verified" {
                if is_job_merged(chosen) {
                    "merged".to_string()
                } else {
                    "verified".to_string()
                }
            } else {
                "failed".to_string()
            };

            for job in &group_jobs {
                if !all_matched_jobs.iter().any(|j| j.id == job.id) {
                    all_matched_jobs.push(*job);
                }
            }

            added_tasks.push(AddedTaskInfo {
                id,
                title,
                heading: name,
                jobs: group_jobs,
                chosen,
                state,
            });
        }

        let seven_days = 7 * 24 * 3600;
        let any_dispatched = all_matched_jobs.iter().any(|j| {
            j.status == "running"
                || j.started_at
                    .is_some_and(|st| st >= now.saturating_sub(seven_days))
        });
        if !any_dispatched {
            continue;
        }

        let mut task_states: HashMap<String, String> = HashMap::new();
        let mut chosen_jobs: HashMap<String, &Job> = HashMap::new();

        for task in &doc.tasks {
            if let Some(m_jobs) = task_jobs.get(&task.id) {
                if let Some(chosen) = choose_task_job(m_jobs) {
                    chosen_jobs.insert(task.id.clone(), chosen);
                    let state = if chosen.status == "running" {
                        "running".to_string()
                    } else if chosen.verdict.as_deref() == Some("redone") {
                        "redone".to_string()
                    } else if chosen.status == "verified" {
                        if is_job_merged(chosen) {
                            "merged".to_string()
                        } else {
                            "verified".to_string()
                        }
                    } else {
                        "failed".to_string()
                    };
                    task_states.insert(task.id.clone(), state);
                }
            }
        }

        for task in &doc.tasks {
            if !task_states.contains_key(&task.id) {
                let all_deps_merged = task
                    .deps
                    .iter()
                    .all(|dep_id| task_states.get(dep_id).map(|s| s.as_str()) == Some("merged"));
                let state = if all_deps_merged { "ready" } else { "waiting" };
                task_states.insert(task.id.clone(), state.to_string());
            }
        }

        for added in &added_tasks {
            task_jobs.insert(added.id.clone(), added.jobs.clone());
            task_states.insert(added.id.clone(), added.state.clone());
            chosen_jobs.insert(added.id.clone(), added.chosen);
        }

        let mut repos: Vec<String> = all_matched_jobs.iter().map(|j| j.repo.clone()).collect();
        repos.sort();
        repos.dedup();

        let is_multi_repo = repos.len() > 1;
        let mut lanes = Vec::new();

        if is_multi_repo {
            for r in &repos {
                lanes.push(serde_json::json!({
                    "id": r.to_lowercase().replace(' ', "-"),
                    "label": r,
                }));
            }
            let any_unrun = doc.tasks.iter().any(|t| !chosen_jobs.contains_key(&t.id));
            if any_unrun {
                lanes.push(serde_json::json!({
                    "id": "not-dispatched",
                    "label": "Not dispatched",
                }));
            }
        } else {
            let mut seen_lanes = HashSet::new();
            for t in &doc.tasks {
                if seen_lanes.insert(t.lane.clone()) {
                    let id = t.lane.to_lowercase().replace(' ', "-");
                    lanes.push(serde_json::json!({
                        "id": id,
                        "label": t.lane,
                    }));
                }
            }
            if !added_tasks.is_empty() {
                lanes.push(serde_json::json!({
                    "id": "added",
                    "label": "Added during the work",
                }));
            }
        }

        let mut tasks_json = Vec::new();
        for task in &doc.tasks {
            let state = task_states.get(&task.id).unwrap().clone();
            let lane_id = if is_multi_repo {
                if let Some(chosen) = chosen_jobs.get(&task.id) {
                    chosen.repo.to_lowercase().replace(' ', "-")
                } else {
                    "not-dispatched".to_string()
                }
            } else {
                task.lane.to_lowercase().replace(' ', "-")
            };
            let task_val = if let Some(job) = chosen_jobs.get(&task.id) {
                let files = if !job.changed_files.is_empty() {
                    job.changed_files.clone()
                } else {
                    task.files.clone()
                };
                serde_json::json!({
                    "id": task.id,
                    "title": task.title,
                    "heading": task.heading,
                    "lane": lane_id,
                    "deps": task.deps,
                    "state": state,
                    "dispatch": job.key,
                    "now": job.now,
                    "gate_running": job.gate_running,
                    "gate_attempts": job.gate_attempts,
                    "elapsed_s": job.elapsed_s,
                    "cost_usd": job.cost_usd,
                    "turns": job.turns,
                    "files": files,
                    "commits": job.commits,
                    "verdict": job.verdict,
                    "notes": job.verdict_notes,
                    "added": false,
                })
            } else {
                serde_json::json!({
                    "id": task.id,
                    "title": task.title,
                    "heading": task.heading,
                    "lane": lane_id,
                    "deps": task.deps,
                    "state": state,
                    "dispatch": serde_json::Value::Null,
                    "now": "",
                    "gate_running": false,
                    "gate_attempts": 0,
                    "elapsed_s": serde_json::Value::Null,
                    "cost_usd": serde_json::Value::Null,
                    "turns": 0,
                    "files": task.files,
                    "commits": [],
                    "verdict": serde_json::Value::Null,
                    "notes": serde_json::Value::Null,
                    "added": false,
                })
            };
            tasks_json.push(task_val);
        }

        for added in &added_tasks {
            let state = task_states.get(&added.id).unwrap().clone();
            let job = added.chosen;
            let lane_id = if is_multi_repo {
                job.repo.to_lowercase().replace(' ', "-")
            } else {
                "added".to_string()
            };
            let files = if !job.changed_files.is_empty() {
                job.changed_files.clone()
            } else {
                Vec::new()
            };
            let task_val = serde_json::json!({
                "id": added.id,
                "title": added.title,
                "heading": added.heading,
                "lane": lane_id,
                "deps": serde_json::json!([]),
                "state": state,
                "dispatch": job.key,
                "now": job.now,
                "gate_running": job.gate_running,
                "gate_attempts": job.gate_attempts,
                "elapsed_s": job.elapsed_s,
                "cost_usd": job.cost_usd,
                "turns": job.turns,
                "files": files,
                "commits": job.commits,
                "verdict": job.verdict,
                "notes": job.verdict_notes,
                "added": true,
            });
            tasks_json.push(task_val);
        }

        let all_task_ids: Vec<String> = doc
            .tasks
            .iter()
            .map(|t| t.id.clone())
            .chain(added_tasks.iter().map(|t| t.id.clone()))
            .collect();

        let mut events: Vec<PlanEvent> = Vec::new();
        for task_id in &all_task_ids {
            if let Some(m_jobs) = task_jobs.get(task_id) {
                for job in m_jobs {
                    if let Some(st) = job.started_at {
                        events.push(PlanEvent {
                            at: st,
                            task: task_id.clone(),
                            text: "dispatched".to_string(),
                        });
                    }
                    if job.status != "running" {
                        if let Some(fin) = job.finished_at.or(job.log_mtime) {
                            events.push(PlanEvent {
                                at: fin,
                                task: task_id.clone(),
                                text: job.status.clone(),
                            });
                        }
                    }
                    if let Some(ref v) = job.verdict {
                        if let Some(ts) = job.verdict_ts {
                            events.push(PlanEvent {
                                at: ts,
                                task: task_id.clone(),
                                text: format!("verdict {v}"),
                            });
                        }
                    }
                }
            }
        }
        events.sort_by_key(|e| std::cmp::Reverse(e.at));
        let events_json: Vec<serde_json::Value> = events
            .into_iter()
            .map(|e| {
                serde_json::json!({
                    "at": e.at,
                    "task": e.task,
                    "text": e.text,
                })
            })
            .collect();

        let mut decisions_json = Vec::new();
        for d in &doc.decisions {
            decisions_json.push(serde_json::json!({
                "text": d,
                "source": "plan",
            }));
        }
        for task_id in &all_task_ids {
            if let Some(job) = chosen_jobs.get(task_id) {
                if let Some(ref notes) = job.verdict_notes {
                    if !notes.trim().is_empty() {
                        decisions_json.push(serde_json::json!({
                            "text": notes,
                            "source": format!("{} verdict note", task_id),
                        }));
                    }
                }
            }
        }

        let tasks_count = doc.tasks.len() + added_tasks.len();
        let merged_count = task_states
            .values()
            .filter(|s| s.as_str() == "merged")
            .count();
        let verified_count = task_states
            .values()
            .filter(|s| s.as_str() == "verified")
            .count();
        let running_count = task_states
            .values()
            .filter(|s| s.as_str() == "running")
            .count();
        let not_started_count = task_states
            .values()
            .filter(|s| s.as_str() == "ready" || s.as_str() == "waiting")
            .count();
        let cost_usd: f64 = all_matched_jobs.iter().filter_map(|j| j.cost_usd).sum();
        let cost_usd = (cost_usd * 100.0).round() / 100.0;
        let first_dispatch_at = all_matched_jobs.iter().filter_map(|j| j.started_at).min();

        let intervals: Vec<(u64, u64)> = all_matched_jobs
            .iter()
            .filter_map(|j| {
                let st = j.started_at?;
                let fin = j.finished_at.unwrap_or(now);
                Some((st, fin))
            })
            .collect();
        let mut max_parallel = 0;
        for (t, _) in &intervals {
            let count = intervals
                .iter()
                .filter(|(st, fin)| *st <= *t && *t <= *fin)
                .count();
            if count > max_parallel {
                max_parallel = count;
            }
        }

        let stats_json = serde_json::json!({
            "tasks": tasks_count,
            "merged": merged_count,
            "verified": verified_count,
            "running": running_count,
            "not_started": not_started_count,
            "cost_usd": cost_usd,
            "first_dispatch_at": first_dispatch_at,
            "max_parallel": max_parallel,
        });

        let has_running = running_count > 0;
        if opts.running_only && !has_running {
            continue;
        }
        let latest_activity = events_json
            .first()
            .and_then(|e| e["at"].as_u64())
            .or(first_dispatch_at)
            .unwrap_or(0);

        let plan_val = serde_json::json!({
            "key": key,
            "repo": repo,
            "repos": repos,
            "path": path_str,
            "title": doc.title,
            "lanes": lanes,
            "tasks": tasks_json,
            "events": events_json,
            "decisions": decisions_json,
            "stats": stats_json,
        });

        plan_items.push(PlanItem {
            key,
            has_running,
            latest_activity,
            val: plan_val,
        });
    }

    if opts.running_only {
        plan_items.retain(|p| p.has_running);
    }

    plan_items.sort_by(|a, b| match (a.has_running, b.has_running) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => b
            .latest_activity
            .cmp(&a.latest_activity)
            .then_with(|| a.key.cmp(&b.key)),
    });

    serde_json::json!({
        "generated_at": now,
        "plans": plan_items.into_iter().map(|p| p.val).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_1_depends_on_clause_with_trailing_parens() {
        let plan = r#"# Test Plan
### Task OC2b: b
### Task OC3: c
### Task OC4: d
### Task OC5: e
**Depends on:** OC2b, OC3, OC4 (all on main before this is dispatched).
"#;
        let doc = parse_plan(plan);
        assert_eq!(doc.tasks[3].deps, ["OC2b", "OC3", "OC4"]);
    }

    #[test]
    fn test_2_depends_on_none_with_siblings() {
        let plan = r#"# Test Plan
### Task SL2: skill
### Task OC6: repl
**Depends on:** none. **Siblings:** SL2 changes skill.rs
"#;
        let doc = parse_plan(plan);
        assert!(doc.tasks[1].deps.is_empty());
    }

    #[test]
    fn test_3_depends_on_task_with_siblings() {
        let plan = r#"# Test Plan
### Task C1: cost
### Task ST1: state
### Task X: test
**Depends on:** C1. **Siblings:** ST1 changes repl.rs
"#;
        let doc = parse_plan(plan);
        assert_eq!(doc.tasks[2].deps, ["C1"]);
    }

    #[test]
    fn test_4_depends_on_with_paren_explanation() {
        let plan = r#"# Test Plan
### Task S1: setup
### Task S2: init
**Depends on:** S1 (uses setup::run_setup).
"#;
        let doc = parse_plan(plan);
        assert_eq!(doc.tasks[1].deps, ["S1"]);
    }

    #[test]
    fn test_5_depends_on_u1_and_u2() {
        let plan = r#"# Test Plan
### Task U1: usage
### Task U2: ids
### Task U3: cost
**Depends on:** U1 and U2
"#;
        let doc = parse_plan(plan);
        assert_eq!(doc.tasks[2].deps, ["U1", "U2"]);
    }

    #[test]
    fn test_6_single_and_short_task_ids() {
        let plan_t = r#"# Test Plan
### Task T1: first
### Task T2: second
### Task T3: third
**Depends on:** T1
"#;
        let doc_t = parse_plan(plan_t);
        assert_eq!(doc_t.tasks[2].deps, ["T1"]);

        let plan_ab = r#"# Test Plan
### Task A: first
### Task B: second
**Depends on:** A
"#;
        let doc_ab = parse_plan(plan_ab);
        assert_eq!(doc_ab.tasks[1].deps, ["A"]);
    }

    #[test]
    fn test_7_depends_on_numeric_form() {
        let plan = r#"# Test Plan
### Task 1: first
### Task 2: second
### Task 3: third
Depends on: Task 2
"#;
        let doc = parse_plan(plan);
        assert_eq!(doc.tasks[0].id, "T1");
        assert_eq!(doc.tasks[1].id, "T2");
        assert_eq!(doc.tasks[2].deps, ["T2"]);
    }

    #[test]
    fn test_8_non_plan_task_dropped_unless_written_as_task_x() {
        let plan = r#"# Test Plan
### Task A: first
**Depends on:** X9
### Task B: second
Depends on: Task X9
"#;
        let doc = parse_plan(plan);
        assert!(doc.tasks[0].deps.is_empty());
        assert_eq!(doc.tasks[1].deps, ["X9"]);
    }

    #[test]
    fn test_9_real_plan() {
        let doc = parse_plan(include_str!(
            "../../../docs/superpowers/plans/2026-10-07-orchestrate-mode.md"
        ));
        let find = |id: &str| {
            doc.tasks
                .iter()
                .find(|t| t.id == id)
                .unwrap_or_else(|| panic!("task {id} not found in plan"))
                .deps
                .clone()
        };
        assert_eq!(find("OC5"), ["OC2", "OC3", "OC4"]);
        assert!(find("OC6").is_empty());
        assert_eq!(find("U2"), ["OC9"]);
        assert_eq!(find("S2"), ["S1"]);
        assert_eq!(find("U3"), ["U1", "U2"]);

        let plan_ids: HashSet<&str> = doc.tasks.iter().map(|t| t.id.as_str()).collect();
        for task in &doc.tasks {
            for dep in &task.deps {
                assert!(
                    plan_ids.contains(dep.as_str()),
                    "task {} has dependency {} which is not a task id in the plan",
                    task.id,
                    dep
                );
            }
        }
    }
}
