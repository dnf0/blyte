//! Verification of state serialization and runtime action logging invariants.
//!
//! Validates that `ActionEntry` serializes and deserializes accurately,
//! that legacy runtime states omitting `actions` deserialize cleanly via defaults,
//! and that agent patches cannot tamper with or mutate the observed actions ledger.

use blyte_agent::state::{ActionEntry, Runtime, State};
use serde_json::json;

/// Verifies that an ActionEntry round-trips through JSON serialization without loss.
#[test]
fn test_action_entry_serialization() {
    // Construct an action entry with representative tool execution metadata.
    let entry = ActionEntry {
        turn: 1,
        tool: "read".to_string(),
        target: "src/main.rs (lines 1-10)".to_string(),
        summary: "10 lines read".to_string(),
        is_error: false,
    };

    // Serialize to JSON and parse back to verify round-trip fidelity.
    let serialized = serde_json::to_string(&entry).expect("serialization should succeed");
    let deserialized: ActionEntry =
        serde_json::from_str(&serialized).expect("deserialization should succeed");

    assert_eq!(entry, deserialized);
}

/// Verifies backward compatibility when deserializing Runtime instances that lack the `actions` field.
#[test]
fn test_runtime_actions_default_backward_compatibility() {
    // JSON payload representing state written by an earlier version without the actions ledger.
    let legacy_json = r#"{"attempts":1,"failures":[],"rollbacks":0,"files_written":[]}"#;

    // Deserialization must succeed, using #[serde(default)] to supply an empty vector.
    let runtime: Runtime =
        serde_json::from_str(legacy_json).expect("deserializing legacy JSON should succeed");

    assert!(runtime.actions.is_empty());
}

/// Verifies that agent patches cannot overwrite or corrupt the runtime actions ledger.
#[test]
fn test_agent_patch_cannot_mutate_runtime_actions() {
    // Initialize state with an observed action entry in the runtime ledger.
    let mut state = State::new("implement feature", "cargo test");
    let initial_entry = ActionEntry {
        turn: 1,
        tool: "bash".to_string(),
        target: "cargo test".to_string(),
        summary: "exit 1: test failure".to_string(),
        is_error: true,
    };
    state.runtime.actions.push(initial_entry.clone());

    // Agent attempts to clear or overwrite the runtime actions ledger via patch.
    state.apply_agent_patch(&json!({"actions": []}));
    assert_eq!(
        state.runtime.actions.len(),
        1,
        "runtime actions ledger must be immutable to agent patches"
    );
    assert_eq!(state.runtime.actions[0], initial_entry);

    // Ensure poisoned key lands inertly in the agent region without altering runtime.
    assert_eq!(state.agent.fields["actions"], json!([]));
}
