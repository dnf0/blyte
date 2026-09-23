//! The loop-breaker may only claim what an exit code actually shows.
//!
//! Regression for the 2026-09-23 native TDD smoke run: a compile-only `go test -run '^$'`
//! exiting 0 made the guard and the prompt tell a GREEN worker its failing test was passing,
//! and the worker never edited a file.

use serde_json::json;
use superstate_agent::context::{ContextStrategy, StateOnly};
use superstate_agent::run::{
    current_phase_actions, extract_action_target, should_intercept_redundant_bash,
    verification_passed_since_edit, PHASE_MARKER_TOOL,
};
use superstate_agent::state::{ActionEntry, State};
use superstate_agent::working_set::WorkingSet;
use superstate_provider::{Message, ToolCall};

fn act(tool: &str, target: &str, summary: &str) -> ActionEntry {
    ActionEntry {
        turn: 1,
        tool: tool.into(),
        target: target.into(),
        summary: summary.into(),
        is_error: false,
    }
}

fn marker(id: &str) -> ActionEntry {
    act(PHASE_MARKER_TOOL, id, "phase started")
}

fn bash(cmd: &str) -> ToolCall {
    ToolCall {
        id: "call_bash".into(),
        name: "bash".into(),
        arguments: json!({"command": cmd}),
        signature: None,
    }
}

fn prompt_text(st: &State) -> String {
    StateOnly
        .build(st, &[])
        .iter()
        .map(|m| match m {
            Message::User(t) => t.clone(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_different_test_command_passing_does_not_block_this_one() {
    let actions = vec![
        act("write", "reproduce_issue_test.go", "wrote 95 lines"),
        act("bash", "go test -run '^$' ./...", "exit 0"),
    ];
    let guard = should_intercept_redundant_bash(
        &bash("go test -run TestReproduceIssue ./..."),
        &actions,
        &WorkingSet::default(),
        "",
    );
    assert!(guard.is_none(), "{guard:?}");
}

#[test]
fn the_identical_command_is_still_intercepted() {
    let actions = vec![
        act("write", "a.go", "wrote 3 lines"),
        act("bash", "go test ./...", "exit 0"),
    ];
    let msg = should_intercept_redundant_bash(
        &bash("go test ./..."),
        &actions,
        &WorkingSet::default(),
        "",
    )
    .expect("an identical passing command is intercepted");
    assert!(msg.contains("already exited 0"), "{msg}");
    assert!(!msg.contains("have already passed"), "{msg}");
}

#[test]
fn an_edit_since_the_pass_lets_it_run_again() {
    let actions = vec![
        act("write", "a.go", "wrote 3 lines"),
        act("bash", "go test ./...", "exit 0"),
        act("edit", "a.go", "replaced 1 line"),
    ];
    assert!(should_intercept_redundant_bash(
        &bash("go test ./..."),
        &actions,
        &WorkingSet::default(),
        ""
    )
    .is_none());
}

#[test]
fn a_truncated_command_is_never_treated_as_identical() {
    let long = "go test -run 'TestAVeryLongNameThatGoesOnAndOn|TestAnotherOne' ./...";
    let target = extract_action_target("bash", &json!({"command": long}));
    assert!(target.ends_with("..."), "{target}");
    let actions = vec![
        act("write", "a.go", "wrote 3 lines"),
        act("bash", &target, "exit 0"),
    ];
    assert!(
        should_intercept_redundant_bash(&bash(long), &actions, &WorkingSet::default(), "")
            .is_none()
    );
}

#[test]
fn a_pass_in_an_earlier_phase_does_not_carry_into_the_next() {
    let actions = vec![
        act("write", "a.go", "wrote 3 lines"),
        act("bash", "go test ./...", "exit 0"),
        marker("fix-implementation"),
    ];
    assert!(should_intercept_redundant_bash(
        &bash("go test ./..."),
        &actions,
        &WorkingSet::default(),
        ""
    )
    .is_none());
}

#[test]
fn current_phase_actions_start_after_the_latest_marker() {
    let actions = vec![
        act("bash", "a", "exit 0"),
        marker("red"),
        act("bash", "b", "exit 0"),
        marker("green"),
        act("bash", "c", "exit 0"),
    ];
    let current = current_phase_actions(&actions);
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].target, "c");
    assert_eq!(current_phase_actions(&actions[..1]).len(), 1);
}

#[test]
fn only_the_verification_command_itself_counts_as_verified() {
    let v = "go test ./...";
    let compile_only = vec![
        act("write", "a.go", "wrote 3 lines"),
        act("bash", "go test -run '^$' ./...", "exit 0"),
    ];
    assert!(!verification_passed_since_edit(&compile_only, v));

    let real = vec![
        act("write", "a.go", "wrote 3 lines"),
        act("bash", v, "exit 0"),
    ];
    assert!(verification_passed_since_edit(&real, v));
    assert!(!verification_passed_since_edit(&real, ""));

    let mut next_phase = real.clone();
    next_phase.push(marker("green"));
    assert!(!verification_passed_since_edit(&next_phase, v));
}

#[test]
fn inspection_advice_never_claims_tests_pass_from_a_compile_only_check() {
    let actions = vec![
        act("write", "reproduce_issue_test.go", "wrote 95 lines"),
        act("bash", "go test -run '^$' ./...", "exit 0"),
        act("bash", "git status", "exit 0"),
    ];
    let msg = should_intercept_redundant_bash(
        &bash("git status"),
        &actions,
        &WorkingSet::default(),
        "go test ./...",
    )
    .expect("an immediately repeated inspection is intercepted");
    assert!(!msg.contains("already passed"), "{msg}");
    assert!(!msg.contains("exited 0 after your last edit"), "{msg}");
}

#[test]
fn the_prompt_never_claims_tests_pass_from_a_compile_only_check() {
    let mut st = State::new("fix it", "go test ./...");
    st.runtime.actions = vec![
        act("write", "reproduce_issue_test.go", "wrote 95 lines"),
        act("bash", "go test -run '^$' ./...", "exit 0"),
    ];
    let text = prompt_text(&st);
    assert!(!text.contains("## Verification Status"), "{text}");

    st.runtime
        .actions
        .push(act("bash", "go test ./...", "exit 0"));
    let text = prompt_text(&st);
    assert!(
        text.contains("The verification command `go test ./...` exited 0 after your last edit"),
        "{text}"
    );
    assert!(!text.contains("Unit tests are passing"), "{text}");
}

#[test]
fn a_different_command_passing_does_not_erase_a_recorded_failure() {
    let mut ws = WorkingSet::default();
    ws.record_failure_for(
        "go test -run TestReproduceIssue ./...",
        "--- FAIL: TestReproduceIssue",
    );
    ws.clear_failure_for("go test -run '^$' ./...");
    assert!(ws.last_failure.is_some());
    ws.clear_failure_for("go test -run TestReproduceIssue ./...");
    assert!(ws.last_failure.is_none());
    assert!(ws.last_failure_cmd.is_none());
}
