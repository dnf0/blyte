//! `compact`: `folded` plus exact reductions of tool output. Superseded outputs are masked (in
//! completed epochs only), duplicates of visible outputs are referenced, and calls are never lost.

use serde_json::{json, Value};
use superstate_agent::context::{Compact, ContextStrategy, Conversational};
use superstate_agent::state::State;
use superstate_provider::Message;

fn call(n: usize, name: &str, args: Value) -> Message {
    Message::ToolCall {
        id: format!("c{n}"),
        name: name.into(),
        arguments: args,
        signature: None,
    }
}

fn res(n: usize, name: &str, content: &str) -> Message {
    Message::ToolResult {
        id: format!("c{n}"),
        name: name.into(),
        content: content.into(),
        is_error: false,
    }
}

fn open() -> Message {
    Message::User("Begin. fix it".into())
}

fn content(m: &Message) -> &str {
    match m {
        Message::ToolResult { content, .. } => content,
        other => panic!("expected a tool result, got {other:?}"),
    }
}

fn compact(epoch: usize) -> Compact {
    Compact {
        budget_tokens: 1_000_000,
        epoch_messages: epoch,
        min_dedup_chars: 200,
    }
}

fn st() -> State {
    State::new("fix it", "")
}

#[test]
fn a_duplicate_of_a_visible_output_is_referenced_not_repeated() {
    let big = "x".repeat(500);
    let h = vec![
        open(),
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &big),
        call(2, "read", json!({"path": "b.rs"})),
        res(2, "read", &"y".repeat(300)),
        call(3, "bash", json!({"command": "cat a.rs"})),
        res(3, "bash", &big),
    ];
    let out = compact(1_000).build(&st(), &h);
    assert_eq!(content(&out[2]), big, "the first occurrence stays");
    assert!(
        content(&out[6]).starts_with("[identical to the earlier output of `read` `a.rs` above"),
        "{}",
        content(&out[6])
    );
}

#[test]
fn short_outputs_are_never_deduplicated() {
    let h = vec![
        open(),
        call(1, "bash", json!({"command": "echo hi"})),
        res(1, "bash", "hi"),
        call(2, "bash", json!({"command": "echo hi again"})),
        res(2, "bash", "hi"),
    ];
    let out = compact(1_000).build(&st(), &h);
    assert_eq!(content(&out[4]), "hi");
}

#[test]
fn an_edit_supersedes_an_earlier_read_once_its_epoch_completes() {
    let big = "line\n".repeat(200);
    let h = vec![
        open(),
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &big),
        call(2, "edit", json!({"path": "a.rs", "old": "x", "new": "y"})),
        res(2, "edit", "replaced 1 line"),
        call(3, "bash", json!({"command": "ls"})),
        res(3, "bash", "a.rs"),
    ];
    let out = compact(4).build(&st(), &h);
    assert!(
        content(&out[2]).contains("a.rs changed later"),
        "{}",
        content(&out[2])
    );
    assert!(
        matches!(&out[1], Message::ToolCall { name, .. } if name == "read"),
        "the call is kept"
    );
}

#[test]
fn nothing_is_masked_before_its_epoch_completes() {
    let big = "line\n".repeat(200);
    let h = vec![
        open(),
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &big),
        call(2, "edit", json!({"path": "a.rs", "old": "x", "new": "y"})),
        res(2, "edit", "replaced 1 line"),
    ];
    let out = compact(100).build(&st(), &h);
    assert_eq!(content(&out[2]), big);
}

#[test]
fn an_identical_command_rerun_supersedes_the_earlier_output() {
    let h = vec![
        open(),
        call(1, "bash", json!({"command": "pytest -q"})),
        res(1, "bash", &format!("FAILED {}", "f".repeat(300))),
        call(2, "bash", json!({"command": "pytest -q"})),
        res(2, "bash", &format!("passed {}", "p".repeat(300))),
        call(3, "read", json!({"path": "z.rs"})),
        res(3, "read", "z"),
    ];
    let out = compact(4).build(&st(), &h);
    assert!(
        content(&out[2]).contains("later run of the same call"),
        "{}",
        content(&out[2])
    );
    assert!(
        content(&out[4]).starts_with("passed"),
        "the latest run stays"
    );
}

#[test]
fn a_read_of_a_different_range_does_not_supersede() {
    let big = "line\n".repeat(200);
    let h = vec![
        open(),
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &big),
        call(
            2,
            "read",
            json!({"path": "a.rs", "start_line": 10, "end_line": 20}),
        ),
        res(2, "read", "slice"),
        call(3, "bash", json!({"command": "ls"})),
        res(3, "bash", "a.rs"),
    ];
    let out = compact(4).build(&st(), &h);
    assert_eq!(content(&out[2]), big);
}

#[test]
fn the_prompt_only_changes_at_epoch_boundaries() {
    let c = Compact {
        budget_tokens: 1_000_000,
        epoch_messages: 8,
        min_dedup_chars: 20,
    };
    let mut h = vec![open()];
    for t in 0..30 {
        let (name, args) = match t % 3 {
            0 => ("read", json!({"path": format!("f{}.rs", t % 4)})),
            1 => (
                "edit",
                json!({"path": format!("f{}.rs", t % 4), "old": "a", "new": "b"}),
            ),
            _ => ("bash", json!({"command": "pytest -q"})),
        };
        h.push(call(t, name, args));
        h.push(res(
            t,
            name,
            &format!("output of {name} {}", "z".repeat(40 + t % 2)),
        ));
    }
    let mut changes = 0;
    for n in 2..h.len() {
        let a = compact_build(&c, &h[..n]);
        let b = compact_build(&c, &h[..n + 1]);
        if b.len() >= a.len() && b[..a.len()] == a[..] {
            continue;
        }
        assert_ne!((n - 1) / 8, n / 8, "the prompt changed mid-epoch at n={n}");
        changes += 1;
    }
    assert!(changes > 0, "masks should apply at some epoch boundary");
}

fn compact_build(c: &Compact, h: &[Message]) -> Vec<Message> {
    c.build(&st(), h)
}

#[test]
fn with_nothing_to_reduce_it_is_exactly_conversational() {
    let h = vec![
        open(),
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &"a".repeat(300)),
        call(2, "read", json!({"path": "b.rs"})),
        res(2, "read", &"b".repeat(300)),
    ];
    assert_eq!(compact(4).build(&st(), &h), Conversational.build(&st(), &h));
    assert_eq!(Compact::default().epoch_messages, 40);
    assert_eq!(Compact::default().budget_tokens, 48_000);
    assert_eq!(Compact::default().min_dedup_chars, 200);
    assert_eq!(Compact::default().name(), "compact");
}
