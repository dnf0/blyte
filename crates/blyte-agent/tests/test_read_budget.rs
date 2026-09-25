//! `read` returns whole files up to READ_LINE_BUDGET lines (within MAX_OUTPUT_CHARS); command
//! output keeps OUTPUT_LINE_BUDGET. Paging a 650-line file cost 15 turns in a dogfood run.

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, MAX_OUTPUT_CHARS, OUTPUT_LINE_BUDGET, READ_LINE_BUDGET};
use serde_json::json;

fn file(d: &tempfile::TempDir, name: &str, lines: usize, width: usize) {
    let body: String = (1..=lines).map(|i| format!("{i:0width$}\n")).collect();
    std::fs::write(d.path().join(name), body).unwrap();
}

#[test]
fn a_file_under_the_read_budget_comes_back_whole() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "mid.rs", 1_000, 8);
    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"path": "mid.rs"}),
    );
    assert!(
        !o.content.contains("elided"),
        "a 1,000-line file must not be cut"
    );
    assert_eq!(o.content.lines().count(), 1_000);
    assert!(o.content.contains("00000500") && o.content.ends_with("00001000"));
}

#[test]
fn a_file_over_the_read_budget_is_cut_with_head_and_tail() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "big.rs", READ_LINE_BUDGET + 1_000, 8);
    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"path": "big.rs"}),
    );
    assert!(o.content.contains("elided by blyte"));
    assert!(o.content.contains("00000001"), "head kept");
    assert!(
        o.content
            .contains(&format!("{:08}", READ_LINE_BUDGET + 1_000)),
        "tail kept"
    );
}

#[test]
fn the_character_cap_still_holds_for_reads() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "wide.rs", 1_500, 120);
    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"path": "wide.rs"}),
    );
    assert!(
        o.content.chars().count() <= MAX_OUTPUT_CHARS + 200,
        "{}",
        o.content.len()
    );
}

#[test]
fn command_output_keeps_the_smaller_budget() {
    let d = tempfile::tempdir().unwrap();
    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "bash",
        &json!({"command": "seq 1 1000"}),
    );
    assert!(o.content.contains("elided by blyte"));
    assert!(o.content.lines().count() < OUTPUT_LINE_BUDGET + 20);
    const { assert!(READ_LINE_BUDGET > OUTPUT_LINE_BUDGET) };
}
