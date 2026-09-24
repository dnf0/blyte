//! Automated Test Slicing & Impact Analysis.
//!
//! This module analyzes git diff output against a target base branch to discover
//! which test files are directly affected or impacted by changes in source files.
//! It forms the foundation for progressive verification and test slicing, ensuring
//! fast milestone feedback while targeting only relevant test suites.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Check if a relative file path represents an existing or candidate test file.
///
/// Examines path components, file stem prefixes, suffixes, and extensions
/// across Python, Rust, TypeScript, JavaScript, and Go conventions.
fn is_test_file(path: &Path) -> bool {
    let file_name = match path.file_name().and_then(|s| s.to_str()) {
        Some(name) => name,
        // Guard against paths without valid filenames (e.g. root or empty)
        None => return false,
    };
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");

    // Identify if the file resides in any recognized test directory component
    let is_in_test_dir = path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s == "tests" || s == "test" || s == "__tests__"
    });

    // Check language-specific test naming patterns
    match ext {
        "py" => file_name.starts_with("test_") || file_name.ends_with("_test.py") || is_in_test_dir,
        "rs" => file_name.starts_with("test_") || file_name.ends_with("_test.rs") || is_in_test_dir,
        "ts" | "tsx" | "js" | "jsx" => {
            file_name.contains(".test.") || file_name.contains(".spec.") || is_in_test_dir
        }
        "go" => file_name.ends_with("_test.go"),
        _ => false,
    }
}

