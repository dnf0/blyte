//! `state-window`: state-only's bounded prompt plus the last few tool results.

use blyte_agent::context::{ContextStrategy, StateOnly, StateWindow};
use blyte_agent::state::State;
use blyte_provider::Message;
use serde_json::json;

fn call(n: usize) -> Message {
    Message::ToolCall {
        id: format!("c{n}"),
        name: "bash".into(),
        arguments: json!({"command": format!("grep -rn thing{n}")}),
        signature: None,
    }
}

fn result(n: usize, content: &str) -> Message {
    Message::ToolResult {
        id: format!("c{n}"),
        name: "bash".into(),
        content: content.into(),
        is_error: false,
    }
}

/// `n` call/result pairs after the opening message, as the run loop records them.
fn history(n: usize) -> Vec<Message> {
    let mut h = vec![Message::User("Begin. fix it".into())];
    for i in 1..=n {
        h.push(call(i));
        h.push(result(i, &format!("result {i}")));
    }
    h
}

fn ids(msgs: &[Message]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| match m {
            Message::ToolCall { id, .. } => Some(format!("call:{id}")),
            Message::ToolResult { id, .. } => Some(format!("result:{id}")),
            _ => None,
        })
        .collect()
}

fn window(results: usize, lines: usize) -> StateWindow {
    StateWindow {
        results,
        lines_per_result: lines,
    }
}

#[test]
fn keeps_the_last_results_in_order_and_drops_older_ones() {
    let st = State::new("fix it", "");
    let out = window(3, 60).build(&st, &history(10));
    assert!(
        matches!(out[0], Message::User(_)),
        "the state prompt comes first"
    );
    assert_eq!(
        ids(&out),
        vec![
            "call:c8",
            "result:c8",
            "call:c9",
            "result:c9",
            "call:c10",
            "result:c10"
        ]
    );
}

#[test]
fn long_results_are_cut_to_the_line_budget() {
    let st = State::new("fix it", "");
    let long: String = (1..=200).map(|i| format!("line {i}\n")).collect();
    let h = vec![Message::User("Begin.".into()), call(1), result(1, &long)];
    let out = window(8, 20).build(&st, &h);
    let Message::ToolResult { content, .. } = out.last().unwrap() else {
        panic!("expected the result last");
    };
    assert!(
        content.lines().count() < 30,
        "{} lines",
        content.lines().count()
    );
    assert!(content.contains("elided by blyte"));
}

#[test]
fn the_prompt_stays_bounded_however_long_the_session() {
    let st = State::new("fix it", "");
    let w = StateWindow::default();
    let out = w.build(&st, &history(500));
    assert!(out.len() <= 1 + 2 * w.results + 1, "{} messages", out.len());
    assert_eq!(w.results, 8);
    assert_eq!(w.lines_per_result, 60);
}

#[test]
fn gate_feedback_after_the_results_is_kept_last() {
    let st = State::new("fix it", "");
    let mut h = history(2);
    h.push(Message::User("Verification did not pass.\n\nexit 1".into()));
    let out = window(8, 60).build(&st, &h);
    match out.last().unwrap() {
        Message::User(t) => assert!(t.starts_with("Verification did not pass")),
        other => panic!("expected gate feedback last, got {other:?}"),
    }
    assert_eq!(ids(&out).len(), 4);
}

#[test]
fn every_result_of_a_multi_call_turn_is_visible() {
    let st = State::new("fix it", "");
    let h = vec![
        Message::User("Begin.".into()),
        call(1),
        result(1, "a"),
        call(2),
        result(2, "b"),
    ];
    let out = window(8, 60).build(&st, &h);
    assert_eq!(
        ids(&out),
        vec!["call:c1", "result:c1", "call:c2", "result:c2"]
    );
}

#[test]
fn the_first_turn_carries_the_opening_message() {
    let st = State::new("fix it", "");
    let out = window(8, 60).build(&st, &history(0));
    assert_eq!(out.len(), 2);
    assert!(matches!(&out[1], Message::User(t) if t.starts_with("Begin.")));
}

#[test]
fn state_only_is_unchanged_as_the_control() {
    let st = State::new("fix it", "");
    let out = StateOnly.build(&st, &history(10));
    assert_eq!(ids(&out), vec!["call:c10", "result:c10"]);
    assert_eq!(StateWindow::default().name(), "state-window");
}
