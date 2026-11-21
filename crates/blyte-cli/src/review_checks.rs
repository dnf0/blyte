//! Scope checks for `blyte review`.
//!
//! Flags files changed that the task text never mentions, commit subjects
//! not matching what the task specified, host commands, outside reads, and
//! non-verified outcomes.

const IGNORED_LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "uv.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "poetry.lock",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checks {
    pub flags: Vec<String>,
}

/// Files changed by the run that the task text never mentions.
/// A file counts as mentioned when the task text contains its full repo-relative path,
/// or its file name (last path component), anywhere (code spans, prose, lists).
/// Ignored (never flagged): Cargo.lock, uv.lock, package-lock.json, pnpm-lock.yaml, yarn.lock, poetry.lock.
pub fn unmentioned_files(task: &str, changed: &[String]) -> Vec<String> {
    changed
        .iter()
        .filter(|path| {
            if path.is_empty() {
                return false;
            }
            let file_name = std::path::Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(path.as_str());

            if IGNORED_LOCKFILES.contains(&file_name) || IGNORED_LOCKFILES.contains(&path.as_str())
            {
                return false;
            }

            let mentioned =
                task.contains(path.as_str()) || (!file_name.is_empty() && task.contains(file_name));
            !mentioned
        })
        .cloned()
        .collect()
}

/// Commit subjects (first lines) that do not appear verbatim in the task text.
/// Returns them in the order given. If the task text contains no line starting with
/// "commit" or "Commit" (case-insensitive word "commit" anywhere in a heading or line), return empty:
/// the task did not specify a message.
pub fn unexpected_subjects(task: &str, subjects: &[String]) -> Vec<String> {
    if !specifies_commit(task) {
        return Vec::new();
    }
    subjects
        .iter()
        .filter(|s| !task.contains(s.as_str()))
        .cloned()
        .collect()
}

fn specifies_commit(task: &str) -> bool {
    for line in task.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') && contains_word_commit(trimmed) {
            return true;
        }
        let stripped = trimmed.trim_start_matches(|c: char| {
            c.is_whitespace()
                || c == '*'
                || c == '-'
                || c == '_'
                || c == '>'
                || c == '`'
                || c == '#'
        });
        if stripped.to_lowercase().starts_with("commit")
            || stripped.to_lowercase().contains("commit message")
        {
            return true;
        }
    }
    false
}

fn contains_word_commit(s: &str) -> bool {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w == "commit" || w == "commits")
}

fn sanitize_flag(flag: String) -> String {
    let single_line = flag.lines().next().unwrap_or("").to_string();
    if single_line.chars().count() > 200 {
        single_line.chars().take(200).collect()
    } else {
        single_line
    }
}

const KNOWN_FILE_EXTENSIONS: &[&str] = &[
    "rs",
    "py",
    "pyi",
    "ts",
    "tsx",
    "js",
    "jsx",
    "mjs",
    "cjs",
    "go",
    "c",
    "h",
    "cc",
    "cpp",
    "cxx",
    "hpp",
    "hh",
    "java",
    "kt",
    "kts",
    "scala",
    "cs",
    "fs",
    "rb",
    "php",
    "swift",
    "m",
    "mm",
    "zig",
    "nim",
    "lua",
    "pl",
    "pm",
    "r",
    "jl",
    "ex",
    "exs",
    "erl",
    "hrl",
    "hs",
    "ml",
    "mli",
    "clj",
    "cljs",
    "el",
    "vim",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "bat",
    "cmd",
    "sql",
    "toml",
    "yaml",
    "yml",
    "json",
    "jsonc",
    "json5",
    "xml",
    "html",
    "htm",
    "css",
    "scss",
    "sass",
    "less",
    "vue",
    "svelte",
    "astro",
    "md",
    "mdx",
    "rst",
    "txt",
    "adoc",
    "tex",
    "csv",
    "tsv",
    "ini",
    "cfg",
    "conf",
    "env",
    "properties",
    "lock",
    "mod",
    "sum",
    "proto",
    "thrift",
    "avsc",
    "graphql",
    "gql",
    "prisma",
    "tf",
    "tfvars",
    "hcl",
    "nix",
    "dhall",
    "star",
    "bzl",
    "cmake",
    "mk",
    "gradle",
    "sbt",
    "cabal",
    "gemspec",
    "podspec",
    "csproj",
    "fsproj",
    "sln",
    "plist",
    "svg",
    "png",
    "jpg",
    "jpeg",
    "gif",
    "ico",
    "webp",
    "woff",
    "woff2",
    "ttf",
    "eot",
    "wasm",
    "wat",
    "dockerfile",
];