/// Collect candidate test files on disk that correspond to a modified source file.
///
/// Inspects language conventions:
/// - Python: `tests/test_<stem>.py`, `tests/<stem>_test.py`, `tests/unit/test_<stem>.py`, adjacent `test_<stem>.py`, mirrored `tests/<subpath>/...`
/// - Rust: `tests/test_<stem>.rs`, `tests/<stem>.rs`, adjacent `test_<stem>.rs`, mirrored `crates/<crate>/tests/...`
/// - TypeScript/JavaScript: `tests/<stem>.test.ts`, `tests/<stem>.spec.ts`, adjacent `<stem>.test.ts`, mirrored under `tests/` and `__tests__/`
/// - Go: `<dir>/<stem>_test.go`, `tests/<stem>_test.go`
fn collect_candidates_for_source(repo_root: &Path, rel_path: &Path, tests: &mut Vec<PathBuf>) {
    let file_stem = match rel_path.file_stem().and_then(|s| s.to_str()) {
        Some(s) if !s.is_empty() => s,
        // Guard clause: skip paths without a filename stem
        None => return,
        _ => return,
    };
    let ext = rel_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let parent = rel_path.parent().unwrap_or(Path::new(""));

    // Extract subdirectory structure located after 'src' for mirrored test directory resolution
    let after_src = {
        let mut past_src = false;
        let mut sub = PathBuf::new();
        for c in parent.components() {
            if past_src {
                sub.push(c);
            } else if c.as_os_str() == "src" {
                past_src = true;
            }
        }
        if past_src {
            Some(sub)
        } else {
            None
        }
    };

    // Extract package or crate directory prefix located before 'src' for workspace packages
    let before_src = {
        let mut prefix = PathBuf::new();
        let mut found_src = false;
        for c in parent.components() {
            if c.as_os_str() == "src" {
                found_src = true;
                break;
            }
            prefix.push(c);
        }
        if found_src && !prefix.as_os_str().is_empty() {
            Some(prefix)
        } else {
            None
        }
    };

    let mut candidate_paths: Vec<PathBuf> = Vec::new();

    match ext {
        "py" => {
            // Standard root tests directory conventions
            candidate_paths.push(PathBuf::from("tests").join(format!("test_{file_stem}.py")));
            candidate_paths.push(PathBuf::from("tests").join(format!("{file_stem}_test.py")));
            candidate_paths.push(PathBuf::from("tests/unit").join(format!("test_{file_stem}.py")));
            candidate_paths.push(PathBuf::from("tests/unit").join(format!("{file_stem}_test.py")));

            // Adjacent test files within the same directory as source
            candidate_paths.push(parent.join(format!("test_{file_stem}.py")));
            candidate_paths.push(parent.join(format!("{file_stem}_test.py")));

            // Mirrored test directory mirroring parent directory path
            candidate_paths.push(
                PathBuf::from("tests")
                    .join(parent)
                    .join(format!("test_{file_stem}.py")),
            );
            candidate_paths.push(
                PathBuf::from("tests")
                    .join(parent)
                    .join(format!("{file_stem}_test.py")),
            );

            // Mirrored test directory stripping the 'src' prefix (e.g. src/auth -> tests/auth)
            if let Some(ref sub) = after_src {
                if !sub.as_os_str().is_empty() {
                    candidate_paths.push(
                        PathBuf::from("tests")
                            .join(sub)
                            .join(format!("test_{file_stem}.py")),
                    );
                    candidate_paths.push(
                        PathBuf::from("tests")
                            .join(sub)
                            .join(format!("{file_stem}_test.py")),
                    );
                    candidate_paths.push(
                        PathBuf::from("tests/unit")
                            .join(sub)
                            .join(format!("test_{file_stem}.py")),
                    );
                    candidate_paths.push(
                        PathBuf::from("tests/unit")
                            .join(sub)
                            .join(format!("{file_stem}_test.py")),
                    );
                }
            }

            // Workspace package test locations (e.g. packages/<pkg>/tests)
            if let Some(ref pre) = before_src {
                candidate_paths.push(pre.join("tests").join(format!("test_{file_stem}.py")));
                candidate_paths.push(pre.join("tests").join(format!("{file_stem}_test.py")));
                if let Some(ref sub) = after_src {
                    if !sub.as_os_str().is_empty() {
                        candidate_paths.push(
                            pre.join("tests")
                                .join(sub)
                                .join(format!("test_{file_stem}.py")),
                        );
                        candidate_paths.push(
                            pre.join("tests")
                                .join(sub)
                                .join(format!("{file_stem}_test.py")),
                        );
                    }
                }
            }
        }
        "rs" => {
            // Standard Cargo integration tests directory
            candidate_paths.push(PathBuf::from("tests").join(format!("test_{file_stem}.rs")));
            candidate_paths.push(PathBuf::from("tests").join(format!("{file_stem}.rs")));

            // Adjacent test files
            candidate_paths.push(parent.join(format!("test_{file_stem}.rs")));
            candidate_paths.push(parent.join(format!("{file_stem}_test.rs")));

            // Mirrored parent test structure
            candidate_paths.push(
                PathBuf::from("tests")
                    .join(parent)
                    .join(format!("test_{file_stem}.rs")),
            );
            candidate_paths.push(
                PathBuf::from("tests")
                    .join(parent)
                    .join(format!("{file_stem}.rs")),
            );

            // Multi-crate workspace package test directories (e.g. crates/<crate>/tests)
            if let Some(ref pre) = before_src {
                candidate_paths.push(pre.join("tests").join(format!("test_{file_stem}.rs")));
                candidate_paths.push(pre.join("tests").join(format!("{file_stem}.rs")));
                if let Some(ref sub) = after_src {
                    if !sub.as_os_str().is_empty() {
                        candidate_paths.push(
                            pre.join("tests")
                                .join(sub)
                                .join(format!("test_{file_stem}.rs")),
                        );
                        candidate_paths
                            .push(pre.join("tests").join(sub).join(format!("{file_stem}.rs")));
                    }
                }
            }
        }
        "ts" | "tsx" | "js" | "jsx" => {
            // Comprehensive TypeScript and JavaScript test file suffixes
            let test_suffixes = [
                "test.ts", "spec.ts", "test.tsx", "spec.tsx", "test.js", "spec.js", "test.jsx",
                "spec.jsx",
            ];

            for suffix in test_suffixes {
                // Top-level tests/ and __tests__/ directories
                candidate_paths.push(PathBuf::from("tests").join(format!("{file_stem}.{suffix}")));
                candidate_paths
                    .push(PathBuf::from("__tests__").join(format!("{file_stem}.{suffix}")));

                // Adjacent test files
                candidate_paths.push(parent.join(format!("{file_stem}.{suffix}")));
                candidate_paths.push(
                    PathBuf::from("tests")
                        .join(parent)
                        .join(format!("{file_stem}.{suffix}")),
                );
                candidate_paths.push(
                    PathBuf::from("__tests__")
                        .join(parent)
                        .join(format!("{file_stem}.{suffix}")),
                );

                // Subdirectory mirrored test locations
                if let Some(ref sub) = after_src {
                    if !sub.as_os_str().is_empty() {
                        candidate_paths.push(
                            PathBuf::from("tests")
                                .join(sub)
                                .join(format!("{file_stem}.{suffix}")),
                        );
                        candidate_paths.push(
                            PathBuf::from("__tests__")
                                .join(sub)
                                .join(format!("{file_stem}.{suffix}")),
                        );
                    }
                }

                // Monorepo package directories
                if let Some(ref pre) = before_src {
                    candidate_paths.push(pre.join("tests").join(format!("{file_stem}.{suffix}")));
                    candidate_paths
                        .push(pre.join("__tests__").join(format!("{file_stem}.{suffix}")));
                }
            }
        }
        "go" => {
            // Go packages co-locate tests adjacent to source files
            candidate_paths.push(parent.join(format!("{file_stem}_test.go")));
            candidate_paths.push(PathBuf::from("tests").join(format!("{file_stem}_test.go")));
            if let Some(ref sub) = after_src {
                if !sub.as_os_str().is_empty() {
                    candidate_paths.push(
                        PathBuf::from("tests")
                            .join(sub)
                            .join(format!("{file_stem}_test.go")),
                    );
                }
            }
        }
        _ => {}
    }

    // Verify each candidate path exists on disk before including in results
    for cand in candidate_paths {
        let full = repo_root.join(&cand);
        if full.is_file() {
            tests.push(full);
        }
    }
}

