//! `blyte init`: extracts the gate from CI workflows, detects environment settings,
//! and proves each step in a fresh worktree before writing `blyte.toml`.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::cli::InitArgs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Passed,
    Environment(String),
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitStep {
    pub command: String,
    pub source: String,
    pub verdict: Option<Verdict>,
}

/// Detect check commands from `.github/workflows/*.yml` and `*.yaml`.
pub fn detect_ci(root: &Path) -> Vec<String> {
    drop_overlapping(
        detect_ci_with_sources(root)
            .into_iter()
            .map(|(cmd, _)| cmd)
            .collect(),
    )
}

/// Detect check commands and keep the file name of the workflow where each was first found.
pub fn detect_ci_with_sources(root: &Path) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for (file_name, content) in read_workflow_files(root) {
        let (cmds, _, _) = parse_workflow(&content);
        for cmd in cmds {
            if seen.insert(cmd.clone()) {
                result.push((cmd, file_name.clone()));
            }
        }
    }
    result
}

/// Detect per-directory `uv sync` setup commands from `.github/workflows/*.yml` and `*.yaml`.
pub fn detect_ci_setup(root: &Path) -> Vec<String> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for (_, content) in read_workflow_files(root) {
        let (_, setup_cmds, _) = parse_workflow(&content);
        for cmd in setup_cmds {
            if seen.insert(cmd.clone()) {
                result.push(cmd);
            }
        }
    }
    result
}

/// Detect check commands that were left out because they depend on CI expressions or unknown matrix keys.
pub fn detect_ci_left_out(root: &Path) -> Vec<(String, String, String)> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for (file_name, content) in read_workflow_files(root) {
        let (_, _, left_out) = parse_workflow(&content);
        for (cmd, reason) in left_out {
            if seen.insert(cmd.clone()) {
                result.push((cmd, file_name.clone(), reason));
            }
        }
    }
    result
}

fn read_workflow_files(root: &Path) -> Vec<(String, String)> {
    let workflows_dir = root.join(".github").join("workflows");
    if !workflows_dir.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&workflows_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext == "yml" || ext == "yaml" {
                        files.push(path);
                    }
                }
            }
        }
    }
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    let mut out = Vec::new();
    for file in files {
        let file_name = file
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or_default()
            .to_string();
        if let Ok(content) = std::fs::read_to_string(&file) {
            out.push((file_name, content));
        }
    }
    out
}

#[derive(Debug)]
struct PytestInfo {
    prefix: String,
    runner: String,
    positional: Vec<String>,
    narrowness: usize,
}

fn parse_pytest_step(cmd: &str) -> Option<PytestInfo> {
    let trimmed = cmd.trim();
    let prefixes = ["uv run ", "poetry run ", "pdm run ", "pipenv run "];
    let (prefix, after_prefix) = if let Some(p) = prefixes.iter().find(|&&p| trimmed.starts_with(p))
    {
        (*p, &trimmed[p.len()..])
    } else {
        ("", trimmed)
    };

    let runners = ["python3 -m pytest", "python -m pytest", "pytest"];
    let (runner, rest) = runners.iter().find_map(|&r| {
        if after_prefix == r {
            Some((r, ""))
        } else if let Some(rest) = after_prefix.strip_prefix(r) {
            if rest.starts_with(char::is_whitespace) {
                Some((r, rest.trim_start()))
            } else {
                None
            }
        } else {
            None
        }
    })?;

    let tokens = tokenize(rest);
    let mut positional = Vec::new();
    let mut narrowness = 0;
    let mut i = 0;

    while i < tokens.len() {
        let tok = &tokens[i];
        if tok == "--" {
            i += 1;
            while i < tokens.len() {
                positional.push(tokens[i].clone());
                i += 1;
            }
            break;
        }

        if tok == "-m" || tok == "-k" {
            narrowness += 1;
            i += 1;
            if i < tokens.len() {
                narrowness += tokens[i].matches(" and ").count();
                i += 1;
            }
        } else if tok.starts_with("-m=") || tok.starts_with("-k=") {
            narrowness += 1;
            let val = &tok[3..];
            narrowness += val.matches(" and ").count();
            i += 1;
        } else if tok == "--ignore" || tok == "--deselect" {
            narrowness += 1;
            i += 1;
            if i < tokens.len() {
                i += 1;
            }
        } else if tok.starts_with("--ignore=") || tok.starts_with("--deselect=") {
            narrowness += 1;
            i += 1;
        } else if tok == "-p" {
            i += 1;
            if i < tokens.len() {
                i += 1;
            }
        } else if tok.starts_with('-') {
            i += 1;
        } else {
            positional.push(tok.clone());
            i += 1;
        }
    }

    Some(PytestInfo {
        prefix: prefix.to_string(),
        runner: runner.to_string(),
        positional,
        narrowness,
    })
}

/// Drop overlapping pytest steps, keeping only the broader of two that test the same files.
pub fn drop_overlapping(steps: Vec<String>) -> Vec<String> {
    let n = steps.len();
    if n <= 1 {
        return steps;
    }

    let parsed: Vec<Option<PytestInfo>> = steps.iter().map(|s| parse_pytest_step(s)).collect();

    let mut adj = vec![Vec::new(); n];
    for i in 0..n {
        let Some(ref a) = parsed[i] else { continue };
        for j in (i + 1)..n {
            let Some(ref b) = parsed[j] else { continue };
            if a.prefix == b.prefix
                && a.runner == b.runner
                && (a.positional == b.positional
                    || a.positional.is_empty()
                    || b.positional.is_empty())
            {
                adj[i].push(j);
                adj[j].push(i);
            }
        }
    }

    let mut visited = vec![false; n];
    let mut result_slots: Vec<Option<String>> = vec![None; n];

    for i in 0..n {
        if visited[i] {
            continue;
        }
        visited[i] = true;
        let mut comp = vec![i];
        let mut q = VecDeque::new();
        q.push_back(i);
        while let Some(u) = q.pop_front() {
            for &v in &adj[u] {
                if !visited[v] {
                    visited[v] = true;
                    comp.push(v);
                    q.push_back(v);
                }
            }
        }

        if comp.len() == 1 {
            result_slots[i] = Some(steps[i].clone());
        } else {
            let first_pos = *comp.iter().min().unwrap();
            let best = *comp
                .iter()
                .min_by(|&&a, &&b| {
                    let na = parsed[a].as_ref().unwrap().narrowness;
                    let nb = parsed[b].as_ref().unwrap().narrowness;
                    na.cmp(&nb).then_with(|| a.cmp(&b))
                })
                .unwrap();
            result_slots[first_pos] = Some(steps[best].clone());
        }
    }

    result_slots.into_iter().flatten().collect()
}