const KNOWN_EXTENSIONLESS_FILES: &[&str] = &[
    "Makefile",
    "Dockerfile",
    "Containerfile",
    "Justfile",
    "Procfile",
    "Gemfile",
    "Rakefile",
    "Brewfile",
    "Vagrantfile",
    "LICENSE",
    "CHANGELOG",
    "README",
    "OWNERS",
    "CODEOWNERS",
    "BUILD",
    "WORKSPACE",
];

fn looks_like_file_reference(raw: &str) -> bool {
    let token = raw
        .trim_start_matches(|c: char| {
            matches!(
                c,
                '`' | '\'' | '"' | '(' | '[' | '{' | '<' | '*' | ',' | ';' | ':'
            )
        })
        .trim_end_matches(|c: char| {
            matches!(
                c,
                '`' | '\'' | '"' | ')' | ']' | '}' | '>' | '*' | ',' | ';' | ':' | '.' | '!' | '?'
            )
        });
    if token.is_empty()
        || token.starts_with("http://")
        || token.starts_with("https://")
        || token.contains("://")
    {
        return false;
    }

    let last_segment = token.rsplit('/').next().unwrap_or(token);
    if last_segment.is_empty() {
        return false;
    }
    if KNOWN_EXTENSIONLESS_FILES.contains(&last_segment) {
        return true;
    }

    if let Some((stem, ext)) = last_segment.rsplit_once('.') {
        let valid_stem = !stem.is_empty()
            && stem
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
        if !valid_stem || ext.is_empty() {
            return false;
        }
        let lower_ext = ext.to_ascii_lowercase();
        if KNOWN_FILE_EXTENSIONS.contains(&lower_ext.as_str()) {
            return true;
        }
        if token.contains('/')
            && ext.len() <= 6
            && ext.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
        {
            return true;
        }
    }

    false
}

/// Returns whether the task text names any file or file path.
pub fn task_names_files(task: &str) -> bool {
    task.split_whitespace().any(looks_like_file_reference)
}

fn is_unnamed_files_flag(flag: &str) -> bool {
    flag.starts_with("files not named in the task")
}

/// Returns whether a verified run should receive a fresh model review.
pub fn should_run_model_review(
    flags: &[String],
    task_text: Option<&str>,
    risky_matches: &[String],
) -> bool {
    if !risky_matches.is_empty() {
        return true;
    }
    let names_files = task_text.is_some_and(task_names_files);
    flags
        .iter()
        .any(|f| names_files || !is_unnamed_files_flag(f))
}