/// Inspect git diff against base branch and discover test files covering modified source files.
///
/// Runs `git diff --name-only <base>` in `repo_root` to find all modified files.
/// For each modified file:
/// - If it is already a test file, it is included if it exists on disk.
/// - If it is a source file, candidate test locations are inspected across supported languages.
///
/// Returns a sorted and deduplicated list of test file paths.
///
/// # Parameters
/// - `repo_root`: The absolute or working directory path of the git repository.
/// - `base_branch`: Optional base branch name to diff against (defaults to "main").
///
/// # Returns
/// A sorted, deduplicated `Vec<PathBuf>` containing all discovered impacted test files.
pub fn infer_impacted_tests(repo_root: &Path, base_branch: Option<&str>) -> Vec<PathBuf> {
    let base = base_branch.unwrap_or("main");

    // Execute git diff to retrieve modified file names without octal-escaping non-ASCII paths
    let output = match Command::new("git")
        .args(["-c", "core.quotepath=off", "diff", "--name-only", base])
        .current_dir(repo_root)
        .output()
    {
        // On success, capture stdout as string
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        // Fallback for non-zero exit or execution error: return empty list safely
        _ => return Vec::new(),
    };

    let mut tests = Vec::new();

    // Iterate through diff entries and categorize modified files
    for raw_line in output.lines() {
        let line = raw_line.trim();

        // Strip optional surrounding quotes added by git for filenames with special characters
        let line = if line.starts_with('"') && line.ends_with('"') && line.len() >= 2 {
            &line[1..line.len() - 1]
        } else {
            line
        };

        // Skip blank lines
        if line.is_empty() {
            continue;
        }

        let rel_path = Path::new(line);

        // If the modified file is itself a test file, include it directly if it exists on disk
        if is_test_file(rel_path) {
            let full_path = repo_root.join(rel_path);
            if full_path.is_file() {
                tests.push(full_path);
            }
        } else {
            // Source file modified: search workspace for candidate test files
            collect_candidates_for_source(repo_root, rel_path, &mut tests);
        }
    }

    // Sort and deduplicate all found test file paths for deterministic output
    tests.sort();
    tests.dedup();
    tests
}

