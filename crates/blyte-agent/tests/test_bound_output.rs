//! No tool result can overflow the model's context: output is bounded by size, not only lines,
//! and the grep fallback never searches dependency or build directories.

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{bound_output, execute, grep_command, MAX_LINE_CHARS, MAX_OUTPUT_CHARS};
use serde_json::json;

#[test]
fn a_huge_single_line_is_clipped() {
    let out = bound_output(&"a".repeat(5_000_000));
    assert!(out.len() < MAX_LINE_CHARS + 100, "{} chars", out.len());
    assert!(
        out.contains("more chars clipped"),
        "{}",
        &out[out.len() - 80..]
    );
}

#[test]
fn many_long_lines_are_capped_in_total() {
    let text: String = (0..300)
        .map(|i| format!("{i} {}\n", "x".repeat(450)))
        .collect();
    let out = bound_output(&text);
    assert!(out.len() <= MAX_OUTPUT_CHARS + 200, "{} chars", out.len());
    assert!(out.contains("elided by blyte"));
    assert!(out.starts_with("0 "), "the head is kept");
    assert!(
        out.trim_end().starts_with("0 ") && out.contains("299 "),
        "the tail is kept"
    );
}

#[test]
fn ordinary_output_is_untouched() {
    let text = "exit 0\nall 11 tests passed\n";
    assert_eq!(bound_output(text), text);
}

#[test]
fn every_tool_result_is_bounded() {
    let d = tempfile::tempdir().unwrap();
    // One 2 MB line, as minified JavaScript produces.
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "bash",
        &json!({"command": "head -c 2000000 /dev/zero | tr '\\0' a"}),
    );
    assert!(
        r.content.len() < MAX_OUTPUT_CHARS,
        "{} chars",
        r.content.len()
    );
    assert!(r.content.starts_with("exit 0"));

    std::fs::write(d.path().join("min.js"), "b".repeat(3_000_000)).unwrap();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"path": "min.js"}),
    );
    assert!(
        r.content.len() < MAX_OUTPUT_CHARS,
        "{} chars",
        r.content.len()
    );
}

#[test]
fn the_grep_fallback_skips_dependency_and_build_directories() {
    let cmd = grep_command(false, "test.*\\.ts", ".", "");
    for dir in ["node_modules", ".git", "target", ".venv", "dist", "build"] {
        assert!(cmd.contains(&format!("--exclude-dir={dir}")), "{cmd}");
    }
    let rg = grep_command(true, "x", ".", "*.rs");
    assert!(rg.starts_with("rg "), "{rg}");
    assert!(rg.contains("--glob"), "{rg}");
}
