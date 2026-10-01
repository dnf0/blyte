//! Python checks run as modules are CI checks too: `python3 -m unittest` gave "no toolchain
//! detected" (2026-10-01).

use blyte_cli::init::detect_ci;

#[test]
fn module_runs_and_tox_are_checks() {
    let d = tempfile::tempdir().unwrap();
    let wf = d.path().join(".github/workflows/ci.yml");
    std::fs::create_dir_all(wf.parent().unwrap()).unwrap();
    std::fs::write(
        &wf,
        "jobs:\n  t:\n    steps:\n      - run: pip install -e .\n      - run: python3 -m unittest -q\n      - run: uv run python -m pytest --cov=x tests/\n      - run: tox -e lint\n      - run: python3 -m pip install nox\n",
    )
    .unwrap();
    assert_eq!(
        detect_ci(d.path()),
        vec![
            "python3 -m unittest -q",
            "uv run python -m pytest tests/",
            "tox -e lint"
        ]
    );
}