/// Build the flags list. Each flag is one line of at most 200 characters.
pub fn build(
    task: Option<&str>,
    changed: &[String],
    subjects: &[String],
    host_cmds: usize,
    outside_reads: usize,
    outcome: Option<&str>,
) -> Checks {
    let mut flags = Vec::new();

    if task.is_none() {
        flags.push("task file not found: scope not checked".to_string());
    }

    if let Some(outc) = outcome {
        if outc != "verified" {
            flags.push(format!("outcome {outc}: review in depth"));
        }
    }

    if changed.is_empty() {
        flags.push("no files changed".to_string());
    }

    if let Some(t) = task {
        let unmentioned = unmentioned_files(t, changed);
        if !unmentioned.is_empty() {
            let msg = if unmentioned.len() <= 10 {
                format!("files not named in the task: {}", unmentioned.join(", "))
            } else {
                let first_10 = &unmentioned[..10];
                let more = unmentioned.len() - 10;
                format!(
                    "files not named in the task: {}, … (+{more} more)",
                    first_10.join(", ")
                )
            };
            flags.push(msg);
        }

        let unexpected = unexpected_subjects(t, subjects);
        for subj in unexpected {
            flags.push(format!("commit message not in the task: {subj}"));
        }
    }

    if host_cmds > 0 {
        flags.push(format!("{host_cmds} host command(s): see == host commands"));
    }

    if outside_reads > 0 {
        flags.push(format!(
            "{outside_reads} read(s) outside the repository: see == reads outside the repository"
        ));
    }

    let flags = flags.into_iter().map(sanitize_flag).collect();
    Checks { flags }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unmentioned_files() {
        let task = "Mentions `src/a.rs` by path and `b.rs` by name only";
        let changed = vec![
            "src/a.rs".to_string(),
            "src/x/b.rs".to_string(),
            "src/c.rs".to_string(),
            "Cargo.lock".to_string(),
        ];
        assert_eq!(unmentioned_files(task, &changed), vec!["src/c.rs"]);
    }

    #[test]
    fn test_unexpected_subjects() {
        let task = "Commit message: `feat: add x`";
        let subjects = vec!["feat: add x".to_string(), "fix: y".to_string()];
        assert_eq!(unexpected_subjects(task, &subjects), vec!["fix: y"]);
    }

    #[test]
    fn test_unexpected_subjects_no_commit() {
        let task = "Just implement the feature and run the tests.";
        let subjects = vec!["feat: add x".to_string(), "fix: y".to_string()];
        assert!(unexpected_subjects(task, &subjects).is_empty());
    }

    #[test]
    fn test_build_none_task() {
        let checks = build(None, &[], &[], 0, 0, None);
        assert_eq!(
            checks.flags.first().map(|s| s.as_str()),
            Some("task file not found: scope not checked")
        );
    }

    #[test]
    fn test_build_outcome_gate_failed() {
        let checks = build(
            Some("mentions a.rs"),
            &["a.rs".to_string()],
            &[],
            0,
            0,
            Some("gate_failed"),
        );
        assert!(checks
            .flags
            .contains(&"outcome gate_failed: review in depth".to_string()));
    }

    #[test]
    fn test_build_12_unmentioned_files() {
        let changed: Vec<String> = (0..12).map(|i| format!("f{i}.rs")).collect();
        let checks = build(Some("mentions none of them"), &changed, &[], 0, 0, None);
        assert_eq!(checks.flags.len(), 1);
        let flag = &checks.flags[0];
        for i in 0..10 {
            assert!(flag.contains(&format!("f{i}.rs")));
        }
        assert!(!flag.contains("f10.rs"));
        assert!(!flag.contains("f11.rs"));
        assert!(flag.contains("(+2 more)"));
    }

    #[test]
    fn test_build_clean() {
        let task = "Files: `src/a.rs`\nCommit message: `feat: x`";
        let changed = vec!["src/a.rs".to_string()];
        let subjects = vec!["feat: x".to_string()];
        let checks = build(Some(task), &changed, &subjects, 0, 0, Some("verified"));
        assert!(checks.flags.is_empty());
    }

    #[test]
    fn test_build_host_cmds() {
        let checks = build(Some(""), &[], &[], 2, 0, None);
        assert!(checks
            .flags
            .contains(&"2 host command(s): see == host commands".to_string()));
    }

    #[test]
    fn test_ignored_lockfiles() {
        let task = "no files mentioned";
        let changed = vec![
            "Cargo.lock".to_string(),
            "uv.lock".to_string(),
            "package-lock.json".to_string(),
            "pnpm-lock.yaml".to_string(),
            "yarn.lock".to_string(),
            "poetry.lock".to_string(),
        ];
        assert!(unmentioned_files(task, &changed).is_empty());
    }

    #[test]
    fn test_build_outside_reads() {
        let checks = build(Some(""), &[], &[], 0, 3, None);
        assert!(checks.flags.contains(
            &"3 read(s) outside the repository: see == reads outside the repository".to_string()
        ));
    }
}
