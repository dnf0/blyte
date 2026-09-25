//! Verification of state serialization and runtime invariants.

use blyte_agent::state::Runtime;

/// Verifies backward compatibility when deserializing Runtime instances.
#[test]
fn test_runtime_default_backward_compatibility() {
    let legacy_json = r#"{"attempts":1,"failures":[],"rollbacks":0,"files_written":[]}"#;
    let runtime: Runtime =
        serde_json::from_str(legacy_json).expect("deserializing legacy JSON should succeed");
    assert_eq!(runtime.rollbacks, 0);
}
