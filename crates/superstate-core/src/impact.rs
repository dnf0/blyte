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

    // Execute git diff to retrieve modified file names
    let output = match Command::new("git")
        .args(["diff", "--name-only", base])
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
