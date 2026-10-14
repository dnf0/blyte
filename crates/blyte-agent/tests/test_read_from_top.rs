//! A read from line 1 of a file that fits the read budget returns the whole file: a shadow run
//! read a 700-line file as L1-L200, L201-L400, ... six calls where one would do. Ranges that
//! start later are deliberate (a re-read after an edit) and are honoured.

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, READ_LINE_BUDGET};
use serde_json::json;

fn file(d: &tempfile::TempDir, name: &str, lines: usize) {
    let body: String = (1..=lines).map(|i| format!("{i:08}\n")).collect();
    std::fs::write(d.path().join(name), body).unwrap();
}

fn read(d: &tempfile::TempDir, args: serde_json::Value) -> String {
    execute(d.path(), &LocalShell, &LocalFs, "read", &args).content
}

#[test]
fn a_range_from_line_one_of_a_file_that_fits_returns_the_whole_file() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "run.rs", 700);
    let o = read(
        &d,
        json!({"path": "run.rs", "start_line": 1, "end_line": 200}),
    );
    assert_eq!(o.lines().count(), 700);
    assert!(o.ends_with("00000700"));
}

#[test]
fn an_end_line_alone_counts_as_a_read_from_the_top() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "run.rs", 700);
    let o = read(&d, json!({"path": "run.rs", "end_line": 50}));
    assert_eq!(o.lines().count(), 700);
}

#[test]
fn a_range_starting_later_is_honoured() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "run.rs", 700);
    let o = read(
        &d,
        json!({"path": "run.rs", "start_line": 470, "end_line": 490}),
    );
    assert_eq!(o.lines().count(), 21);
    assert!(o.contains("00000470") && o.ends_with("00000490"));
}

#[test]
fn a_range_from_the_top_of_a_file_too_big_to_fit_is_honoured() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "big.rs", READ_LINE_BUDGET + 500);
    let o = read(
        &d,
        json!({"path": "big.rs", "start_line": 1, "end_line": 100}),
    );
    assert_eq!(o.lines().count(), 100);
    assert!(!o.contains("elided"));
}

#[test]
fn an_end_line_before_the_start_line_is_an_error_not_a_panic() {
    let d = tempfile::tempdir().unwrap();
    file(&d, "run.rs", 700);
    let args = json!({"path": "run.rs", "start_line": 220, "end_line": 200});
    let o = execute(d.path(), &LocalShell, &LocalFs, "read", &args);
    assert!(o.is_error);
    assert!(o.content.contains("before start_line"));
}
