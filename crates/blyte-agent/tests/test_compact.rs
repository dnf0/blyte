//! `compact`: `folded` plus exact reductions of tool output. Superseded outputs are masked only
//! when a fold happens (the prompt is rewritten then anyway), duplicates of visible outputs are
//! referenced, and calls are never lost.

use blyte_agent::context::{Compact, ContextStrategy, Conversational};
use blyte_agent::state::State;
use blyte_provider::Message;
use serde_json::{json, Value};

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

fn compact(budget: usize) -> Compact {
    Compact {
        budget_tokens: budget,
        min_dedup_chars: 200,
    }
}

fn st() -> State {
    State::new("fix it", "")
}

fn digests(out: &[Message]) -> usize {
    out.iter()
        .filter(|m| matches!(m, Message::User(t) if t.starts_with("## Earlier turns, folded")))
        .count()
}

fn result_text<'a>(out: &'a [Message], needle: &str) -> Option<&'a str> {
    out.iter().find_map(|m| match m {
        Message::ToolResult { content, .. } if content.contains(needle) => Some(content.as_str()),
        _ => None,
    })
}

/// Twenty filler turns (~100 tokens each), then `tail`, then one large output that triggers a
/// fold, whose older half is filler, so `tail` stays verbatim.
fn with_fold(tail: Vec<Message>) -> Vec<Message> {
    let mut h = vec![open()];
    for t in 0..20 {
        h.push(call(
            100 + t,
            "bash",
            json!({"command": format!("echo filler {t}")}),
        ));
        h.push(res(
            100 + t,
            "bash",
            &format!("filler {t} {}", "f".repeat(400)),
        ));
    }
    h.extend(tail);
    h.push(call(900, "bash", json!({"command": "cat big.log"})));
    h.push(res(900, "bash", &"g".repeat(2_000)));
    h
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
    let out = compact(1_000_000).build(&st(), &h);
    assert!(matches!(&out[2], Message::ToolResult { content, .. } if *content == big));
    let Message::ToolResult { content, .. } = &out[6] else {
        panic!("expected a result");
    };
    assert!(
        content.starts_with("[identical to the earlier output of `read` `a.rs` above"),
        "{content}"
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
    let out = compact(1_000_000).build(&st(), &h);
    assert!(matches!(&out[4], Message::ToolResult { content, .. } if content == "hi"));
}

#[test]
fn an_edit_supersedes_an_earlier_read_when_a_fold_happens() {
    let h = with_fold(vec![
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &format!("READ_A {}", "r".repeat(700))),
        call(2, "edit", json!({"path": "a.rs", "old": "x", "new": "y"})),
        res(2, "edit", "replaced 1 line"),
    ]);
    let out = compact(2_400).build(&st(), &h);
    assert!(digests(&out) >= 1, "expected a fold");
    assert!(
        out.iter().any(|m| matches!(m, Message::ToolResult { content, .. } if content.contains("a.rs changed later"))),
        "the read must be masked"
    );
    assert!(
        result_text(&out, "READ_A").is_none(),
        "the stale content must be gone"
    );
    assert!(
        out.iter().any(|m| matches!(m, Message::ToolCall { name, arguments, .. } if name == "read" && arguments["path"] == "a.rs")),
        "the call is kept"
    );
}

#[test]
fn nothing_is_masked_before_the_first_fold() {
    let h = with_fold(vec![
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &format!("READ_A {}", "r".repeat(700))),
        call(2, "edit", json!({"path": "a.rs", "old": "x", "new": "y"})),
        res(2, "edit", "replaced 1 line"),
    ]);
    let out = compact(1_000_000).build(&st(), &h);
    assert_eq!(digests(&out), 0);
    assert!(result_text(&out, "READ_A").is_some());
}

#[test]
fn an_identical_rerun_supersedes_the_earlier_output_when_a_fold_happens() {
    let h = with_fold(vec![
        call(1, "bash", json!({"command": "pytest -q"})),
        res(1, "bash", &format!("FIRST_RUN {}", "p".repeat(700))),
        call(2, "bash", json!({"command": "pytest -q"})),
        res(2, "bash", &format!("SECOND_RUN {}", "q".repeat(700))),
    ]);
    let out = compact(2_600).build(&st(), &h);
    assert!(digests(&out) >= 1, "expected a fold");
    assert!(
        result_text(&out, "FIRST_RUN").is_none(),
        "the earlier run is masked"
    );
    assert!(
        result_text(&out, "SECOND_RUN").is_some(),
        "the latest run stays"
    );
}

#[test]
fn a_read_of_a_different_range_does_not_supersede() {
    let h = with_fold(vec![
        call(1, "read", json!({"path": "a.rs"})),
        res(1, "read", &format!("READ_A {}", "r".repeat(700))),
        call(
            2,
            "read",
            json!({"path": "a.rs", "start_line": 10, "end_line": 20}),
        ),
        res(2, "read", "slice"),
    ]);
    let out = compact(2_400).build(&st(), &h);
    assert!(digests(&out) >= 1, "expected a fold");
    assert!(result_text(&out, "READ_A").is_some());
}

#[test]
fn the_prompt_only_changes_when_a_fold_happens() {
    let c = Compact {
        budget_tokens: 1_500,
        min_dedup_chars: 20,
    };
    let mut h = vec![open()];
    for t in 0..60 {
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
            &format!("output of {name} {}", "z".repeat(150 + t % 2)),
        ));
    }
    let mut folds_seen = 0;
    for n in 2..h.len() {
        let a = c.build(&st(), &h[..n]);
        let b = c.build(&st(), &h[..n + 1]);
        if b.len() >= a.len() && b[..a.len()] == a[..] {
            continue;
        }
        assert_ne!(
            digests(&a),
            digests(&b),
            "the prompt changed without a fold at n={n}"
        );
        folds_seen += 1;
    }
    assert!(folds_seen > 0, "expected folds in this history");
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
    assert_eq!(
        compact(1_000_000).build(&st(), &h),
        Conversational.build(&st(), &h)
    );
    assert_eq!(Compact::default().budget_tokens, 48_000);
    assert_eq!(Compact::default().min_dedup_chars, 200);
    assert_eq!(Compact::default().name(), "compact");
}
