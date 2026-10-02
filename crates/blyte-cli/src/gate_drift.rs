//! Warn when a repository's gate is missing checks its CI runs (2026-10-02).

use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CheckKey {
    Make(String),
    Cargo(String),
    Go(String),
    Npm(String),
    Pnpm(String),
    Yarn(String),
    Ruff(String),
    Tool(String),
}

impl CheckKey {
    pub fn key_text(&self) -> String {
        match self {
            CheckKey::Make(target) => format!("make {target}"),
            CheckKey::Cargo(subcmd) => format!("cargo {subcmd}"),
            CheckKey::Go(subcmd) => format!("go {subcmd}"),
            CheckKey::Npm(script) => format!("npm run {script}"),
            CheckKey::Pnpm(script) => format!("pnpm run {script}"),
            CheckKey::Yarn(script) => format!("yarn {script}"),
            CheckKey::Ruff(subcmd) => format!("ruff {subcmd}"),
            CheckKey::Tool(tool) => tool.clone(),
        }
    }

    pub fn matches(&self, other: &CheckKey) -> bool {
        if self == other {
            return true;
        }
        match (self, other) {
            (CheckKey::Npm(s1), CheckKey::Pnpm(s2))
            | (CheckKey::Npm(s1), CheckKey::Yarn(s2))
            | (CheckKey::Pnpm(s1), CheckKey::Npm(s2))
            | (CheckKey::Pnpm(s1), CheckKey::Yarn(s2))
            | (CheckKey::Yarn(s1), CheckKey::Npm(s2))
            | (CheckKey::Yarn(s1), CheckKey::Pnpm(s2)) => s1 == s2,
            _ => false,
        }
    }
}

pub fn split_pipeline_and_chain(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quote = None;
    let chars: Vec<char> = line.chars().collect();
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
                if ch == '"' || ch == '\'' {
                    in_quote = Some(ch);
                    current.push(ch);
                    i += 1;
                } else if (ch == '&' && i + 1 < chars.len() && chars[i + 1] == '&')
                    || (ch == '|' && i + 1 < chars.len() && chars[i + 1] == '|')
                {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed.to_string());
                    }
                    current.clear();
                    i += 2;
                } else if ch == ';' || ch == '|' {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed.to_string());
                    }
                    current.clear();
                    i += 1;
                } else {
                    current.push(ch);
                    i += 1;
                }
            }
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_string());
    }
    parts
}

