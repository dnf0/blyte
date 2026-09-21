//! Autonomous TDD decomposition: the 2-node DAG that replaces hand-authored gates.
//!
//! Manifesto Invariant V: "Autonomous software engineering cannot rely on synthetic
//! hand-authored scaffolds or answer-key leakage. Pre-seeding maintainer reproduction tests
//! is cheating; it turns an open-ended engineering diagnosis into a narrow test-passing
//! game." Authoring the reproduction test IS the engineering work being measured, so the
//! control plane must never hand the worker a ready-made assertion.
//!
//! This module derives, from the repository alone, two milestones:
//!
//!   1. RED   -- the worker authors its own reproduction test. The gate proves the test
//!               *compiles* and *fails* against the pristine code.
//!   2. GREEN -- the worker implements the fix. The gate proves the reproduction test and
//!               the repository's own suite both pass.
//!
//! The compile proof in RED is what stops the milestone being vacuous. Without it a worker
//! could satisfy "the test fails" with a syntax error, which certifies nothing.
//!
//! Detection is deliberately fail-closed: an unrecognised repository returns an error rather
//! than a permissive command. A gate that cannot fail is the defect this replaces, so
//! silently degrading to `true` would reintroduce it.

use std::path::Path;

use crate::models::MilestoneSpec;

/// Task id of the reproduction milestone. Stable so tooling can recognise the DAG.
pub const TDD_REPRODUCTION_ID: &str = "tdd-reproduction-test";
/// Task id of the implementation milestone.
pub const TDD_IMPLEMENTATION_ID: &str = "fix-implementation";

/// Why a repository could not be decomposed into the TDD DAG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TddError {
    /// No supported test runner could be identified from the repository layout.
    UnknownRunner,
}

impl std::fmt::Display for TddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TddError::UnknownRunner => write!(
                f,
                "could not identify a test runner (looked for go.mod, package.json, \
                 pyproject.toml/setup.py/setup.cfg/pytest.ini/tox.ini). Autonomous TDD needs \
                 the repository's own runner; refusing to emit a gate that cannot fail"
            ),
        }
    }
}

impl std::error::Error for TddError {}

/// A repository's native test runner and the reproduction-test path that suits it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TddProfile {
    /// Human-readable runner name, recorded in the milestone title.
    pub runner: &'static str,
    /// Path, relative to the repository root, where the worker must author its test.
    pub reproduction_path: &'static str,
    /// Shell command proving the reproduction test compiles and FAILS.
    pub red_cmd: String,
    /// Shell command proving the reproduction test and the full suite PASS.
    pub green_cmd: String,
}

/// Whether any entry in `dir` matches `pred`, without descending.
fn any_entry(dir: &Path, pred: impl Fn(&str) -> bool) -> bool {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return false;
    };
    read_dir.flatten().any(|e| {
        e.file_name()
            .to_str()
            .map(|n| pred(n))
            .unwrap_or(false)
    })
}

/// Identifies the repository's native test runner.
///
/// Order matters: a polyglot repository is classified by the manifest at its root, and Go is
/// checked first because a `go.mod` repository that also carries a `package.json` for tooling
/// is still a Go project.
pub fn detect_profile(repo_root: &Path) -> Result<TddProfile, TddError> {
    if repo_root.join("go.mod").is_file() {
        return Ok(go_profile());
    }
    if repo_root.join("package.json").is_file() {
        return Ok(node_profile(repo_root));
    }
    let python_markers = [
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "pytest.ini",
        "tox.ini",
    ];
    if python_markers.iter().any(|m| repo_root.join(m).is_file()) {
        return Ok(python_profile());
    }
    // A bare `tests/` directory holding Python files is enough to identify pytest when the
    // project ships no packaging manifest at all.
    let tests_dir = repo_root.join("tests");
    if tests_dir.is_dir() && any_entry(&tests_dir, |n| n.ends_with(".py")) {
        return Ok(python_profile());
    }
    Err(TddError::UnknownRunner)
}

/// pytest: `--collect-only` proves the module imports before `!` proves the assertion fails.
fn python_profile() -> TddProfile {
    let path = "tests/test_reproduce_issue.py";
    TddProfile {
        runner: "pytest",
        reproduction_path: path,
        red_cmd: format!(
            "test -f {path} && python -m pytest {path} --collect-only -q >/dev/null 2>&1 \
             && ! python -m pytest {path} -q"
        ),
        green_cmd: format!("python -m pytest {path} -q && python -m pytest -q"),
    }
}

