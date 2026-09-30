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
