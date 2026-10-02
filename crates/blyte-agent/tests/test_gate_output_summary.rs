//! A long gate failure keeps the test runner's summary, not just shutdown noise at the end
//! (2026-10-01).

use blyte_agent::run::cut_gate_output;

#[test]
fn failing_test_names_survive_the_cut() {
    let mut out = String::from("1 of 2 steps failed: uv run pytest -q\n");
    for i in 0..200 {
        out.push_str(&format!("tests/test_mod_{i}.py ........ [ {i}%]\n"));
    }
    out.push_str("FAILED tests/flows/paths_test.py::test_process_tile - KeyError: 'x'\n");
    out.push_str("FAILED tests/core/partition_test.py::test_cells_cover_the_world\n");
    out.push_str("= 2 failed, 3345 passed, 11 skipped in 199.65s =\n");
    for _ in 0..200 {
        out.push_str("12:46:35.607 | ERROR   | asyncio - Task was destroyed but it is pending!\n");
    }
    let cut = cut_gate_output(&out);
    assert!(cut.chars().count() <= 4_500, "{}", cut.chars().count());
    assert!(cut.starts_with("1 of 2 steps failed"), "{cut}");
    assert!(cut.contains("Failing, from the full output:"), "{cut}");
    assert!(
        cut.contains("FAILED tests/flows/paths_test.py::test_process_tile"),
        "{cut}"
    );
    assert!(
        cut.contains("FAILED tests/core/partition_test.py::test_cells_cover_the_world"),
        "{cut}"
    );
    assert!(cut.contains("2 failed, 3345 passed"), "{cut}");
    let short = "1 of 1 steps failed: cargo test\nerror: oops\n";
    assert_eq!(cut_gate_output(short), short);
}
