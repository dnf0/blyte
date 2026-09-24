//! `folded`: an append-only conversation, bounded by folding its oldest half into
//! harness-written digests once it passes a token budget.

use blyte_agent::context::{ContextStrategy, Conversational, Folded};
use blyte_agent::state::State;
use blyte_provider::Message;
use serde_json::json;

fn call(n: usize) -> Message {
    Message::ToolCall {
        id: format!("c{n}"),
        name: "bash".into(),
        arguments: json!({"command": format!("grep -rn thing{n} src")}),
        signature: Some(format!("sig{n}")),
    }
}

fn result(n: usize, body: &str) -> Message {
    Message::ToolResult {
        id: format!("c{n}"),
        name: "bash".into(),
        content: body.into(),
        is_error: false,
    }
}

/// Opening message, then `n` turns of: model text, a call, its result.
fn history(n: usize, result_chars: usize) -> Vec<Message> {
    let mut h = vec![Message::User("Begin. fix it".into())];
    for i in 1..=n {
        h.push(Message::Assistant(format!("checking thing{i}")));
        h.push(call(i));
        h.push(result(i, &"x".repeat(result_chars)));
    }
    h
}

fn digests(msgs: &[Message]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| match m {
            Message::User(t) if t.starts_with("## Earlier turns, folded") => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn approx_tokens(msgs: &[Message]) -> usize {
    msgs.iter()
        .map(|m| match m {
            Message::User(t) | Message::Assistant(t) => t.len(),
            Message::ToolCall { arguments, .. } => arguments.to_string().len(),
            Message::ToolResult { content, .. } => content.len(),
        })
        .sum::<usize>()
        / 4
}

#[test]
fn below_the_budget_it_is_exactly_conversational() {
    let st = State::new("fix it", "");
    let h = history(5, 100);
    let folded = Folded {
        budget_tokens: 48_000,
    };
    assert_eq!(folded.build(&st, &h), Conversational.build(&st, &h));
}

#[test]
fn over_the_budget_old_turns_become_digests_and_the_opening_stays() {
    let st = State::new("fix it", "");
    let h = history(200, 400); // ~20k tokens of results
    let out = Folded {
        budget_tokens: 4_000,
    }
    .build(&st, &h);
    assert!(matches!(&out[0], Message::User(t) if t == "Begin. fix it"));
    let d = digests(&out);
    assert!(!d.is_empty(), "expected at least one fold");
    assert!(matches!(&out[1], Message::User(t) if t.starts_with("## Earlier turns, folded")));
    let tail_tokens = approx_tokens(&out[1 + d.len()..]);
    assert!(
        tail_tokens <= 4_000 + 200,
        "verbatim tail {tail_tokens} tokens"
    );
    assert!(out.len() < h.len());
}

#[test]
fn a_call_is_never_separated_from_its_result() {
    let st = State::new("fix it", "");
    let out = Folded {
        budget_tokens: 3_000,
    }
    .build(&st, &history(150, 300));
    let tail_start = 1 + digests(&out).len();
    assert!(
        !matches!(out[tail_start], Message::ToolResult { .. }),
        "the tail must not open with an orphaned result"
    );
    for (i, m) in out.iter().enumerate() {
        if let Message::ToolResult { id, .. } = m {
            match &out[i - 1] {
                Message::ToolCall { id: cid, .. } => assert_eq!(cid, id),
                other => panic!("result {id} not preceded by its call: {other:?}"),
            }
        }
    }
}

#[test]
fn earlier_folds_are_identical_on_the_next_turn() {
    let st = State::new("fix it", "");
    let f = Folded {
        budget_tokens: 3_000,
    };
    let short = f.build(&st, &history(120, 300));
    let long = f.build(&st, &history(160, 300));
    let (ds, dl) = (digests(&short), digests(&long));
    assert!(!ds.is_empty());
    assert!(dl.len() >= ds.len());
    assert_eq!(
        &dl[..ds.len()],
        &ds[..],
        "a later turn must not rewrite earlier digests"
    );
    assert_eq!(short[0], long[0]);
}

#[test]
fn digests_summarise_actions_deterministically() {
    let st = State::new("fix it", "");
    let out = Folded {
        budget_tokens: 2_000,
    }
    .build(&st, &history(100, 300));
    let first = &digests(&out)[0];
    assert!(first.contains("`bash` `grep -rn thing1 src`"), "{first}");
    assert!(first.contains("you said: checking thing1"), "{first}");
    assert_eq!(
        out,
        Folded {
            budget_tokens: 2_000
        }
        .build(&st, &history(100, 300)),
        "same history, same output"
    );
}

#[test]
fn recent_calls_keep_their_thought_signatures() {
    let st = State::new("fix it", "");
    let out = Folded {
        budget_tokens: 3_000,
    }
    .build(&st, &history(150, 300));
    let last_call = out
        .iter()
        .rev()
        .find_map(|m| match m {
            Message::ToolCall { signature, .. } => Some(signature.clone()),
            _ => None,
        })
        .expect("a call in the tail");
    assert_eq!(last_call, Some("sig150".into()));
    assert_eq!(Folded::default().budget_tokens, 48_000);
    assert_eq!(Folded::default().name(), "folded");
}
