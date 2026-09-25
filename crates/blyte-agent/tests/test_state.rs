//! Verification of state serialization and runtime invariants.

use blyte_agent::state::{Runtime, State};
use serde_json::json;

/// Verifies backward compatibility when deserializing Runtime instances.
#[test]
fn test_runtime_default_backward_compatibility() {
    let legacy_json = r#"{"attempts":1,"failures":[],"rollbacks":0,"files_written":[]}"#;
    let runtime: Runtime =
        serde_json::from_str(legacy_json).expect("deserializing legacy JSON should succeed");
    assert_eq!(runtime.attempts, 1);
}

/// Verifies that agent patches cannot overwrite or corrupt the runtime region.
#[test]
fn test_agent_patch_cannot_mutate_runtime() {
    let mut state = State::new("implement feature", "cargo test");
    state.runtime.attempts = 1;

    // Agent attempts to clear or overwrite the runtime attempts via patch.
    state.apply_agent_patch(&json!({"attempts": 99}));
    assert_eq!(
        state.runtime.attempts, 1,
        "runtime region must be immutable to agent patches"
    );

    // Ensure poisoned key lands inertly in the agent region without altering runtime.
    assert_eq!(state.agent.fields["attempts"], json!(99));
}