/// Go: `go test -run '^$' ./...` compiles every test binary and runs no test, so exit 0 is a
/// pure compilation proof. The reproduction test is then required to fail by name.
fn go_profile() -> TddProfile {
    let path = "reproduce_issue_test.go";
    TddProfile {
        runner: "go test",
        reproduction_path: path,
        red_cmd: format!(
            "test -f {path} && go test -run '^$' ./... >/dev/null 2>&1 \
             && ! go test -run TestReproduceIssue ./..."
        ),
        green_cmd: "go test -run TestReproduceIssue ./... && go test ./...".to_string(),
    }
}

/// Node: vitest when it is a declared dependency, otherwise the project's own `npm test`.
///
/// TypeScript projects get a `tsc --noEmit` compile proof when a tsconfig is present, because
/// a JavaScript test runner cannot distinguish "failed to parse" from "asserted and failed".
fn node_profile(repo_root: &Path) -> TddProfile {
    let manifest = std::fs::read_to_string(repo_root.join("package.json")).unwrap_or_default();
    let uses_vitest = manifest.contains("\"vitest\"");
    let typescript = repo_root.join("tsconfig.json").is_file();
    let path = if typescript {
        "tests/reproduce-issue.test.ts"
    } else {
        "tests/reproduce-issue.test.js"
    };

    let compile_proof = if typescript {
        format!("test -f {path} && npx --no-install tsc --noEmit >/dev/null 2>&1 && ")
    } else {
        format!("test -f {path} && ")
    };

    let (runner, run_one, run_all) = if uses_vitest {
        (
            "vitest",
            format!("npx --no-install vitest run {path}"),
            "npx --no-install vitest run".to_string(),
        )
    } else {
        (
            "npm test",
            format!("npm test -- {path}"),
            "npm test".to_string(),
        )
    };

    TddProfile {
        runner,
        reproduction_path: path,
        red_cmd: format!("{compile_proof}! {run_one}"),
        green_cmd: format!("{run_one} && {run_all}"),
    }
}