fn tokenize_cmd(s: &str) -> Vec<String> {
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

fn unquote(s: &str) -> &str {
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn is_env_assignment(s: &str) -> bool {
    let Some(eq_idx) = s.find('=') else {
        return false;
    };
    if eq_idx == 0 {
        return false;
    }
    let key = &s[..eq_idx];
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_uv_flag_with_arg(flag: &str) -> bool {
    if flag.contains('=') {
        return false;
    }
    matches!(
        flag,
        "--extra"
            | "-e"
            | "--package"
            | "-p"
            | "--python"
            | "--directory"
            | "--project"
            | "--env-file"
            | "--with"
            | "--with-requirements"
            | "--from"
            | "--allow-insecure-host"
    )
}

fn is_uvx_flag_with_arg(flag: &str) -> bool {
    if flag.contains('=') {
        return false;
    }
    matches!(
        flag,
        "--from" | "--with" | "--python" | "--package" | "-p" | "-e"
    )
}

fn is_python_binary(tok: &str) -> bool {
    if tok == "python" || tok == "python3" {
        return true;
    }
    if let Some(rest) = tok.strip_prefix("python3.") {
        return rest.chars().all(|c| c.is_ascii_digit());
    }
    false
}

fn unwrap_wrappers(tokens: &[String]) -> &[String] {
    let mut idx = 0;
    while idx < tokens.len() {
        let tok = unquote(&tokens[idx]);

        // 1. Env assignments like FOO=1, BAR="hello"
        if is_env_assignment(tok) {
            idx += 1;
            continue;
        }

        // env command: env [flags] [VAR=VAL] ...
        if tok == "env" {
            idx += 1;
            while idx < tokens.len() {
                let t = unquote(&tokens[idx]);
                if t == "-i" || t.starts_with("-u") || is_env_assignment(t) {
                    idx += 1;
                } else {
                    break;
                }
            }
            continue;
        }

        // 2. xargs [flags]
        if tok == "xargs" {
            idx += 1;
            while idx < tokens.len() {
                let t = unquote(&tokens[idx]);
                if t.starts_with('-') {
                    if matches!(t, "-I" | "-n" | "-L" | "-s" | "-E" | "-d" | "-a") {
                        idx += 1;
                        if idx < tokens.len() && !unquote(&tokens[idx]).starts_with('-') {
                            idx += 1;
                        }
                    } else {
                        idx += 1;
                    }
                } else {
                    break;
                }
            }
            continue;
        }

        // 3. uv run [flags]
        if tok == "uv" && idx + 1 < tokens.len() && unquote(&tokens[idx + 1]) == "run" {
            idx += 2;
            while idx < tokens.len() {
                let t = unquote(&tokens[idx]);
                if t.starts_with('-') {
                    if is_uv_flag_with_arg(t) {
                        idx += 1;
                        if idx < tokens.len() && !unquote(&tokens[idx]).starts_with('-') {
                            idx += 1;
                        }
                    } else {
                        idx += 1;
                    }
                } else {
                    break;
                }
            }
            continue;
        }

        // 4. uvx [--from X] [flags]
        if tok == "uvx" {
            idx += 1;
            while idx < tokens.len() {
                let t = unquote(&tokens[idx]);
                if t.starts_with('-') {
                    if is_uvx_flag_with_arg(t) {
                        idx += 1;
                        if idx < tokens.len() && !unquote(&tokens[idx]).starts_with('-') {
                            idx += 1;
                        }
                    } else {
                        idx += 1;
                    }
                } else {
                    break;
                }
            }
            continue;
        }

        // 5. poetry run [flags]
        if tok == "poetry" && idx + 1 < tokens.len() && unquote(&tokens[idx + 1]) == "run" {
            idx += 2;
            while idx < tokens.len() {
                let t = unquote(&tokens[idx]);
                if t.starts_with('-') {
                    if matches!(t, "-C" | "--directory") {
                        idx += 1;
                        if idx < tokens.len() && !unquote(&tokens[idx]).starts_with('-') {
                            idx += 1;
                        }
                    } else {
                        idx += 1;
                    }
                } else {
                    break;
                }
            }
            continue;
        }

        // 6. pipenv run [flags]
        if tok == "pipenv" && idx + 1 < tokens.len() && unquote(&tokens[idx + 1]) == "run" {
            idx += 2;
            while idx < tokens.len() && unquote(&tokens[idx]).starts_with('-') {
                idx += 1;
            }
            continue;
        }

        // 7. pdm run [flags]
        if tok == "pdm" && idx + 1 < tokens.len() && unquote(&tokens[idx + 1]) == "run" {
            idx += 2;
            while idx < tokens.len() && unquote(&tokens[idx]).starts_with('-') {
                idx += 1;
            }
            continue;
        }

        // 8. npx [flags]
        if tok == "npx" {
            idx += 1;
            while idx < tokens.len() {
                let t = unquote(&tokens[idx]);
                if t.starts_with('-') {
                    if matches!(t, "-p" | "--package") {
                        idx += 1;
                        if idx < tokens.len() && !unquote(&tokens[idx]).starts_with('-') {
                            idx += 1;
                        }
                    } else {
                        idx += 1;
                    }
                } else {
                    break;
                }
            }
            continue;
        }

        // 9. pnpm exec [flags]
        if tok == "pnpm" && idx + 1 < tokens.len() && unquote(&tokens[idx + 1]) == "exec" {
            idx += 2;
            while idx < tokens.len() && unquote(&tokens[idx]).starts_with('-') {
                idx += 1;
            }
            continue;
        }

        // 10. bunx [flags]
        if tok == "bunx" {
            idx += 1;
            while idx < tokens.len() && unquote(&tokens[idx]).starts_with('-') {
                idx += 1;
            }
            continue;
        }

        // 11. python -m / python3 -m
        if is_python_binary(tok) {
            let mut m_idx = idx + 1;
            while m_idx < tokens.len() {
                let t = unquote(&tokens[m_idx]);
                if t == "-m" {
                    break;
                }
                if t.starts_with('-') {
                    m_idx += 1;
                } else {
                    break;
                }
            }
            if m_idx < tokens.len() && unquote(&tokens[m_idx]) == "-m" && m_idx + 1 < tokens.len() {
                idx = m_idx + 1;
                continue;
            }
        }

        break;
    }
    &tokens[idx..]
}

fn is_pnpm_builtin(s: &str) -> bool {
    matches!(
        s,
        "install"
            | "i"
            | "add"
            | "remove"
            | "rm"
            | "update"
            | "up"
            | "link"
            | "unlink"
            | "import"
            | "rebuild"
            | "prune"
            | "fetch"
            | "publish"
            | "pack"
            | "exec"
            | "dlx"
            | "create"
            | "init"
    )
}

fn is_yarn_builtin(s: &str) -> bool {
    matches!(
        s,
        "install"
            | "add"
            | "remove"
            | "upgrade"
            | "config"
            | "create"
            | "cache"
            | "link"
            | "unlink"
            | "publish"
            | "pack"
            | "init"
            | "dlx"
            | "node"
    )
}

fn extract_check_key_from_tokens(tokens: &[String]) -> Option<CheckKey> {
    if tokens.is_empty() {
        return None;
    }
    let cmd = unquote(&tokens[0]);
    match cmd {
        "make" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('-') {
                    if (t == "-C" || t == "-f") && i + 1 < tokens.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                } else if is_env_assignment(t) {
                    i += 1;
                } else {
                    return Some(CheckKey::Make(t.to_string()));
                }
            }
            None
        }
        "cargo" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('+') || t.starts_with('-') {
                    i += 1;
                } else {
                    return Some(CheckKey::Cargo(t.to_string()));
                }
            }
            None
        }
        "go" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('-') {
                    i += 1;
                } else {
                    return Some(CheckKey::Go(t.to_string()));
                }
            }
            None
        }
        "npm" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('-') {
                    i += 1;
                } else if t == "test" || t == "t" {
                    return Some(CheckKey::Npm("test".to_string()));
                } else if t == "run" || t == "run-script" {
                    i += 1;
                    while i < tokens.len() {
                        let script = unquote(&tokens[i]);
                        if script.starts_with('-') {
                            i += 1;
                        } else {
                            return Some(CheckKey::Npm(script.to_string()));
                        }
                    }
                    return None;
                } else {
                    return None;
                }
            }
            None
        }
        "pnpm" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('-') {
                    i += 1;
                } else if t == "test" || t == "t" {
                    return Some(CheckKey::Pnpm("test".to_string()));
                } else if t == "run" {
                    i += 1;
                    while i < tokens.len() {
                        let script = unquote(&tokens[i]);
                        if script.starts_with('-') {
                            i += 1;
                        } else {
                            return Some(CheckKey::Pnpm(script.to_string()));
                        }
                    }
                    return None;
                } else if !is_pnpm_builtin(t) {
                    return Some(CheckKey::Pnpm(t.to_string()));
                } else {
                    return None;
                }
            }
            None
        }
        "yarn" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('-') {
                    i += 1;
                } else if t == "run" {
                    i += 1;
                    while i < tokens.len() {
                        let script = unquote(&tokens[i]);
                        if script.starts_with('-') {
                            i += 1;
                        } else {
                            return Some(CheckKey::Yarn(script.to_string()));
                        }
                    }
                    return None;
                } else if !is_yarn_builtin(t) {
                    return Some(CheckKey::Yarn(t.to_string()));
                } else {
                    return None;
                }
            }
            None
        }
        "ruff" => {
            let mut i = 1;
            while i < tokens.len() {
                let t = unquote(&tokens[i]);
                if t.starts_with('-') {
                    i += 1;
                } else if t == "check" || t == "format" {
                    return Some(CheckKey::Ruff(t.to_string()));
                } else {
                    return None;
                }
            }
            None
        }
        "pytest" | "pre-commit" | "pyright" | "mypy" | "eslint" | "vitest" | "jest" | "tsc"
        | "tox" | "nox" | "black" | "isort" => Some(CheckKey::Tool(cmd.to_string())),
        _ => None,
    }
}