#[derive(Default)]
struct JobContext {
    matrix: Vec<(String, Vec<String>)>,
    working_dir: Option<String>,
    env: BTreeMap<String, String>,
}

#[derive(Default)]
struct StepCandidate {
    job_id: usize,
    working_dir: Option<String>,
    env: BTreeMap<String, String>,
    commands: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnvScope {
    Workflow,
    Job,
    Step,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DefaultsScope {
    Workflow,
    Job,
}

fn parse_workflow(content: &str) -> (Vec<String>, Vec<String>, Vec<(String, String)>) {
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    let mut in_jobs = false;
    let mut job_indent: Option<usize> = None;
    let mut job_sub_indent: Option<usize> = None;
    let mut current_job: usize = 0;
    let mut jobs: Vec<JobContext> = vec![JobContext::default()];
    let mut workflow_working_dir: Option<String> = None;
    let mut workflow_env: BTreeMap<String, String> = BTreeMap::new();

    let mut matrix_indent: Option<usize> = None;
    let mut matrix_key_indent: Option<usize> = None;
    let mut current_block_key: Option<String> = None;

    let mut steps_indent: Option<usize> = None;
    let mut current_step_indent: Option<usize> = None;
    let mut current_step: Option<StepCandidate> = None;
    let mut candidates: Vec<StepCandidate> = Vec::new();

    let mut env_ctx: Option<(usize, EnvScope)> = None;
    let mut defaults_ctx: Option<(usize, DefaultsScope, Option<usize>)> = None;

    while i < lines.len() {
        let line = lines[i];
        let trimmed_start = line.trim_start();
        if trimmed_start.is_empty() || trimmed_start.starts_with('#') {
            i += 1;
            continue;
        }
        let indent = line.chars().take_while(|c| c.is_whitespace()).count();
        let clean_line = strip_yaml_comment(trimmed_start);

        if let Some((e_indent, _)) = env_ctx {
            if indent <= e_indent {
                env_ctx = None;
            }
        }
        if let Some((d_indent, scope, r_indent)) = defaults_ctx {
            if indent <= d_indent {
                defaults_ctx = None;
            } else if let Some(ri) = r_indent {
                if indent <= ri {
                    defaults_ctx = Some((d_indent, scope, None));
                }
            }
        }
        if let Some(s_indent) = steps_indent {
            if indent < s_indent || (indent == s_indent && !clean_line.starts_with('-')) {
                if let Some(step) = current_step.take() {
                    candidates.push(step);
                }
                current_step_indent = None;
                steps_indent = None;
            }
        }
        if let Some(cs_indent) = current_step_indent {
            if indent < cs_indent || (indent == cs_indent && clean_line.starts_with('-')) {
                if let Some(step) = current_step.take() {
                    candidates.push(step);
                }
                current_step_indent = None;
            }
        }

        if indent == 0 {
            matrix_indent = None;
            matrix_key_indent = None;
            current_block_key = None;
            if clean_line == "jobs:" {
                in_jobs = true;
                job_indent = None;
                job_sub_indent = None;
                i += 1;
                continue;
            } else {
                in_jobs = false;
                job_indent = None;
                job_sub_indent = None;
                if let Some(rest) = clean_line.strip_prefix("env:") {
                    let rest = rest.trim();
                    if rest.is_empty() {
                        env_ctx = Some((indent, EnvScope::Workflow));
                    } else if rest.starts_with('{') && rest.ends_with('}') {
                        parse_flow_map(&rest[1..rest.len() - 1], &mut workflow_env);
                    }
                    i += 1;
                    continue;
                } else if clean_line == "defaults:" {
                    defaults_ctx = Some((indent, DefaultsScope::Workflow, None));
                    i += 1;
                    continue;
                }
            }
        } else if in_jobs {
            if job_indent.is_none() && !clean_line.starts_with('-') && clean_line.contains(':') {
                job_indent = Some(indent);
            }
            if Some(indent) == job_indent
                && !clean_line.starts_with('-')
                && clean_line.contains(':')
            {
                if let Some(step) = current_step.take() {
                    candidates.push(step);
                }
                current_step_indent = None;
                steps_indent = None;
                job_sub_indent = None;
                current_job += 1;
                jobs.push(JobContext::default());
                matrix_indent = None;
                matrix_key_indent = None;
                current_block_key = None;
                i += 1;
                continue;
            }
            if let Some(j_indent) = job_indent {
                if indent > j_indent
                    && job_sub_indent.is_none()
                    && !clean_line.starts_with('-')
                    && clean_line.contains(':')
                {
                    job_sub_indent = Some(indent);
                }
            }
        }

        if let Some((d_indent, scope, r_indent)) = defaults_ctx {
            if indent > d_indent {
                if r_indent.is_none() && clean_line == "run:" {
                    defaults_ctx = Some((d_indent, scope, Some(indent)));
                } else if let Some(ri) = r_indent {
                    if indent > ri {
                        if let Some((k, v)) = clean_line.split_once(':') {
                            if k.trim() == "working-directory" {
                                let wd = unquote(v.trim()).to_string();
                                match scope {
                                    DefaultsScope::Workflow => workflow_working_dir = Some(wd),
                                    DefaultsScope::Job => jobs[current_job].working_dir = Some(wd),
                                }
                            }
                        }
                    }
                }
                i += 1;
                continue;
            }
        }

        if let Some((e_indent, scope)) = env_ctx {
            if indent > e_indent {
                if !clean_line.starts_with('-') {
                    if let Some((k, v)) = clean_line.split_once(':') {
                        let key = k.trim();
                        let val = unquote(v.trim()).to_string();
                        if !key.is_empty() {
                            match scope {
                                EnvScope::Workflow => {
                                    workflow_env.insert(key.to_string(), val);
                                }
                                EnvScope::Job => {
                                    jobs[current_job].env.insert(key.to_string(), val);
                                }
                                EnvScope::Step => {
                                    if let Some(ref mut step) = current_step {
                                        step.env.insert(key.to_string(), val);
                                    }
                                }
                            }
                        }
                    }
                }
                i += 1;
                continue;
            }
        }

        if let Some(m_indent) = matrix_indent {
            if indent <= m_indent {
                matrix_indent = None;
                matrix_key_indent = None;
                current_block_key = None;
            } else {
                if matrix_key_indent.is_none()
                    && !clean_line.starts_with('-')
                    && clean_line.contains(':')
                {
                    matrix_key_indent = Some(indent);
                }
                if Some(indent) == matrix_key_indent && !clean_line.starts_with('-') {
                    current_block_key = None;
                    if let Some((raw_key, raw_val)) = clean_line.split_once(':') {
                        let key = raw_key.trim();
                        let val = raw_val.trim();
                        if key != "include" && key != "exclude" && !key.is_empty() {
                            if val.starts_with('[') && val.ends_with(']') {
                                let items = parse_flow_list(&val[1..val.len() - 1]);
                                set_matrix_key(&mut jobs[current_job].matrix, key, items);
                            } else if val.is_empty() {
                                set_matrix_key(&mut jobs[current_job].matrix, key, Vec::new());
                                current_block_key = Some(key.to_string());
                            } else {
                                set_matrix_key(&mut jobs[current_job].matrix, key, Vec::new());
                            }
                        }
                    }
                    i += 1;
                    continue;
                } else if let Some(k_indent) = matrix_key_indent {
                    if indent >= k_indent {
                        if let Some(ref key) = current_block_key {
                            if let Some(item_str) = clean_line.strip_prefix('-') {
                                let item = item_str.trim();
                                if !item.is_empty() && !is_yaml_mapping_or_expr(item) {
                                    let val = unquote(item).to_string();
                                    push_matrix_val(&mut jobs[current_job].matrix, key, val);
                                } else {
                                    set_matrix_key(&mut jobs[current_job].matrix, key, Vec::new());
                                    current_block_key = None;
                                }
                            } else {
                                set_matrix_key(&mut jobs[current_job].matrix, key, Vec::new());
                                current_block_key = None;
                            }
                        }
                        i += 1;
                        continue;
                    }
                }
            }
        }

        if in_jobs && Some(indent) == job_sub_indent {
            if let Some(rest) = clean_line.strip_prefix("env:") {
                let rest = rest.trim();
                if rest.is_empty() {
                    env_ctx = Some((indent, EnvScope::Job));
                } else if rest.starts_with('{') && rest.ends_with('}') {
                    parse_flow_map(&rest[1..rest.len() - 1], &mut jobs[current_job].env);
                }
                i += 1;
                continue;
            } else if clean_line == "defaults:" {
                defaults_ctx = Some((indent, DefaultsScope::Job, None));
                i += 1;
                continue;
            } else if clean_line == "steps:" {
                steps_indent = Some(indent);
                i += 1;
                continue;
            }
        }

        if matrix_indent.is_none() && in_jobs && clean_line == "matrix:" {
            matrix_indent = Some(indent);
            matrix_key_indent = None;
            current_block_key = None;
            i += 1;
            continue;
        }

        if steps_indent.is_some() && clean_line.starts_with('-') {
            let after_dash = clean_line[1..].trim_start();
            let dash_offset = clean_line.len() - after_dash.len();
            let step_key_indent = indent + dash_offset;
            current_step_indent = Some(step_key_indent);
            current_step = Some(StepCandidate {
                job_id: current_job,
                ..Default::default()
            });
        }

        let step_clean = if Some(indent) == current_step_indent {
            Some(clean_line)
        } else if current_step_indent.is_some() && clean_line.starts_with('-') {
            let after_dash = clean_line[1..].trim_start();
            Some(after_dash)
        } else {
            None
        };

        if let Some(sc) = step_clean {
            if let Some(rest) = sc.strip_prefix("working-directory:") {
                if let Some(ref mut step) = current_step {
                    step.working_dir = Some(unquote(rest.trim()).to_string());
                }
                i += 1;
                continue;
            } else if let Some(rest) = sc.strip_prefix("env:") {
                let rest = rest.trim();
                let key_indent = current_step_indent.unwrap_or(indent);
                if rest.is_empty() {
                    env_ctx = Some((key_indent, EnvScope::Step));
                } else if rest.starts_with('{') && rest.ends_with('}') {
                    if let Some(ref mut step) = current_step {
                        parse_flow_map(&rest[1..rest.len() - 1], &mut step.env);
                    }
                }
                i += 1;
                continue;
            }
        }

        let is_run_key = if let Some(rest) = trimmed_start.strip_prefix("- ") {
            rest.trim_start().starts_with("run:")
        } else {
            trimmed_start.starts_with("run:")
        };

        // The pre-commit action is a `uses:` step, not a `run:`; its default is every file.
        let uses = trimmed_start
            .strip_prefix("- ")
            .unwrap_or(trimmed_start)
            .trim_start();
        if uses.starts_with("uses: pre-commit/action@") {
            let cmd = "pre-commit run --all-files".to_string();
            if let Some(ref mut step) = current_step {
                step.commands.push(cmd);
            } else {
                candidates.push(StepCandidate {
                    job_id: current_job,
                    commands: vec![cmd],
                    ..Default::default()
                });
            }
        }

        if is_run_key {
            let run_idx = line.find("run:").unwrap();
            let after_run = line[run_idx + 4..].trim();
            let after_run_clean = if let Some(hash) = after_run.find('#') {
                after_run[..hash].trim()
            } else {
                after_run
            };

            if after_run_clean.starts_with('|') || after_run_clean.starts_with('>') {
                let mut block_lines = Vec::new();
                i += 1;
                while i < lines.len() {
                    let next_line = lines[i];
                    let next_trimmed = next_line.trim();
                    if next_trimmed.is_empty() {
                        block_lines.push(next_line);
                        i += 1;
                        continue;
                    }
                    let leading_spaces =
                        next_line.chars().take_while(|c| c.is_whitespace()).count();
                    if leading_spaces <= run_idx {
                        break;
                    }
                    block_lines.push(next_line);
                    i += 1;
                }

                let joined = join_continuation_lines(&block_lines);
                if let Some(ref mut step) = current_step {
                    step.commands.extend(joined);
                } else {
                    candidates.push(StepCandidate {
                        job_id: current_job,
                        commands: joined,
                        ..Default::default()
                    });
                }
                continue;
            } else if !after_run_clean.is_empty() {
                let cmd_str = unquote(after_run_clean).to_string();
                if let Some(ref mut step) = current_step {
                    step.commands.push(cmd_str);
                } else {
                    candidates.push(StepCandidate {
                        job_id: current_job,
                        commands: vec![cmd_str],
                        ..Default::default()
                    });
                }
            }
        }
        i += 1;
    }

    if let Some(step) = current_step.take() {
        candidates.push(step);
    }

    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut setup_cmds = Vec::new();
    let mut seen_setup = HashSet::new();
    let mut left_out = Vec::new();
    let mut seen_left_out = HashSet::new();

    for cand in candidates {
        let job = &jobs[cand.job_id];
        let matrix = &job.matrix;

        let effective_wd = cand
            .working_dir
            .as_ref()
            .or(job.working_dir.as_ref())
            .or(workflow_working_dir.as_ref())
            .cloned();

        let mut effective_env = workflow_env.clone();
        for (k, v) in &job.env {
            effective_env.insert(k.clone(), v.clone());
        }
        for (k, v) in &cand.env {
            effective_env.insert(k.clone(), v.clone());
        }

        for raw_cmd in &cand.commands {
            let combos =
                expand_step_combinations(raw_cmd, effective_wd.as_deref(), &effective_env, matrix);
            for (exp_cmd, exp_wd, exp_env) in combos {
                for part in split_chain(&exp_cmd) {
                    if let Some(uv_sync) = filter_uv_sync_command(&part) {
                        if is_non_root_wd(exp_wd.as_deref()) {
                            if let Some(reason) =
                                check_left_out_reason(&uv_sync, exp_wd.as_deref(), &exp_env, matrix)
                            {
                                if seen_left_out.insert(uv_sync.clone()) {
                                    left_out.push((uv_sync, reason));
                                }
                            } else {
                                let rendered = format!(
                                    "({})",
                                    render_step_command(&uv_sync, exp_wd.as_deref(), &exp_env)
                                );
                                if seen_setup.insert(rendered.clone()) {
                                    setup_cmds.push(rendered);
                                }
                            }
                        }
                        continue;
                    }
                    let Some(kept) = filter_and_clean_command(&part) else {
                        continue;
                    };
                    if let Some(reason) =
                        check_left_out_reason(&kept, exp_wd.as_deref(), &exp_env, matrix)
                    {
                        if seen_left_out.insert(kept.clone()) {
                            left_out.push((kept, reason));
                        }
                    } else {
                        let rendered = render_step_command(&kept, exp_wd.as_deref(), &exp_env);
                        if seen.insert(rendered.clone()) {
                            result.push(rendered);
                        }
                    }
                }
            }
        }
    }

    (result, setup_cmds, left_out)
}

fn strip_yaml_comment(s: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut prev_ws = true;
    for (idx, c) in s.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '#' && prev_ws {
            return s[..idx].trim_end();
        }
        prev_ws = c.is_whitespace();
    }
    s.trim_end()
}

fn unquote(s: &str) -> &str {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn is_yaml_mapping_or_expr(s: &str) -> bool {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        return false;
    }
    s.starts_with('{')
        || s.starts_with('[')
        || s.starts_with("${{")
        || s.ends_with(':')
        || s.contains(": ")
}

fn parse_flow_list(inner: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in inner.chars() {
        if let Some(q) = quote {
            current.push(c);
            if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
            current.push(c);
        } else if c == ',' {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                if is_yaml_mapping_or_expr(trimmed) {
                    return Vec::new();
                }
                items.push(unquote(trimmed).to_string());
            }
            current.clear();
        } else {
            current.push(c);
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        if is_yaml_mapping_or_expr(trimmed) {
            return Vec::new();
        }
        items.push(unquote(trimmed).to_string());
    }
    items
}

fn parse_flow_map(inner: &str, out: &mut BTreeMap<String, String>) {
    let mut entries = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut brace_depth: usize = 0;
    for c in inner.chars() {
        if let Some(q) = quote {
            current.push(c);
            if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
            current.push(c);
        } else if c == '{' {
            brace_depth += 1;
            current.push(c);
        } else if c == '}' {
            brace_depth = brace_depth.saturating_sub(1);
            current.push(c);
        } else if c == ',' && brace_depth == 0 {
            entries.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    entries.push(current);
    for entry in entries {
        let trimmed = entry.trim();
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim();
            let val = unquote(v.trim()).to_string();
            if !key.is_empty() {
                out.insert(key.to_string(), val);
            }
        }
    }
}

fn set_matrix_key(matrix: &mut Vec<(String, Vec<String>)>, key: &str, values: Vec<String>) {
    if let Some(entry) = matrix.iter_mut().find(|(k, _)| k == key) {
        entry.1 = values;
    } else {
        matrix.push((key.to_string(), values));
    }
}

fn push_matrix_val(matrix: &mut Vec<(String, Vec<String>)>, key: &str, val: String) {
    if let Some(entry) = matrix.iter_mut().find(|(k, _)| k == key) {
        entry.1.push(val);
    } else {
        matrix.push((key.to_string(), vec![val]));
    }
}

fn expand_step_combinations(
    cmd: &str,
    wd: Option<&str>,
    env: &BTreeMap<String, String>,
    matrix: &[(String, Vec<String>)],
) -> Vec<(String, Option<String>, BTreeMap<String, String>)> {
    let mut expanded = vec![(cmd.to_string(), wd.map(String::from), env.clone())];
    for (key, values) in matrix {
        if values.is_empty() {
            continue;
        }
        let mut next = Vec::new();
        for (c, w, e) in &expanded {
            let uses_key = replace_matrix_key(c, key, "").is_some()
                || w.as_deref()
                    .is_some_and(|ws| replace_matrix_key(ws, key, "").is_some())
                || e.values()
                    .any(|ev| replace_matrix_key(ev, key, "").is_some());
            if uses_key {
                for v in values {
                    let nc = replace_matrix_key(c, key, v).unwrap_or_else(|| c.clone());
                    let nw = w
                        .as_ref()
                        .map(|ws| replace_matrix_key(ws, key, v).unwrap_or_else(|| ws.clone()));
                    let ne = e
                        .iter()
                        .map(|(ek, ev)| {
                            (
                                ek.clone(),
                                replace_matrix_key(ev, key, v).unwrap_or_else(|| ev.clone()),
                            )
                        })
                        .collect();
                    next.push((nc, nw, ne));
                }
            } else {
                next.push((c.clone(), w.clone(), e.clone()));
            }
        }
        expanded = next;
    }
    expanded
}

fn replace_matrix_key(s: &str, target_key: &str, val: &str) -> Option<String> {
    let mut out = String::new();
    let mut cursor = 0;
    let mut replaced = false;
    while cursor < s.len() {
        let rest = &s[cursor..];
        let Some(rel_start) = rest.find("${{") else {
            out.push_str(rest);
            break;
        };
        let start = cursor + rel_start;
        let after_open = &s[start + 3..];
        let Some(rel_end) = after_open.find("}}") else {
            out.push_str(rest);
            break;
        };
        let end = start + 3 + rel_end + 2;
        let inner = after_open[..rel_end].trim();
        if inner.strip_prefix("matrix.") == Some(target_key) {
            out.push_str(&s[cursor..start]);
            out.push_str(val);
            replaced = true;
            cursor = end;
        } else {
            out.push_str(&s[cursor..start + 3]);
            cursor = start + 3;
        }
    }
    if replaced {
        Some(out)
    } else {
        None
    }
}

fn first_unexpanded_matrix_key(s: &str) -> Option<String> {
    let mut cursor = 0;
    while let Some(rel) = s[cursor..].find("${{") {
        let start = cursor + rel + 3;
        let after = &s[start..];
        if let Some(rel_end) = after.find("}}") {
            let inner = after[..rel_end].trim();
            if let Some(key) = inner.strip_prefix("matrix.") {
                return Some(key.trim().to_string());
            }
            cursor = start + rel_end + 2;
        } else {
            let inner = after.trim_start();
            if let Some(rest) = inner.strip_prefix("matrix.") {
                let key = rest.split_whitespace().next().unwrap_or("");
                return Some(key.to_string());
            }
            break;
        }
    }
    None
}

fn check_left_out_reason(
    cmd: &str,
    wd: Option<&str>,
    env: &BTreeMap<String, String>,
    matrix: &[(String, Vec<String>)],
) -> Option<String> {
    if let Some(key) = first_unexpanded_matrix_key(cmd) {
        if matrix.iter().any(|(k, _)| k == &key) {
            return Some(format!("matrix key {key} has no plain values"));
        } else {
            return Some(format!("unknown matrix key {key}"));
        }
    }
    for (k, v) in env {
        if v.contains("${{") {
            return Some(format!("env {k} is set from a CI expression"));
        }
    }
    if let Some(w) = wd {
        if w.contains("${{") {
            return Some("working-directory is a CI expression".to_string());
        }
    }
    None
}

fn shell_quote_if_needed(s: &str) -> String {
    let is_safe = !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '.' | '/' | ',' | ':' | '@' | '%' | '+' | '=' | '-')
        });
    if is_safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn is_non_root_wd(wd: Option<&str>) -> bool {
    wd.is_some_and(|dir| !dir.is_empty() && dir != "." && dir != "./")
}

