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

    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for file in files {
        let file_name = file
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or_default()
            .to_string();
        if let Ok(content) = std::fs::read_to_string(&file) {
            let cmds = parse_workflow_commands(&content);
            for cmd in cmds {
                if seen.insert(cmd.clone()) {
                    result.push((cmd, file_name.clone()));
                }
            }
        }
    }
    result
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

fn parse_workflow_commands(content: &str) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    let mut candidate_commands = Vec::new();

    while i < lines.len() {
        let line = lines[i];
        let trimmed_start = line.trim_start();

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
            candidate_commands.push("pre-commit run --all-files".to_string());
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
                for cmd in joined {
                    candidate_commands.push(cmd);
                }
                continue;
            } else if !after_run_clean.is_empty() {
                let mut cmd_str = after_run_clean;
                if (cmd_str.starts_with('"') && cmd_str.ends_with('"'))
                    || (cmd_str.starts_with('\'') && cmd_str.ends_with('\''))
                {
                    cmd_str = &cmd_str[1..cmd_str.len() - 1];
                }
                candidate_commands.push(cmd_str.to_string());
            }
        }
        i += 1;
    }

    // A CI line often chains setup and the check (`mkdir -p out && uv run pytest`): each part
    // is judged on its own.
    let mut result = Vec::new();
    for cmd in candidate_commands {
        for part in split_chain(&cmd) {
            if let Some(kept) = filter_and_clean_command(&part) {
                result.push(kept);
            }
        }
    }
    result
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
    for p in &prefixes {
        if stripped.starts_with(p) {
            stripped = &stripped[p.len()..];
            break;
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

    let is_check = check_cmds.iter().any(|&c| {
        if stripped == c {
            true
        } else if let Some(rest) = stripped.strip_prefix(c) {
            rest.starts_with(char::is_whitespace)
        } else {
            false
        }
    });

    if !is_check {
        return None;
    }

    let text = if let Some(idx) = trimmed.find("| tee") {
        &trimmed[..idx]
    } else {
        trimmed
    };

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

fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = None;

    for ch in s.chars() {
        match in_quote {
            Some(q) => {
                current.push(ch);
                if ch == q {
                    in_quote = None;
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    in_quote = Some(ch);
                    current.push(ch);
                } else if ch.is_whitespace() {
                    if !current.is_empty() {
                        tokens.push(current);
                        current = String::new();
                    }
                } else {
                    current.push(ch);
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
        if setup_passed {
            out.push_str("# prepares each new worktree before the implementer starts\n");
            out.push_str(&format!("setup = \"{s}\"\n"));
        } else {
            out.push_str(&format!(
                "# setup = \"{s}\"   # left out: failed in a fresh worktree\n"
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

    if steps.is_empty() {
        eprintln!(
            "no toolchain detected: write blyte.toml by hand (see blyte init --print for the format)"
        );
        return 1;
    }

    let env = detect_env(&root);
    let setup = detect_setup(&root);

    let (mut verified_steps, mut setup_passed) = if !args.no_verify {
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