pub fn extract_check_keys(line: &str) -> Vec<CheckKey> {
    let mut keys = Vec::new();
    for part in split_pipeline_and_chain(line) {
        let tokens = tokenize_cmd(&part);
        let unwrapped = unwrap_wrappers(&tokens);
        if let Some(key) = extract_check_key_from_tokens(unwrapped) {
            keys.push(key);
        }
    }
    keys
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

fn contains_word_phrase(haystack: &str, needle: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let actual_pos = start + pos;
        let end_pos = actual_pos + needle.len();

        let before_ok = if actual_pos == 0 {
            true
        } else {
            let prev_char = haystack[..actual_pos].chars().next_back().unwrap();
            !is_identifier_char(prev_char)
        };

        let after_ok = if end_pos == haystack.len() {
            true
        } else {
            let next_char = haystack[end_pos..].chars().next().unwrap();
            !is_identifier_char(next_char)
        };

        if before_ok && after_ok {
            return true;
        }
        start = actual_pos + 1;
    }
    false
}

fn blyte_toml_comment_covers(blyte_toml_content: &str, key: &CheckKey) -> bool {
    blyte_toml_content.lines().any(|line| {
        let trimmed = line.trim();
        let Some(after_hash) = trimmed.strip_prefix('#') else {
            return false;
        };
        let comment = after_hash.trim();
        if contains_word_phrase(comment, &key.key_text()) {
            return true;
        }
        match key {
            CheckKey::Npm(s) if s == "test" => contains_word_phrase(comment, "npm test"),
            CheckKey::Pnpm(s) if s == "test" => {
                contains_word_phrase(comment, "pnpm test")
                    || contains_word_phrase(comment, "npm test")
            }
            CheckKey::Yarn(s) => contains_word_phrase(comment, &format!("yarn run {s}")),
            _ => false,
        }
    })
}