fn filter_uv_sync_command(cmd: &str) -> Option<String> {
    let tokens = tokenize(cmd.trim());
    if tokens.first().map(String::as_str) == Some("uv")
        && tokens.get(1).map(String::as_str) == Some("sync")
    {
        Some(tokens.join(" "))
    } else {
        None
    }
}

fn render_step_command(cmd: &str, wd: Option<&str>, env: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    if let Some(dir) = wd {
        if is_non_root_wd(Some(dir)) {
            out.push_str("cd ");
            out.push_str(&shell_quote_if_needed(dir));
            out.push_str(" && ");
        }
    }
    if !env.is_empty() {
        out.push_str("env ");
        for (k, v) in env {
            out.push_str(k);
            out.push('=');
            out.push_str(&shell_quote_if_needed(v));
            out.push(' ');
        }
    }
    out.push_str(cmd);
    out
}

/// Split a shell line on `&&`, `||` and `;` outside quotes.
fn split_chain(cmd: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            current.push(c);
            if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
            current.push(c);
        } else if (c == '&' || c == '|') && chars.get(i + 1) == Some(&c) {
            parts.push(std::mem::take(&mut current));
            i += 1;
        } else if c == ';' {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
        i += 1;
    }
    parts.push(current);
    parts
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

fn join_continuation_lines(lines: &[&str]) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !current.is_empty() {
                result.push(current);
                current = String::new();
            }
            continue;
        }

        if let Some(stripped) = trimmed.strip_suffix('\\') {
            let without_bs = stripped.trim_end();
            if current.is_empty() {
                current.push_str(without_bs);
            } else {
                current.push(' ');
                current.push_str(without_bs);
            }
        } else if current.is_empty() {
            result.push(trimmed.to_string());
        } else {
            current.push(' ');
            current.push_str(trimmed);
            result.push(current);
            current = String::new();
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

fn filter_and_clean_command(cmd: &str) -> Option<String> {
    let trimmed = cmd.trim();
    let prefixes = [
        "uv run ",
        "poetry run ",
        "pdm run ",
        "npx ",
        "pipenv run ",
        "pnpm exec ",
        "bunx ",
    ];
    let mut stripped = trimmed;
    let mut had_prefix = false;
    for p in &prefixes {
        if stripped.starts_with(p) {
            stripped = stripped[p.len()..].trim_start();
            had_prefix = true;
            break;
        }
    }
    if had_prefix {
        while stripped.starts_with('-') {
            let Some((flag, rest)) = stripped.split_once(char::is_whitespace) else {
                break;
            };
            stripped = rest.trim_start();
            if matches!(
                flag,
                "--python" | "-p" | "--package" | "--directory" | "--group" | "--extra" | "--with"
            ) {
                if let Some((_, after_val)) = stripped.split_once(char::is_whitespace) {
                    stripped = after_val.trim_start();
                } else {
                    break;
                }
            }
        }
    }

    let check_cmds = [
        "pytest",
        "python -m pytest",
        "python3 -m pytest",
        "python -m unittest",
        "python3 -m unittest",
        "tox",
        "nox",
        "ruff",
        "pyright",
        "mypy",
        "black",
        "isort",
        "pre-commit run",
        "cargo fmt",
        "cargo clippy",
        "cargo test",
        "go test",
        "go vet",
        "golangci-lint",
        "npm test",
        "npm run lint",
        "npm run typecheck",
        "pnpm test",
        "pnpm run lint",
        "yarn test",
        "yarn lint",
        "make check",
        "make lint",
        "make test",
        "make typecheck",
        "make validate",
        "eslint",
        "vitest",
        "jest",
        "tsc",
    ];

    let text = if let Some(idx) = trimmed.find("| tee") {
        &trimmed[..idx]
    } else {
        trimmed
    };

    let is_check = check_cmds.iter().any(|&c| {
        if stripped == c {
            true
        } else if let Some(rest) = stripped.strip_prefix(c) {
            rest.starts_with(char::is_whitespace)
        } else {
            false
        }
    }) || is_make_check(text);

    if !is_check {
        return None;
    }

    // Reporting options go, with their value when it is a separate word (`--cov-report xml:out`).
    let tokens = tokenize(text);
    let mut kept_tokens: Vec<String> = Vec::new();
    let mut iter = tokens.into_iter().peekable();
    while let Some(tok) = iter.next() {
        let reporting = ["--cov", "--junitxml", "--junit-xml"]
            .iter()
            .any(|o| tok.starts_with(o));
        if !reporting {
            kept_tokens.push(tok);
            continue;
        }
        // These always take a value; a bare `--cov` takes one unless it is the last word.
        let value_follows = iter.peek().is_some_and(|n| !n.starts_with('-'));
        let always = matches!(tok.as_str(), "--cov-report" | "--junitxml" | "--junit-xml");
        if value_follows && (always || (tok == "--cov" && iter.len() > 1)) {
            iter.next();
        }
    }

    Some(kept_tokens.join(" "))
}

fn is_make_check(cmd: &str) -> bool {
    let tokens = tokenize(cmd);
    if tokens.first().map(String::as_str) != Some("make") {
        return false;
    }
    let mut targets = Vec::new();
    let mut i = 1;
    while i < tokens.len() {
        let tok = tokens[i].as_str();
        if tok == "-C" || tok == "--directory" {
            if i + 1 >= tokens.len() || tokens[i + 1].is_empty() {
                return false;
            }
            i += 2;
        } else if (tok.starts_with("-C") && tok.len() > 2)
            || (tok.starts_with("--directory=") && tok.len() > "--directory=".len())
            || matches!(tok, "-s" | "--no-print-directory" | "-k")
        {
            i += 1;
        } else if tok.starts_with('-') || tok.contains('=') {
            return false;
        } else {
            targets.push(tok);
            i += 1;
        }
    }
    if targets.len() != 1 {
        return false;
    }
    let target = targets[0];
    matches!(target, "check" | "lint" | "test" | "typecheck" | "validate")
        || target.starts_with("test-")
        || target.starts_with("test_")
}

fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = None;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        match in_quote {
            Some(q) => {
                current.push(ch);
                if ch == q {
                    in_quote = None;
                }
                i += 1;
            }
            None => {
                if ch == '$' && chars.get(i + 1) == Some(&'{') && chars.get(i + 2) == Some(&'{') {
                    current.push_str("${{");
                    i += 3;
                    while i < chars.len() {
                        if chars[i] == '}' && chars.get(i + 1) == Some(&'}') {
                            current.push_str("}}");
                            i += 2;
                            break;
                        }
                        current.push(chars[i]);
                        i += 1;
                    }
                } else if ch == '"' || ch == '\'' {
                    in_quote = Some(ch);
                    current.push(ch);
                    i += 1;
                } else if ch.is_whitespace() {
                    if !current.is_empty() {
                        tokens.push(current);
                        current = String::new();
                    }
                    i += 1;
                } else {
                    current.push(ch);
                    i += 1;
                }
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Detect repository environment settings (such as UV_FROZEN and UV_OFFLINE).
pub fn detect_env(root: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if root.join("uv.lock").exists() {
        map.insert("UV_FROZEN".to_string(), "1".to_string());
        if let Ok(content) = std::fs::read_to_string(root.join("pyproject.toml")) {
            if let Ok(parsed) = content.parse::<toml::Value>() {
                if let Some(tool) = parsed.get("tool").and_then(|t| t.get("uv")) {
                    if let Some(indexes) = tool.get("index").and_then(|i| i.as_array()) {
                        for idx in indexes {
                            if let Some(url) = idx.get("url").and_then(|u| u.as_str()) {
                                if !url.contains("pypi.org") {
                                    map.insert("UV_OFFLINE".to_string(), "1".to_string());
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    map
}

/// Propose a worktree setup command when the repository uses a supported package manager.
pub fn detect_setup(root: &Path) -> Option<String> {
    if root.join("uv.lock").exists() {
        Some("uv sync".to_string())
    } else {
        None
    }
}

/// Classify step output into Passed, Environment problem, or Failed.
pub fn classify(output: &str, exit_code: i32) -> Verdict {
    if exit_code == 0 {
        return Verdict::Passed;
    }

    let uv_patterns = [
        "network connectivity is disabled",
        "wasn't found in the cache",
    ];

    // Words, not bare status codes: a test named `test_403_…` is not a 403.
    let patterns = [
        "unauthorized",
        "forbidden",
        "not found in the package registry",
        "could not resolve host",
        "network is unreachable",
        "connection refused",
        "operation not permitted",
        "command not found",
        "no such file or directory",
    ];

    for line in output.lines() {
        // Test ids and result lines (`tests/x.py::test_y PASSED`) name tests, not the environment.
        if line.contains("::") {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        let is_uv = uv_patterns.iter().any(|&p| lower.contains(p));
        if is_uv || patterns.iter().any(|&p| lower.contains(p)) {
            let trimmed = line.trim();
            let mut s: String = if trimmed.chars().count() > 120 {
                trimmed.chars().take(120).collect()
            } else {
                trimmed.to_string()
            };
            if is_uv {
                s.push_str(
                    " (run `uv sync` once online to fill uv's cache, then blyte init again)",
                );
            }
            return Verdict::Environment(s);
        }
    }

    Verdict::Failed
}

/// Render the verified blyte.toml content.
pub fn render_verified(
    steps: &[InitStep],
    env: &BTreeMap<String, String>,
    setup: Option<&str>,
    setup_passed: bool,
) -> String {
    let mut out = String::new();
    out.push_str("# blyte.toml: how this repository's work is checked, for `blyte dispatch`.\n");

    let mut kept_steps = Vec::new();
    for step in steps {
        let comment_suffix = match &step.verdict {
            Some(Verdict::Passed) => "passed".to_string(),
            Some(Verdict::Failed) => "failed on HEAD: fix it before dispatching".to_string(),
            Some(Verdict::Environment(reason)) => format!("left out: {reason}"),
            None => "not verified".to_string(),
        };
        out.push_str(&format!(
            "# {}: from {}, {}\n",
            step.command, step.source, comment_suffix
        ));

        if !matches!(step.verdict, Some(Verdict::Environment(_))) {
            kept_steps.push(step.command.as_str());
        }
    }

    let mut gate_str = String::new();
    for step in &kept_steps {
        gate_str.push_str(step);
        gate_str.push('\n');
    }

    out.push_str("[dispatch]\n");
    if let Some(s) = setup {
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        if setup_passed {
            out.push_str("# prepares each new worktree before the implementer starts\n");
            out.push_str(&format!("setup = \"{escaped}\"\n"));
        } else {
            out.push_str(&format!(
                "# setup = \"{escaped}\"   # left out: failed in a fresh worktree\n"
            ));
        }
    }
    out.push_str("gate = \"\"\"\n");
    out.push_str(&gate_str);
    out.push_str("\"\"\"\n");
    out.push_str("# model = \"gemini-3.8-flash-high\"\n");
    out.push_str("max_seconds = 3600\n");
    out.push_str("max_turns = 300\n");
    out.push_str("# max_cost = 2.0\n");
    out.push_str("# network = \"on\"\n");
    out.push_str("# fold_budget = 60000\n\n");

    if !env.is_empty() {
        out.push_str("[dispatch.env]\n");
        out.push_str("# for every command and the gate; never credentials\n");
        for (k, v) in env {
            out.push_str(&format!("{k} = \"{v}\"\n"));
        }
    } else {
        out.push_str("# [dispatch.env]\n");
        out.push_str(
            "# UV_FROZEN = \"1\"   # settings for every command and the gate; never credentials\n",
        );
    }

    out
}

/// What to show of a failing step: the test runner's own summary lines when it printed any
/// (`FAILED …`, `ERROR …`, `== 2 failed, 900 passed ==`), else the last lines. A tail alone can be
/// all shutdown noise.
pub fn failure_excerpt(text: &str) -> Vec<&str> {
    let summary: Vec<&str> = text
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            // pytest's summary lines name a test file; a logger's `ERROR …` line does not.
            let names_test = t.contains("::") || t.contains(".py");
            ((t.starts_with("FAILED ") || t.starts_with("ERROR ")) && names_test)
                || (t.starts_with('=') && (t.contains(" failed") || t.contains(" error")))
        })
        .collect();
    let lines: Vec<&str> = if summary.is_empty() {
        text.lines().collect()
    } else {
        summary
    };
    lines[lines.len().saturating_sub(15)..].to_vec()
}

/// Remove `.wt/init-<pid>` worktrees left by an init that was killed (its process is gone).
fn remove_stale_init_worktrees(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root.join(".wt")) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(pid) = name
            .strip_prefix("init-")
            .and_then(|p| p.parse::<i32>().ok())
        else {
            continue;
        };
        if unsafe { libc::kill(pid, 0) } != 0 {
            let _ = Command::new("git")
                .args(["worktree", "remove", "--force"])
                .arg(entry.path())
                .current_dir(root)
                .output();
        }
    }
    let _ = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(root)
        .output();
}

fn verify_steps(
    root: &Path,
    mut steps: Vec<InitStep>,
    env: &BTreeMap<String, String>,
    sandbox_on: bool,
    timeout: Duration,
    setup: Option<&str>,
) -> (Vec<InitStep>, bool) {
    let common_dir_raw = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(root)
        .output();
    let common_dir = if let Ok(out) = common_dir_raw {
        let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if p.is_absolute() {
            p
        } else {
            root.join(p)
        }
    } else {
        root.join(".git")
    };
    let exclude_path = common_dir.join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = std::fs::read_to_string(&exclude_path).unwrap_or_default();
    if !content.lines().any(|l| l.trim() == ".wt/") {
        let mut new_content = content;
        if !new_content.is_empty() && !new_content.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push_str(".wt/\n");
        let _ = std::fs::write(&exclude_path, new_content);
    }

    remove_stale_init_worktrees(root);
    let pid = std::process::id();
    let wt_dir = root.join(".wt").join(format!("init-{}", pid));
    let _ = std::fs::create_dir_all(root.join(".wt"));

    let add_out = Command::new("git")
        .args(["worktree", "add", "--detach"])
        .arg(&wt_dir)
        .arg("HEAD")
        .current_dir(root)
        .output();

    if let Ok(o) = add_out {
        if !o.status.success() {
            eprintln!(
                "warning: git worktree add failed: {}; skipping verification",
                String::from_utf8_lossy(&o.stderr)
            );
            return (steps, false);
        }
    } else {
        eprintln!("warning: failed to run git worktree add; skipping verification");
        return (steps, false);
    }

    struct Guard<'a> {
        root: &'a Path,
        wt_dir: PathBuf,
    }
    impl<'a> Drop for Guard<'a> {
        fn drop(&mut self) {
            let _ = Command::new("git")
                .args(["worktree", "remove", "--force"])
                .arg(&self.wt_dir)
                .current_dir(self.root)
                .output();
        }
    }
    let _guard = Guard {
        root,
        wt_dir: wt_dir.clone(),
    };

    for (k, v) in env {
        std::env::set_var(k, v);
    }

    let mut setup_passed = false;
    if let Some(cmd) = setup {
        let log_path = wt_dir
            .parent()
            .unwrap_or(root)
            .join(format!("init-{pid}.setup.log"));
        match crate::setup::run_setup(
            cmd,
            &wt_dir,
            root,
            env,
            &log_path,
            crate::setup::SETUP_TIMEOUT,
        ) {
            Ok(()) => {
                setup_passed = true;
            }
            Err(msg) => {
                eprintln!("warning: setup failed; left out of blyte.toml:\n{msg}");
            }
        }
    }

    use blyte_agent::shell::Shell;
    let shell = if sandbox_on {
        let profile = blyte_agent::sandbox::default_profile_net(&wt_dir, &[], true);
        blyte_agent::shell::LocalShell::sandboxed(profile)
    } else {
        blyte_agent::shell::LocalShell::new()
    };

    for step in &mut steps {
        let start = std::time::Instant::now();
        let res = shell.exec(&step.command, &wt_dir, timeout);
        let elapsed = start.elapsed().as_secs();

        // A run that could not finish (a timeout, say) counts as failing, with its error as output.
        let (text, code) = match res {
            Ok(output) => (output.merged(), output.exit_code),
            Err(e) => (e, 1),
        };
        let verdict = classify(&text, code);
        match &verdict {
            Verdict::Passed => eprintln!("init: {} ... passed ({}s)", step.command, elapsed),
            Verdict::Environment(reason) => {
                eprintln!("init: {} ... left out: {}", step.command, reason)
            }
            Verdict::Failed => {
                eprintln!("init: {} ... failed on HEAD ({}s)", step.command, elapsed);
                for l in failure_excerpt(&text) {
                    eprintln!("init:   | {l}");
                }
            }
        }
        step.verdict = Some(verdict);
    }

    (steps, setup_passed)
}

pub fn run(args: &InitArgs) -> i32 {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output();
    let Ok(out) = out else {
        eprintln!("error: git not found");
        return 2;
    };
    if !out.status.success() {
        eprintln!("error: not a git repository");
        return 2;
    }
    let root = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let toml_path = root.join("blyte.toml");

    if !args.print && toml_path.exists() && !args.force {
        eprintln!("blyte.toml already exists; pass --force to overwrite");
        return 1;
    }

    let ci_steps = detect_ci_with_sources(&root);
    let ci_left_out = detect_ci_left_out(&root);
    for (cmd, _, reason) in &ci_left_out {
        eprintln!("init: {cmd} ... left out: {reason}");
    }
    let left_out_init_steps: Vec<InitStep> = ci_left_out
        .into_iter()
        .map(|(cmd, src, reason)| InitStep {
            command: cmd,
            source: src,
            verdict: Some(Verdict::Environment(reason)),
        })
        .collect();

    let (steps, from_ci) = if !ci_steps.is_empty() {
        let commands: Vec<String> = ci_steps.iter().map(|(cmd, _)| cmd.clone()).collect();
        let kept = drop_overlapping(commands);
        (
            kept.into_iter()
                .map(|cmd| {
                    let src = ci_steps
                        .iter()
                        .find(|(c, _)| c == &cmd)
                        .map(|(_, s)| s.clone())
                        .unwrap_or_default();
                    InitStep {
                        command: cmd,
                        source: src,
                        verdict: None,
                    }
                })
                .collect::<Vec<_>>(),
            true,
        )
    } else {
        let detected = crate::config::detect(&root);
        (
            detected
                .into_iter()
                .map(|cmd| InitStep {
                    command: cmd,
                    source: "detected".to_string(),
                    verdict: None,
                })
                .collect::<Vec<_>>(),
            false,
        )
    };

    if steps.is_empty() && left_out_init_steps.is_empty() {
        eprintln!(
            "no toolchain detected: write blyte.toml by hand (see blyte init --print for the format)"
        );
        return 1;
    }

    let env = detect_env(&root);
    let mut setup_parts = Vec::new();
    if let Some(root_setup) = detect_setup(&root) {
        setup_parts.push(root_setup);
    }
    setup_parts.extend(detect_ci_setup(&root));
    let setup = if setup_parts.is_empty() {
        None
    } else {
        Some(setup_parts.join(" && "))
    };

    let (mut verified_steps, mut setup_passed) = if !args.no_verify && !steps.is_empty() {
        verify_steps(
            &root,
            steps,
            &env,
            args.sandbox == "on",
            Duration::from_secs(args.step_timeout),
            setup.as_deref(),
        )
    } else {
        (steps, setup.is_some())
    };

    if from_ci && !args.no_verify {
        let any_passed = verified_steps
            .iter()
            .any(|s| matches!(s.verdict, Some(Verdict::Passed)));
        if !any_passed {
            let detected = crate::config::detect(&root);
            let not_all_identical = !detected.is_empty()
                && !detected
                    .iter()
                    .all(|d| verified_steps.iter().any(|s| &s.command == d));
            if not_all_identical {
                eprintln!(
                    "init: every step from CI failed or was left out; trying the checks detected from the repository"
                );
                let detected_steps: Vec<InitStep> = detected
                    .into_iter()
                    .map(|cmd| InitStep {
                        command: cmd,
                        source: "detected".to_string(),
                        verdict: None,
                    })
                    .collect();
                let (verified_detected, detected_setup_passed) = verify_steps(
                    &root,
                    detected_steps,
                    &env,
                    args.sandbox == "on",
                    Duration::from_secs(args.step_timeout),
                    setup.as_deref(),
                );
                if verified_detected
                    .iter()
                    .any(|s| matches!(s.verdict, Some(Verdict::Passed)))
                {
                    verified_steps = verified_detected;
                    setup_passed = detected_setup_passed;
                }
            }
        }
    }

    verified_steps.extend(left_out_init_steps);

    let kept_count = verified_steps
        .iter()
        .filter(|s| !matches!(s.verdict, Some(Verdict::Environment(_))))
        .count();

    if kept_count == 0 {
        eprintln!("error: every step was left out due to environment issues");
        for step in &verified_steps {
            if let Some(Verdict::Environment(ref r)) = step.verdict {
                eprintln!("  `{}` left out: {r}", step.command);
            }
        }
        return 1;
    }

    let rendered = render_verified(&verified_steps, &env, setup.as_deref(), setup_passed);

    if args.print {
        print!("{rendered}");
    } else {
        if let Err(e) = std::fs::write(&toml_path, &rendered) {
            eprintln!("error writing {}: {e}", toml_path.display());
            return 1;
        }
        println!("wrote {}", toml_path.display());
        for step in &verified_steps {
            if !matches!(step.verdict, Some(Verdict::Environment(_))) {
                println!("{}", step.command);
            }
        }
    }

    let any_failed = verified_steps
        .iter()
        .any(|s| matches!(s.verdict, Some(Verdict::Failed)));

    if any_failed {
        for s in &verified_steps {
            if matches!(s.verdict, Some(Verdict::Failed)) {
                eprintln!(
                    "warning: step `{}` failed on HEAD: fix it before dispatching",
                    s.command
                );
            }
        }
        3
    } else {
        0
    }
}
