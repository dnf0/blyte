//! `read` keeps long lines whole up to READ_LINE_CHARS: a plan's 640-character contract line was
//! clipped at 500 and the model never saw it. Command output keeps MAX_LINE_CHARS.

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, MAX_LINE_CHARS, READ_LINE_CHARS};
use serde_json::json;

fn write(d: &tempfile::TempDir, name: &str, body: &str) {
    std::fs::write(d.path().join(name), body).unwrap();
}

#[test]
fn read_keeps_a_long_line_whole() {
    let d = tempfile::tempdir().unwrap();
    let line = format!("start {} end", "x".repeat(1_500));
    write(&d, "plan.md", &format!("# Plan\n{line}\nlast\n"));
    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"path": "plan.md"}),
    );
    assert!(
        o.content.contains(&line),
        "the whole 1,510-character line must come back"
    );
    assert!(!o.content.contains("clipped by blyte"));
}

#[test]
fn read_still_clips_a_minified_line() {
    let d = tempfile::tempdir().unwrap();
    write(&d, "app.min.js", &"y".repeat(50_000));
    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"path": "app.min.js"}),
    );
    assert!(o.content.contains("clipped by blyte"));
    assert!(
        o.content.chars().count() < READ_LINE_CHARS + 200,
        "{}",
        o.content.len()
    );
}

#[test]
fn command_output_keeps_the_short_line_cap() {
    let d = tempfile::tempdir().unwrap();
    write(&d, "plan.md", &format!("{}\n", "z".repeat(1_500)));
    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "bash",
        &json!({"command": "cat plan.md"}),
    );
    assert!(o.content.contains("clipped by blyte"));
    assert!(o.content.chars().count() < MAX_LINE_CHARS + 200);
    const { assert!(READ_LINE_CHARS > MAX_LINE_CHARS) };
}