/// Helper to verify that a matched keyword in a command string respects word boundaries.
///
/// Ensures the matched keyword is not an arbitrary substring of a larger identifier
/// (e.g. `run_pytest_suite.sh` vs `pytest`).
fn is_word_boundary_match(s: &str, idx: usize, word_len: usize) -> bool {
    let bytes = s.as_bytes();
    // Prior character must be start of string or a path/argument separator
    let before_ok =
        idx == 0 || bytes[idx - 1] == b' ' || bytes[idx - 1] == b'\t' || bytes[idx - 1] == b'/';
    let after_idx = idx + word_len;
    // Succeeding character must be end of string or a whitespace separator
    let after_ok = after_idx == bytes.len()
        || bytes[after_idx] == b' '
        || bytes[after_idx] == b'\t'
        || bytes[after_idx] == b'\n';
    before_ok && after_ok
}
/// Splits a shell command string into discrete argument tokens respecting single and double quotes.
fn split_cmd_tokens(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;

    for c in s.chars() {
        if escape {
            current.push(c);
            escape = false;
        } else if c == '\\' {
            escape = true;
        } else if c == '\'' && !in_double {
            in_single = !in_single;
        } else if c == '"' && !in_single {
            in_double = !in_double;
        } else if c.is_whitespace() && !in_single && !in_double {
            if !current.is_empty() {
                tokens.push(current);
                current = String::new();
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Formats and deduplicates relative file paths from repository root.
fn format_rel_paths(files: &[PathBuf], repo_root: &Path) -> Vec<String> {
    let mut rel_paths: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(repo_root)
                .unwrap_or(p)
                .to_string_lossy()
                .to_string()
        })
        .collect();
    rel_paths.sort();
    rel_paths.dedup();
    rel_paths
}

/// Constructs a specialized, targeted test execution command for the detected test runner,
/// focusing execution solely on the provided impacted test files while preserving flags.
///
/// # Arguments
/// - `base_cmd`: The full milestone verification command string (e.g. `pytest`, `cargo test`).
/// - `impacted_files`: Slice of absolute paths to test files impacted by changes.
/// - `repo_root`: The base repository directory against which relative paths are formed.
///
/// # Returns
/// - `Some(specialized_cmd)` if the test runner is recognized and sliceable.
/// - `None` if `impacted_files` is empty or if the runner command is unrecognized.
pub fn build_sliced_test_cmd(
    base_cmd: &str,
    impacted_files: &[PathBuf],
    repo_root: &Path,
) -> Option<String> {
    // Guard clause: return None immediately if no test files were impacted
    if impacted_files.is_empty() {
        return None;
    }

    let trimmed = base_cmd.trim();

    // Never slice a command containing a heredoc.
    //
    // Detection below is a substring search for a runner name, which happily matches one
    // inside a heredoc BODY -- the script being fed to another process, not the command this
    // shell runs. Rewriting around that match produces an unterminated heredoc that executes
    // on the host instead of wherever the real command was routed. EXP-072: katex's gate
    // contained `jest` inside `cat <<'GATEEOF' | ./run-in-container sh -s`, and the slice
    // reported "No tests found ... In /workspace" four times, costing four attempts, $2.71
    // and the task. Slicing a command we cannot parse is worse than not slicing it.
    if trimmed.contains("<<") {
        return None;
    }

    // 1. Pytest detection (supports ./run-in-container pytest, python -m pytest, pytest)
    if let Some(prefix_idx) = trimmed.find("pytest") {
        if is_word_boundary_match(trimmed, prefix_idx, "pytest".len()) {
            let prefix = trimmed[..prefix_idx + "pytest".len()].trim();
            let suffix = trimmed[prefix_idx + "pytest".len()..].trim();
            let tokens = split_cmd_tokens(suffix);

            // Filter out directory targets or specific test file paths from the original command
            let mut kept_flags = Vec::new();
            for t in tokens {
                if t == "&&" || t == "||" || t == ";" || t == "|" || t == "&" {
                    break;
                }
                let is_path_target = t.ends_with(".py")
                    || t == "tests"
                    || t == "tests/"
                    || t.starts_with("tests/")
                    || t == "test"
                    || t == "test/"
                    || t.starts_with("test/");
                if !is_path_target {
                    kept_flags.push(t);
                }
            }

            let rel_paths = format_rel_paths(impacted_files, repo_root);
            if !rel_paths.is_empty() {
                let mut parts = vec![prefix.to_string()];
                if !kept_flags.is_empty() {
                    parts.push(kept_flags.join(" "));
                }
                parts.push(rel_paths.join(" "));
                return Some(parts.join(" "));
            }
        }
    }

    // 2. Cargo test detection
    if let Some(idx) = trimmed.find("cargo test") {
        if is_word_boundary_match(trimmed, idx, "cargo test".len()) {
            let prefix = trimmed[..idx + "cargo test".len()].trim();
            let suffix = trimmed[idx + "cargo test".len()..].trim();
            let tokens = split_cmd_tokens(suffix);

            let mut kept_flags = Vec::new();
            let mut skip_next = false;
            let mut prev_token_takes_value = false;
            for t in tokens {
                if t == "&&" || t == "||" || t == ";" || t == "|" || t == "&" {
                    break;
                }
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if t == "--test" || t == "--bench" {
                    skip_next = true;
                    continue;
                }
                if t.starts_with("--test=") || t.starts_with("--bench=") {
                    continue;
                }
                if !t.starts_with('-') && !prev_token_takes_value {
                    // Positional target or filter stem: drop it
                    continue;
                }
                prev_token_takes_value = t == "--features"
                    || t == "-p"
                    || t == "--package"
                    || t == "--bin"
                    || t == "--example"
                    || t == "--target";
                kept_flags.push(t);
            }

            let mut test_args = Vec::new();
            for file in impacted_files {
                let rel = file.strip_prefix(repo_root).unwrap_or(file);
                if let Some(parent) = rel.parent() {
                    if parent == Path::new("tests") || parent.ends_with("tests") {
                        if let Some(stem) = rel.file_stem().and_then(|s| s.to_str()) {
                            test_args.push(format!("--test {}", stem));
                            continue;
                        }
                    }
                }
                if let Some(stem) = rel.file_stem().and_then(|s| s.to_str()) {
                    test_args.push(stem.to_string());
                }
            }
            test_args.sort();
            test_args.dedup();

            if !test_args.is_empty() {
                let mut parts = vec![prefix.to_string()];
                if !kept_flags.is_empty() {
                    parts.push(kept_flags.join(" "));
                }
                parts.push(test_args.join(" "));
                return Some(parts.join(" "));
            }
        }
    }

    // 3. Go test detection
    if let Some(idx) = trimmed.find("go test") {
        if is_word_boundary_match(trimmed, idx, "go test".len()) {
            let prefix = trimmed[..idx + "go test".len()].trim();
            let suffix = trimmed[idx + "go test".len()..].trim();
            let tokens = split_cmd_tokens(suffix);

            let mut kept_flags = Vec::new();
            for t in tokens {
                if t == "&&" || t == "||" || t == ";" || t == "|" || t == "&" {
                    break;
                }
                let is_pkg_target = t == "./..."
                    || t == "..."
                    || t == "."
                    || t == "./"
                    || t.ends_with("/...")
                    || (t.starts_with("./") && !t.starts_with("-"));
                if !is_pkg_target {
                    kept_flags.push(t);
                }
            }

            let mut pkgs = Vec::new();
            for file in impacted_files {
                let rel = file.strip_prefix(repo_root).unwrap_or(file);
                let parent = rel.parent().unwrap_or(Path::new(""));
                let pkg = if parent.as_os_str().is_empty() {
                    ".".to_string()
                } else if parent.starts_with(".") {
                    parent.to_string_lossy().to_string()
                } else {
                    format!("./{}", parent.to_string_lossy())
                };
                pkgs.push(pkg);
            }
            pkgs.sort();
            pkgs.dedup();

            if !pkgs.is_empty() {
                let mut parts = vec![prefix.to_string()];
                if !kept_flags.is_empty() {
                    parts.push(kept_flags.join(" "));
                }
                parts.push(pkgs.join(" "));
                return Some(parts.join(" "));
            }
        }
    }

    // 4. Node / Vitest / Jest detection
    let rel_paths = format_rel_paths(impacted_files, repo_root);
    if !rel_paths.is_empty() {
        for (pattern, use_double_dash) in [
            ("npm test", true),
            ("pnpm test", true),
            ("yarn test", false),
            ("vitest run", false),
            ("vitest", false),
            ("jest", false),
        ] {
            if let Some(idx) = trimmed.find(pattern) {
                if is_word_boundary_match(trimmed, idx, pattern.len()) {
                    let prefix = trimmed[..idx + pattern.len()].trim();
                    let suffix = trimmed[idx + pattern.len()..].trim();
                    let tokens = split_cmd_tokens(suffix);

                    let mut kept_flags = Vec::new();
                    for t in tokens {
                        if t == "&&" || t == "||" || t == ";" || t == "|" || t == "&" {
                            break;
                        }
                        if t == "--" {
                            continue;
                        }
                        let is_target = t.ends_with(".js")
                            || t.ends_with(".ts")
                            || t.ends_with(".jsx")
                            || t.ends_with(".tsx")
                            || t == "tests"
                            || t == "tests/"
                            || t.starts_with("tests/")
                            || t == "test"
                            || t == "test/"
                            || t.starts_with("test/");
                        if !is_target {
                            kept_flags.push(t);
                        }
                    }

                    let mut parts = vec![prefix.to_string()];
                    if use_double_dash {
                        if !kept_flags.is_empty() {
                            parts.push(kept_flags.join(" "));
                        }
                        parts.push(format!("-- {}", rel_paths.join(" ")));
                    } else {
                        if !kept_flags.is_empty() {
                            parts.push(kept_flags.join(" "));
                        }
                        parts.push(rel_paths.join(" "));
                    }
                    return Some(parts.join(" "));
                }
            }
        }
    }

    None
}
