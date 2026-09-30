//! CI steps as real repositories write them: a check chained after setup, and reporting options
//! whose value is a separate word. pigeonhole's `mkdir -p … && uv run pytest --cov-report xml:…`
//! was missed, and `blyte init` fell back to a docs-only `make check` (2026-09-30).

use blyte_cli::init::detect_ci;

#[test]
fn a_chained_check_with_spaced_option_values_is_found_and_cleaned() {
    let d = tempfile::tempdir().unwrap();
    let wf = d.path().join(".github/workflows/pull_request.yaml");
    std::fs::create_dir_all(wf.parent().unwrap()).unwrap();
    std::fs::write(
        &wf,
        "jobs:\n  t:\n    steps:\n      - name: Run tests\n        run: | # set pipefail to ensure the script fails if pytest fails\n          set -eo pipefail\n          mkdir -p tests/coverage && \\\n          uv run pytest --cov=pigeonhole \\\n          --junitxml=tests/coverage/pytest.xml \\\n          --cov-report=term-missing:skip-covered \\\n          --cov-report xml:tests/coverage/pytest-coverage.xml \\\n          --cov-fail-under=80 tests/\n      - run: uv run ruff check . ; uv run pyright\n",
    )
    .unwrap();
    assert_eq!(
        detect_ci(d.path()),
        vec![
            "uv run pytest tests/",
            "uv run ruff check .",
            "uv run pyright"
        ]
    );
}

#[test]
fn the_pre_commit_action_is_a_lint_step() {
    let d = tempfile::tempdir().unwrap();
    let wf = d.path().join(".github/workflows/pr.yaml");
    std::fs::create_dir_all(wf.parent().unwrap()).unwrap();
    std::fs::write(
        &wf,
        "jobs:\n  lint:\n    steps:\n      - uses: actions/checkout@v4\n      - uses: pre-commit/action@v3.0.1\n        with:\n          extra_args: --all-files\n      - run: uv run pytest\n",
    )
    .unwrap();
    assert_eq!(
        detect_ci(d.path()),
        vec!["pre-commit run --all-files", "uv run pytest"]
    );
}

#[test]
fn a_test_named_after_a_status_code_is_not_an_environment_failure() {
    use blyte_cli::init::{classify, Verdict};
    let out = "tests/flows/x_test.py::TestCheck::test_403_always_failed PASSED\nFAILED tests/y.py::test_z - AssertionError\n1 failed, 900 passed\n";
    assert_eq!(classify(out, 1), Verdict::Failed);
    assert!(matches!(
        classify(
            "error: HTTP status client error (401 Unauthorized) for url\n",
            1
        ),
        Verdict::Environment(_)
    ));
}

#[test]
fn a_failure_shows_the_runner_summary_not_shutdown_noise() {
    use blyte_cli::init::failure_excerpt;
    let out = "FAILED tests/a.py::test_x - KeyError\n= 1 failed, 900 passed in 190s =\nERROR asyncio - Task was destroyed but it is pending!\ntask: <Task pending>\n";
    assert_eq!(
        failure_excerpt(out),
        vec![
            "FAILED tests/a.py::test_x - KeyError",
            "= 1 failed, 900 passed in 190s ="
        ]
    );
    assert_eq!(failure_excerpt("a\nb\n"), vec!["a", "b"]);
}
