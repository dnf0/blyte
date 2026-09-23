use serde_json::json;
use superstate_agent::run::{extract_action_summary, extract_action_target};

#[test]
fn test_read_target_and_summary() {
    // 1. read: with and without line numbers
    let target_no_lines = extract_action_target("read", &json!({"path": "src/main.rs"}));
    assert_eq!(target_no_lines, "src/main.rs");

    let target_with_lines = extract_action_target(
        "read",
        &json!({"path": "src/main.rs", "start_line": 10, "end_line": 25}),
    );
    assert_eq!(target_with_lines, "src/main.rs (L10-L25)");

    // read summary: line count
    let summary_lines =
        extract_action_summary("read", "line 1\nline 2\nline 3\n", false);
    assert_eq!(summary_lines, "3 lines read");

    let summary_empty = extract_action_summary("read", "", false);
    assert_eq!(summary_empty, "0 lines read");
}

#[test]
fn test_write_target_and_summary() {
    // 2. write: target includes bytes, summary is "file written"
    let target_with_content = extract_action_target(
        "write",
        &json!({"path": "config.json", "content": "hello world"}),
    );
    assert_eq!(target_with_content, "config.json (11 bytes)");

    let target_missing_content =
        extract_action_target("write", &json!({"path": "config.json"}));
    assert_eq!(target_missing_content, "config.json");

    let summary_ok = extract_action_summary("write", "wrote config.json", false);
    assert_eq!(summary_ok, "file written");

    let summary_err = extract_action_summary("write", "failed to write", true);
    assert_eq!(summary_err, "file written");
}

#[test]
fn test_edit_target_and_summary() {
    // 3. edit: success ("string replaced") and failure ("failed: string not found")
    let target = extract_action_target(
        "edit",
        &json!({"path": "src/lib.rs", "old": "foo", "new": "bar"}),
    );
    assert_eq!(target, "src/lib.rs");

    let summary_success = extract_action_summary("edit", "edited src/lib.rs", false);
    assert_eq!(summary_success, "string replaced");

    let summary_failure = extract_action_summary(
        "edit",
        "`old` not found in src/lib.rs. Read it first.\nStack trace...",
        true,
    );
    assert_eq!(
        summary_failure,
        "failed: `old` not found in src/lib.rs. Read it first."
    );

    let summary_failure_empty = extract_action_summary("edit", "", true);
    assert_eq!(summary_failure_empty, "failed: edit failed");
}

#[test]
fn test_bash_target_and_summary() {
    // 4. bash: short command and long command truncation (>60 chars); success ("exit 0") and failure (last non-empty line extracted)
    let short_cmd = "cargo check";
    let target_short = extract_action_target("bash", &json!({"command": short_cmd}));
    assert_eq!(target_short, "cargo check");

    let exact_60 = "123456789012345678901234567890123456789012345678901234567890";
    assert_eq!(exact_60.chars().count(), 60);
    let target_60 = extract_action_target("bash", &json!({"command": exact_60}));
    assert_eq!(target_60, exact_60);

    let long_cmd = "cargo test --workspace --all-targets --all-features --no-fail-fast -- --nocapture";
    assert!(long_cmd.chars().count() > 60);
    let expected_truncated = format!("{}...", &long_cmd[..57]);
    let target_long = extract_action_target("bash", &json!({"command": long_cmd}));
    assert_eq!(target_long, expected_truncated);

    // Bash summary success
    let summary_ok = extract_action_summary("bash", "exit 0\nEverything is fine", false);
    assert_eq!(summary_ok, "exit 0");

    // Bash summary failure
    let summary_err = extract_action_summary(
        "bash",
        "exit 1\nthread 'main' panicked\nassertion failed: left == right\n\n",
        true,
    );
    assert_eq!(summary_err, "failed: assertion failed: left == right");

    // Bash summary failure clamped to 80 chars
    let long_err_line = "x".repeat(100);
    let summary_clamped = extract_action_summary(
        "bash",
        &format!("exit 1\n{long_err_line}"),
        true,
    );
    assert_eq!(summary_clamped, format!("failed: {}", "x".repeat(80)));

    let summary_err_empty = extract_action_summary("bash", "", true);
    assert_eq!(summary_err_empty, "failed: command failed");
}

#[test]
fn test_glob_target_and_summary() {
    // 5. glob: file count matching
    let target = extract_action_target("glob", &json!({"pattern": "**/*.rs"}));
    assert_eq!(target, "**/*.rs");

    let summary = extract_action_summary(
        "glob",
        "src/lib.rs\nsrc/main.rs\nsrc/run.rs\n",
        false,
    );
    assert_eq!(summary, "3 files matched");

    let summary_empty = extract_action_summary("glob", "", false);
    assert_eq!(summary_empty, "0 files matched");
}

#[test]
fn test_grep_target_and_summary() {
    // 6. grep: pattern and path, match count
    let target_with_path =
        extract_action_target("grep", &json!({"pattern": "TODO", "path": "src"}));
    assert_eq!(target_with_path, "pattern 'TODO' in src");

    let target_without_path =
        extract_action_target("grep", &json!({"pattern": "TODO"}));
    assert_eq!(target_without_path, "pattern 'TODO'");

    let summary = extract_action_summary(
        "grep",
        "src/lib.rs:1:TODO\nsrc/run.rs:10:TODO\n",
        false,
    );
    assert_eq!(summary, "2 matches found");

    let summary_empty = extract_action_summary("grep", "", false);
    assert_eq!(summary_empty, "0 matches found");
}

#[test]
fn test_note_target_and_summary() {
    // 7. note: key count, status string
    let target = extract_action_target(
        "note",
        &json!({"patch": {"findings": "root cause", "fixed": true}}),
    );
    assert_eq!(target, "patch (2 keys)");

    let target_empty = extract_action_target("note", &json!({"patch": {}}));
    assert_eq!(target_empty, "patch (0 keys)");

    let summary = extract_action_summary(
        "note",
        "recorded; your state now has 3 field(s)\nmore info",
        false,
    );
    assert_eq!(summary, "recorded; your state now has 3 field(s)");

    let summary_empty = extract_action_summary("note", "", false);
    assert_eq!(summary_empty, "recorded");
}

#[test]
fn test_fallback_target_and_summary() {
    // Fallback target: first string argument value found or tool name
    let target_string_arg =
        extract_action_target("custom_tool", &json!({"query": "SELECT 1"}));
    assert_eq!(target_string_arg, "SELECT 1");

    let target_no_string_arg =
        extract_action_target("custom_tool", &json!({"count": 42}));
    assert_eq!(target_no_string_arg, "custom_tool");

    let target_null_arg = extract_action_target("custom_tool", &json!(null));
    assert_eq!(target_null_arg, "custom_tool");

    // Fallback summary: first line clamped to 80 chars
    let summary_first_line = extract_action_summary(
        "custom_tool",
        "first line of result\nsecond line",
        false,
    );
    assert_eq!(summary_first_line, "first line of result");

    let long_line = "a".repeat(120);
    let summary_clamped = extract_action_summary("custom_tool", &long_line, false);
    assert_eq!(summary_clamped, "a".repeat(80));

    let summary_empty = extract_action_summary("custom_tool", "", false);
    assert_eq!(summary_empty, "");
}
