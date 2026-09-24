//! Tests for WorkingSet data structures and Invariant I state partitioning.

use superstate_agent::state::{Orchestrator, State};
use superstate_agent::working_set::WorkingSet;

#[test]
fn test_working_set_add_and_update() {
    let mut ws = WorkingSet::default();
    ws.add_or_update("src/main.rs", "fn main() {}\n", Some(1), Some(1), 1);

    assert_eq!(ws.files.len(), 1);
    assert_eq!(ws.files[0].path, "src/main.rs");
    assert_eq!(ws.files[0].content, "fn main() {}\n");
    assert_eq!(ws.files[0].last_accessed_turn, 1);

    // Update existing file on later turn
    ws.add_or_update(
        "src/main.rs",
        "fn main() { println!(); }\n",
        Some(1),
        Some(1),
        3,
    );
    assert_eq!(ws.files.len(), 1);
    assert_eq!(ws.files[0].content, "fn main() { println!(); }\n");
    assert_eq!(ws.files[0].last_accessed_turn, 3);
}

#[test]
fn test_working_set_apply_edit() {
    let mut ws = WorkingSet::default();
    ws.add_or_update(
        "lru.py",
        "def get(self, key):\n    return self.cache[key]\n",
        None,
        None,
        1,
    );

    let edited = ws.apply_edit(
        "lru.py",
        "return self.cache[key]",
        "self.cache.move_to_end(key)\n    return self.cache[key]",
        2,
    );
    assert!(edited);
    assert_eq!(ws.files[0].last_accessed_turn, 2);
    assert!(ws.files[0].content.contains("self.cache.move_to_end(key)"));
}

#[test]
fn test_working_set_lru_eviction() {
    let mut ws = WorkingSet::default();
    ws.add_or_update("a.py", "line 1\n", None, None, 1);
    ws.add_or_update("b.py", "line 2\n", None, None, 2);
    ws.add_or_update("c.py", "line 3\n", None, None, 3);

    // Access a.py on turn 4 so b.py becomes the oldest (turn 2)
    ws.add_or_update("a.py", "line 1 updated\n", None, None, 4);

    // Adding 4th file should evict b.py
    ws.add_or_update("d.py", "line 4\n", None, None, 5);
    ws.prune_lru(3, 250);

    assert_eq!(ws.files.len(), 3);
    let paths: Vec<&str> = ws.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"a.py"));
    assert!(paths.contains(&"c.py"));
    assert!(paths.contains(&"d.py"));
    assert!(!paths.contains(&"b.py"));

    // Verify line budget eviction:
    // Three files with 90 lines each (total 270 > MAX_TOTAL_LINES 250)
    let mut ws_lines = WorkingSet::default();
    ws_lines.add_or_update("f1.py", &"line\n".repeat(90), None, None, 1);
    ws_lines.add_or_update("f2.py", &"line\n".repeat(90), None, None, 2);
    assert_eq!(ws_lines.files.len(), 2);

    // Adding f3.py pushes total to 270 lines, triggering eviction of f1.py
    ws_lines.add_or_update("f3.py", &"line\n".repeat(90), None, None, 3);
    assert_eq!(ws_lines.files.len(), 2);
    let line_paths: Vec<&str> = ws_lines.files.iter().map(|f| f.path.as_str()).collect();
    assert!(!line_paths.contains(&"f1.py"));
    assert!(line_paths.contains(&"f2.py"));
    assert!(line_paths.contains(&"f3.py"));
}

#[test]
fn test_working_set_failure_lifecycle() {
    let mut ws = WorkingSet::default();
    assert!(ws.last_failure.is_none());

    ws.record_failure("FAIL: test_case\nAssertionError: 2 != -1\n");
    assert!(ws.last_failure.is_some());
    assert!(ws.last_failure.as_ref().unwrap().contains("AssertionError"));

    ws.clear_failure();
    assert!(ws.last_failure.is_none());
}

#[test]
fn test_invariant_i_agent_patch_cannot_mutate_working_set() {
    let mut state = State {
        orchestrator: Orchestrator {
            goal: "Test".into(),
            verification_cmd: "cargo test".into(),
            review: false,
        },
        runtime: Default::default(),
        agent: Default::default(),
    };

    state
        .runtime
        .working_set
        .add_or_update("safe.rs", "fn safe() {}", None, None, 1);
    state.runtime.working_set.record_failure("test failure");

    // Agent attempts to clear working_set or tamper with runtime
    state.apply_agent_patch(&serde_json::json!({
        "working_set": null,
        "runtime": null
    }));

    // Working set remains intact in Runtime (R)
    assert_eq!(state.runtime.working_set.files.len(), 1);
    assert_eq!(state.runtime.working_set.files[0].path, "safe.rs");
    assert!(state.runtime.working_set.last_failure.is_some());
}

