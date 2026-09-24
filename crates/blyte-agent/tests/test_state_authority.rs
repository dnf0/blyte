//! The orchestrator region must be unreachable from the agent, structurally.

use blyte_agent::state::State;
use serde_json::json;

#[test]
fn an_agent_patch_cannot_touch_the_goal_or_the_gate() {
    // The load-bearing test. Blyte asserted "the orchestrator states the verification
    // command" as doctrine for months while the code let a worker-authored test become the
    // gate. A patch naming those fields must land nowhere at all.
    let mut s = State::new("fix the bug", "npm test");
    s.apply_agent_patch(&json!({
        "goal": "declare victory",
        "verification_cmd": "true",
        "hypothesis": "the parser drops spans",
    }));
    assert_eq!(s.orchestrator.goal, "fix the bug");
    assert_eq!(s.orchestrator.verification_cmd, "npm test");
    // The legitimate key still applies -- rejection is scoped, not a blanket refusal.
    assert_eq!(s.agent.fields["hypothesis"], "the parser drops spans");
    // And the poisoned keys land in the agent's own region, where they are inert.
    assert_eq!(s.agent.fields["goal"], "declare victory");
}

#[test]
fn an_agent_patch_cannot_forge_runtime_observations() {
    // Failures and rollbacks are observed, never reported. A worker that could write
    // `failures: []` could erase the evidence of its own failed attempts.
    let mut s = State::new("g", "make test");
    s.runtime.failures.push("exit 1: 3 tests failed".into());
    s.apply_agent_patch(&json!({"failures": [], "rollbacks": 0, "attempts": 0}));
    assert_eq!(
        s.runtime.failures.len(),
        1,
        "observed failures must survive any agent patch"
    );
    assert_eq!(s.last_failure(), Some("exit 1: 3 tests failed"));
}

#[test]
fn null_deletes_a_key_per_skill_state_merge_semantics() {
    let mut s = State::new("g", "v");
    s.apply_agent_patch(&json!({"a": 1, "b": 2}));
    s.apply_agent_patch(&json!({"a": null}));
    assert!(!s.agent.fields.contains_key("a"));
    assert_eq!(s.agent.fields["b"], 2);
}

#[test]
fn a_non_object_patch_is_ignored_rather_than_corrupting_state() {
    let mut s = State::new("g", "v");
    s.apply_agent_patch(&json!({"keep": 1}));
    for bad in [json!("string"), json!([1, 2]), json!(null), json!(7)] {
        s.apply_agent_patch(&bad);
    }
    assert_eq!(s.agent.fields["keep"], 1);
    assert_eq!(s.agent.fields.len(), 1);
}

#[test]
fn state_round_trips_through_json() {
    let mut s = State::new("goal", "cargo test");
    s.runtime.failures.push("boom".into());
    s.apply_agent_patch(&json!({"note": "x"}));
    let back: State = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back, s);
}
