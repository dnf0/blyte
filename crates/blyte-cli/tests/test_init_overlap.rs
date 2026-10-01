//! Two pytest steps that test the same files are one step: provision's CI ran the suite twice, and
//! so did its gate (2026-10-01).

use blyte_cli::init::drop_overlapping;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn the_broader_of_two_overlapping_pytest_steps_is_kept() {
    assert_eq!(
        drop_overlapping(s(&[
            "pre-commit run --all-files",
            "uv run pytest tests/ --no-cov -m \"not integration and not slow and not benchmark\"",
            "uv run pytest tests/ -m \"not benchmark\"",
        ])),
        s(&[
            "pre-commit run --all-files",
            "uv run pytest tests/ -m \"not benchmark\""
        ])
    );
    // No positional arguments overlaps with any; a tie keeps the first.
    assert_eq!(
        drop_overlapping(s(&["uv run pytest", "uv run pytest tests/"])),
        s(&["uv run pytest"])
    );
    // Different files: both kept. Other tools are never dropped.
    assert_eq!(
        drop_overlapping(s(&[
            "pytest tests/unit",
            "pytest tests/integration",
            "ruff check .",
            "ruff check ."
        ])),
        s(&[
            "pytest tests/unit",
            "pytest tests/integration",
            "ruff check .",
            "ruff check ."
        ])
    );
}
