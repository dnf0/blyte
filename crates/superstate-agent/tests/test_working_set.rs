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
    ws.add_or_update("src/main.rs", "fn main() { println!(); }\n", Some(1), Some(1), 3);
    assert_eq!(ws.files.len(), 1);
    assert_eq!(ws.files[0].content, "fn main() { println!(); }\n");
    assert_eq!(ws.files[0].last_accessed_turn, 3);
}

#[test]
fn test_working_set_apply_edit() {
    let mut ws = WorkingSet::default();
    ws.add_or_update("lru.py", "def get(self, key):\n    return self.cache[key]\n", None, None, 1);

    let edited = ws.apply_edit("lru.py", "return self.cache[key]", "self.cache.move_to_end(key)\n    return self.cache[key]", 2);
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
        },
        runtime: Default::default(),
        agent: Default::default(),
    };

    state.runtime.working_set.add_or_update("safe.rs", "fn safe() {}", None, None, 1);
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