fn has_python_project_file(root: &Path) -> bool {
    let files = [
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "tox.ini",
        "noxfile.py",
        "pytest.ini",
    ];
    files.iter().any(|f| root.join(f).exists())
}

pub fn is_project_file_missing(root: &Path, key: &CheckKey) -> bool {
    match key {
        CheckKey::Cargo(_) => !root.join("Cargo.toml").exists(),
        CheckKey::Go(_) => !root.join("go.mod").exists(),
        CheckKey::Npm(_) | CheckKey::Pnpm(_) | CheckKey::Yarn(_) => {
            !root.join("package.json").exists()
        }
        CheckKey::Make(target) => {
            let makefiles = ["Makefile", "makefile", "GNUmakefile"];
            for mf in makefiles {
                let path = root.join(mf);
                if path.exists() {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        for line in content.lines() {
                            if line.starts_with('\t') || line.starts_with('#') {
                                continue;
                            }
                            let trimmed = line.trim_start();
                            if let Some(rest) = trimmed.strip_prefix(target.as_str()) {
                                if rest.starts_with(':') || rest.trim_start().starts_with(':') {
                                    return false;
                                }
                            }
                        }
                    }
                }
            }
            true
        }
        CheckKey::Ruff(_) => !has_python_project_file(root),
        CheckKey::Tool(tool) => match tool.as_str() {
            "pre-commit" => {
                !root.join(".pre-commit-config.yaml").exists()
                    && !root.join(".pre-commit-config.yml").exists()
            }
            "eslint" | "vitest" | "jest" | "tsc" => !root.join("package.json").exists(),
            "pytest" | "pyright" | "mypy" | "tox" | "nox" | "black" | "isort" => {
                !has_python_project_file(root)
            }
            _ => false,
        },
    }
}

/// Check whether a CI step is covered by the gate commands, marked in comments,
/// or inapplicable because its project file is missing at the repository root.
pub fn is_ci_step_covered(root: &Path, step: &str, gate: &str, blyte_toml_content: &str) -> bool {
    let step_keys = extract_check_keys(step);
    if step_keys.is_empty() {
        return true;
    }
    let gate_keys: Vec<CheckKey> = gate.lines().flat_map(extract_check_keys).collect();

    step_keys.iter().all(|key| {
        // 1. A gate line has the same key
        if gate_keys.iter().any(|gk| gk.matches(key)) {
            return true;
        }
        // 2. Any comment line in blyte.toml contains the step's key text, whatever it says
        if blyte_toml_comment_covers(blyte_toml_content, key) {
            return true;
        }
        // 3. The tool's project file is missing at the repository root
        if is_project_file_missing(root, key) {
            return true;
        }
        false
    })
}

/// Compute CI steps from repository workflows that are not covered by the gate, comments,
/// or missing project files.
pub fn missing_ci_steps(root: &Path, gate: &str, blyte_toml_content: &str) -> Vec<String> {
    let ci_steps = crate::init::drop_overlapping(crate::init::detect_ci(root));
    if ci_steps.is_empty() {
        return Vec::new();
    }
    let mut seen_keys = HashSet::new();
    let mut missing = Vec::new();

    for step in ci_steps {
        if is_ci_step_covered(root, &step, gate, blyte_toml_content) {
            continue;
        }
        let step_keys = extract_check_keys(&step);
        let mut is_new = false;
        for key in step_keys {
            if seen_keys.insert(key) {
                is_new = true;
            }
        }
        if is_new {
            missing.push(step);
        }
    }
    missing
}

/// Format the single-line warning for missing CI steps.
pub fn format_gate_drift_warning(missing: &[String]) -> Option<String> {
    if missing.is_empty() {
        None
    } else {
        Some(format!(
            "blyte: the gate is missing CI steps: {} (run blyte init --force to refresh blyte.toml)",
            missing.join(", ")
        ))
    }
}