#[test]
fn test_working_set_snippet_distillation() {
    let mut ws = WorkingSet::default();
    let content: String = (1..=120).map(|i| format!("line {i}\n")).collect();
    ws.add_or_update("long.rs", &content, Some(1), Some(120), 1);

    assert_eq!(ws.files.len(), 1);
    let snippet = &ws.files[0];
    assert!(snippet.content.contains("line 1\n"));
    assert!(snippet.content.contains("line 50\n"));
    assert!(snippet.content.contains("[... 20 lines omitted ...]"));
    assert!(snippet.content.contains("line 71\n"));
    assert!(snippet.content.contains("line 120"));
    assert!(!snippet.content.contains("line 60\n"));
}

#[test]
fn test_working_set_failure_distillation() {
    let mut ws = WorkingSet::default();
    let failure_output: String = (1..=30).map(|i| format!("FAIL error {i}\n")).collect();
    ws.record_failure(&failure_output);

    let recorded = ws.last_failure.as_ref().expect("failure should be present");
    assert!(recorded.contains("FAIL error 1"));
    assert!(recorded.contains("FAIL error 10"));
    assert!(recorded.contains("...\nFAIL error 21"));
    assert!(recorded.contains("FAIL error 30"));
    assert!(!recorded.contains("FAIL error 15"));
}

#[test]
fn test_working_set_apply_edit_edge_cases() {
    let mut ws = WorkingSet::default();
    ws.add_or_update(
        "edit_target.rs",
        "fn foo() {\n    line 1\n    line 2\n}\n",
        Some(1),
        Some(4),
        1,
    );

    // Empty old string should return false without altering content
    let empty_res = ws.apply_edit("edit_target.rs", "", "injected\n", 2);
    assert!(!empty_res);
    assert!(!ws.files[0].content.contains("injected"));

    // Non-existent file should return false
    let missing_file_res = ws.apply_edit("non_existent.rs", "line 1", "line 1 modified", 2);
    assert!(!missing_file_res);

    // Substring not present should return false
    let missing_chunk_res =
        ws.apply_edit("edit_target.rs", "not present in file", "replacement", 2);
    assert!(!missing_chunk_res);

    // Replacement that introduces lines should adjust end_line
    let expansion_res = ws.apply_edit(
        "edit_target.rs",
        "line 1\n    line 2",
        "line 1\n    line 1.5\n    line 2",
        2,
    );
    assert!(expansion_res);
    assert_eq!(ws.files[0].end_line, Some(5));

    // Large edit expansion should trigger snippet distillation and LRU pruning
    let huge_replacement: String = (1..=150).map(|i| format!("new_line {i}\n")).collect();
    let huge_res = ws.apply_edit(
        "edit_target.rs",
        "line 1\n    line 1.5\n    line 2",
        &huge_replacement,
        3,
    );
    assert!(huge_res);
    assert!(ws.files[0].content.contains("[..."));

    // EOF fallback: targets suffix rather than earlier occurrences in full-file snippets
    let mut ws2 = WorkingSet::default();
    ws2.add_or_update(
        "eof_test.rs",
        "let val = 42; // first\nlet val = 42",
        None,
        None,
        1,
    );
    let eof_res = ws2.apply_edit("eof_test.rs", "let val = 42\n", "let val = 99\n", 2);
    assert!(eof_res);
    assert_eq!(
        ws2.files[0].content, "let val = 42; // first\nlet val = 99\n",
        "EOF fallback must replace only the suffix, never an earlier occurrence"
    );

    // Newline-only old string must not match via EOF fallback on snippet without newlines
    let mut ws_no_newline = WorkingSet::default();
    ws_no_newline.add_or_update("no_newline.rs", "let val = 42", None, None, 1);
    let newline_only_res = ws_no_newline.apply_edit("no_newline.rs", "\n", "prepended\n", 2);
    assert!(
        !newline_only_res,
        "Newline-only string must not match via EOF fallback on snippet without newlines"
    );
    assert_eq!(ws_no_newline.files[0].content, "let val = 42");

    // Multiple trailing newlines in old on snippet ending in single newline must not match
    let mut ws_trailing = WorkingSet::default();
    ws_trailing.add_or_update("trailing.rs", "code\n", None, None, 1);
    let multi_nl_res = ws_trailing.apply_edit("trailing.rs", "code\n\n", "new\n", 2);
    assert!(
        !multi_nl_res,
        "Extra trailing newlines must not trigger fallback if content already ends with newline"
    );

    // Partial read slices (end_line is Some) must NOT trigger the EOF fallback
    let mut ws3 = WorkingSet::default();
    ws3.add_or_update("partial.rs", "line 1\nline 2", Some(1), Some(2), 1);
    let partial_eof_res = ws3.apply_edit("partial.rs", "line 2\n", "line 2 modified\n", 2);
    assert!(
        !partial_eof_res,
        "Partial read slices must not trigger full-file EOF fallback"
    );
}
