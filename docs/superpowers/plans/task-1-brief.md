# Task 1: Add ActionEntry and Runtime.actions to crates/superstate-agent/src/state.rs

Working Directory: `/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness`

## What to Implement

In `crates/superstate-agent/src/state.rs`:
1. Define `ActionEntry`:
```rust
/// A single action observed and summarized objectively by the harness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionEntry {
    /// Turn number when this action occurred.
    pub turn: u32,
    /// Name of the tool invoked (e.g. "read", "write", "edit", "bash", "glob", "grep", "note", "rollback").
    pub tool: String,
    /// Canonical target or command (e.g. "stats.py (lines 1-30)", "cargo test", "pattern 'mean'").
    pub target: String,
    /// Short 1-line summary of the outcome (e.g. "22 lines read", "exit 1: ZeroDivisionError", "replaced 3 lines").
    pub summary: String,
    /// Whether the action resulted in an error or non-zero exit code.
    pub is_error: bool,
}
```
2. In `pub struct Runtime`, add:
```rust
    /// Chronological log of all actions executed in this session.
    #[serde(default)]
    pub actions: Vec<ActionEntry>,
```

## Testing Requirements (TDD)

In `crates/superstate-agent/tests/test_state.rs`:
Add three tests:
1. `test_action_entry_serialization`:
   Create an `ActionEntry`, serialize to JSON, deserialize back, and assert equality.
2. `test_runtime_actions_default_backward_compatibility`:
   Deserialize a JSON string for `Runtime` that omits `"actions"` (e.g. `r#"{"attempts":1,"failures":[],"rollbacks":0,"files_written":[]}"#`), and assert that `runtime.actions.is_empty()`.
3. `test_agent_patch_cannot_mutate_runtime_actions`:
   Create a `State`, add an `ActionEntry` to `state.runtime.actions`, call `state.apply_agent_patch(&serde_json::json!({"actions": []}))` or `&serde_json::json!({"actions": "overwritten"})`, and assert that `state.runtime.actions` retains the original entry.

## Execution Steps
1. Write the tests in `crates/superstate-agent/tests/test_state.rs`.
2. Run `cargo test -p superstate-agent --test test_state` to observe failure.
3. Update `crates/superstate-agent/src/state.rs`.
4. Run `cargo test -p superstate-agent --test test_state` and `cargo check -p superstate-agent` to verify success.
5. Commit: `git add crates/superstate-agent/src/state.rs crates/superstate-agent/tests/test_state.rs && git commit --no-gpg-sign -m "feat(agent): add ActionEntry and actions ledger to Runtime state"`
6. Run `roborev status; roborev show HEAD`.
7. Write your completion report to `docs/superpowers/plans/task-1-report.md`.