/// Builds the 2-node TDD DAG for a repository.
///
/// The titles carry the worker's instructions because the milestone title is what reaches the
/// prompt. They state the artefact and the mechanical condition, never how to diagnose the
/// issue -- that is the work under measurement.
pub fn tdd_milestones(repo_root: &Path) -> Result<Vec<MilestoneSpec>, TddError> {
    let profile = detect_profile(repo_root)?;
    Ok(vec![
        MilestoneSpec {
            id: Some(TDD_REPRODUCTION_ID.to_string()),
            title: format!(
                "Author a reproduction test at `{}` that demonstrates the reported issue. \
                 It must compile and FAIL against the current, unfixed code ({} asserting RED). \
                 Do not modify any other file in this milestone.",
                profile.reproduction_path, profile.runner
            ),
            verify_cmd: profile.red_cmd.clone(),
            depends_on: None,
            files: vec![profile.reproduction_path.into()],
        },
        MilestoneSpec {
            id: Some(TDD_IMPLEMENTATION_ID.to_string()),
            title: format!(
                "Implement the minimal fix so that `{}` passes and the repository's existing \
                 suite still passes ({} asserting GREEN). Do not weaken or delete the \
                 reproduction test.",
                profile.reproduction_path, profile.runner
            ),
            verify_cmd: profile.green_cmd.clone(),
            depends_on: Some(vec![TDD_REPRODUCTION_ID.to_string()]),
            files: Vec::new(),
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (path, body) in files {
            let full = dir.path().join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("mkdir");
            }
            fs::write(full, body).expect("write");
        }
        dir
    }

    #[test]
    fn detects_go_repositories_by_go_mod() {
        let d = repo(&[("go.mod", "module x\n")]);
        let p = detect_profile(d.path()).expect("go profile");
        assert_eq!(p.runner, "go test");
        assert_eq!(p.reproduction_path, "reproduce_issue_test.go");
    }

    #[test]
    fn detects_python_repositories_by_packaging_manifest() {
        for marker in ["pyproject.toml", "setup.py", "pytest.ini", "tox.ini"] {
            let d = repo(&[(marker, "")]);
            let p = detect_profile(d.path()).expect("python profile");
            assert_eq!(p.runner, "pytest", "marker {marker}");
        }
    }

    #[test]
    fn detects_python_from_a_bare_tests_directory() {
        let d = repo(&[("tests/test_thing.py", "")]);
        assert_eq!(detect_profile(d.path()).unwrap().runner, "pytest");
    }

    #[test]
    fn detects_vitest_and_prefers_typescript_when_tsconfig_present() {
        let d = repo(&[
            ("package.json", r#"{"devDependencies":{"vitest":"^2"}}"#),
            ("tsconfig.json", "{}"),
        ]);
        let p = detect_profile(d.path()).expect("node profile");
        assert_eq!(p.runner, "vitest");
        assert_eq!(p.reproduction_path, "tests/reproduce-issue.test.ts");
        assert!(
            p.red_cmd.contains("tsc --noEmit"),
            "TypeScript needs a compile proof because a JS runner cannot tell a parse error \
             from an assertion failure: {}",
            p.red_cmd
        );
    }

    #[test]
    fn falls_back_to_npm_test_without_vitest() {
        let d = repo(&[("package.json", r#"{"scripts":{"test":"mocha"}}"#)]);
        let p = detect_profile(d.path()).expect("node profile");
        assert_eq!(p.runner, "npm test");
        assert_eq!(p.reproduction_path, "tests/reproduce-issue.test.js");
    }

    #[test]
    fn go_wins_over_a_tooling_package_json() {
        let d = repo(&[("go.mod", "module x\n"), ("package.json", "{}")]);
        assert_eq!(detect_profile(d.path()).unwrap().runner, "go test");
    }

    #[test]
    fn an_unrecognised_repository_is_an_error_not_a_permissive_gate() {
        let d = repo(&[("README.md", "hello")]);
        assert_eq!(detect_profile(d.path()), Err(TddError::UnknownRunner));
    }

    #[test]
    fn red_gate_proves_compilation_before_asserting_failure() {
        // Without the compile proof a worker satisfies RED with a syntax error, which
        // certifies nothing. Every profile must check compilation first.
        for files in [
            vec![("go.mod", "module x\n")],
            vec![("pyproject.toml", "")],
            vec![
                ("package.json", r#"{"devDependencies":{"vitest":"^2"}}"#),
                ("tsconfig.json", "{}"),
            ],
        ] {
            let d = repo(&files);
            let p = detect_profile(d.path()).expect("profile");
            let negation = p.red_cmd.find("! ").expect("red gate must negate a run");
            let proof_markers = ["--collect-only", "go test -run '^$'", "tsc --noEmit"];
            let proof_at = proof_markers
                .iter()
                .filter_map(|m| p.red_cmd.find(m))
                .min()
                .unwrap_or_else(|| panic!("no compile proof in {}", p.red_cmd));
            assert!(
                proof_at < negation,
                "compile proof must precede the failure assertion: {}",
                p.red_cmd
            );
        }
    }

    #[test]
    fn green_gate_runs_the_reproduction_test_and_the_full_suite() {
        let d = repo(&[("pyproject.toml", "")]);
        let p = detect_profile(d.path()).unwrap();
        assert!(p.green_cmd.contains("tests/test_reproduce_issue.py"));
        assert!(
            p.green_cmd.contains("&&"),
            "GREEN must require both the reproduction test and the existing suite"
        );
    }

    #[test]
    fn milestones_form_a_two_node_dag_in_order() {
        let d = repo(&[("go.mod", "module x\n")]);
        let m = tdd_milestones(d.path()).expect("milestones");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].id.as_deref(), Some(TDD_REPRODUCTION_ID));
        assert_eq!(m[1].id.as_deref(), Some(TDD_IMPLEMENTATION_ID));
        assert_eq!(m[0].depends_on, None);
        assert_eq!(
            m[1].depends_on,
            Some(vec![TDD_REPRODUCTION_ID.to_string()]),
            "implementation must not be reachable before the test is proven RED"
        );
        assert_eq!(m[0].files, vec![std::path::PathBuf::from("reproduce_issue_test.go")]);
    }

    #[test]
    fn milestone_titles_name_the_artefact_and_the_mechanical_condition() {
        let d = repo(&[("pyproject.toml", "")]);
        let m = tdd_milestones(d.path()).unwrap();
        assert!(m[0].title.contains("tests/test_reproduce_issue.py"));
        assert!(m[0].title.contains("FAIL"));
        assert!(m[1].title.contains("GREEN"));
        assert!(
            m[1].title.contains("Do not weaken or delete"),
            "the implementation milestone must forbid deleting the test that gates it"
        );
    }

    #[test]
    fn no_milestone_references_the_sealed_grader() {
        let d = repo(&[("pyproject.toml", "")]);
        for milestone in tdd_milestones(d.path()).unwrap() {
            assert!(
                !milestone.verify_cmd.contains("/tests/"),
                "a gate must never read the sealed grader at container root: {}",
                milestone.verify_cmd
            );
        }
    }
}
