//! `blyte init`: extracts the gate from CI workflows, detects environment settings,
//! and proves each step in a fresh worktree before writing `blyte.toml`.

use std::collections::{BTreeMap, HashSet};
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
    detect_ci_with_sources(root)
        .into_iter()
        .map(|(cmd, _)| cmd)
        .collect()
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

    let mut result = Vec::new();
    for cmd in candidate_commands {
        if let Some(kept) = filter_and_clean_command(&cmd) {
            result.push(kept);
        }
    }
    result
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
    let prefixes = ["uv run ", "poetry run ", "pdm run ", "npx ", "pipenv run "];
    let mut stripped = trimmed;
    for p in &prefixes {
        if stripped.starts_with(p) {
            stripped = &stripped[p.len()..];
            break;
        }
    }

    let check_cmds = [
        "pytest",
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

    let tokens = tokenize(text);
    let kept_tokens: Vec<String> = tokens
        .into_iter()
        .filter(|tok| {
            !tok.starts_with("--cov")
                && !tok.starts_with("--junitxml")
                && !tok.starts_with("--junit-xml")
                && !tok.starts_with("--cov-report")
        })
        .collect();

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

/// Classify step output into Passed, Environment problem, or Failed.
pub fn classify(output: &str, exit_code: i32) -> Verdict {
    if exit_code == 0 {
        return Verdict::Passed;
    }

    let patterns = [
        "401",
        "403",
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
        let lower = line.to_ascii_lowercase();
        if patterns.iter().any(|&p| lower.contains(p)) {
            let trimmed = line.trim();
            let s: String = if trimmed.chars().count() > 120 {
                trimmed.chars().take(120).collect()
            } else {
                trimmed.to_string()
            };
            return Verdict::Environment(s);
        }
    }

    Verdict::Failed
}

/// Render the verified blyte.toml content.
pub fn render_verified(steps: &[InitStep], env: &BTreeMap<String, String>) -> String {
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

fn verify_steps(
    root: &Path,
    mut steps: Vec<InitStep>,
    env: &BTreeMap<String, String>,
    sandbox_on: bool,
    timeout: Duration,
) -> Vec<InitStep> {
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
            return steps;
        }
    } else {
        eprintln!("warning: failed to run git worktree add; skipping verification");
        return steps;
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

        match res {
            Ok(output) => {
                let verdict = classify(&output.merged(), output.exit_code);
                match &verdict {
                    Verdict::Passed => {
                        eprintln!("init: {} ... passed ({}s)", step.command, elapsed);
                    }
                    Verdict::Environment(reason) => {
                        eprintln!("init: {} ... left out: {}", step.command, reason);
                    }
                    Verdict::Failed => {
                        eprintln!("init: {} ... failed on HEAD ({}s)", step.command, elapsed);
                    }
                }
                step.verdict = Some(verdict);
            }
            Err(e) => {
                let verdict = classify(&e, 1);
                match &verdict {
                    Verdict::Passed => {
                        eprintln!("init: {} ... passed ({}s)", step.command, elapsed);
                    }
                    Verdict::Environment(reason) => {
                        eprintln!("init: {} ... left out: {}", step.command, reason);
                    }
                    Verdict::Failed => {
                        eprintln!("init: {} ... failed on HEAD ({}s)", step.command, elapsed);
                    }
                }
                step.verdict = Some(verdict);
            }
        }
    }

    steps
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
    let (steps, is_ci) = if !ci_steps.is_empty() {
        (
            ci_steps
                .into_iter()
                .map(|(cmd, src)| InitStep {
                    command: cmd,
                    source: src,
                    verdict: None,
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

    let verified_steps = if is_ci && !args.no_verify {
        verify_steps(
            &root,
            steps,
            &env,
            args.sandbox == "on",
            Duration::from_secs(args.step_timeout),
        )
    } else {
        steps
    };

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

    let rendered = render_verified(&verified_steps, &env);

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
