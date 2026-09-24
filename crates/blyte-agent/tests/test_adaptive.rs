//! `adaptive`: `folded` whose budget grows when the worker re-fetches something only a fold
//! removed from view. Folds stay prefix-stable and the prompt changes only when a fold happens.

use std::sync::Mutex;

use blyte_agent::context::{Adaptive, Compact, ContextStrategy, Conversational, Folded};
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};
use serde_json::{json, Value};

const A: Adaptive = Adaptive {
    min_tokens: 2_000,
    step_tokens: 1_000,
    max_tokens: 4_000,
};

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

fn open() -> Vec<Message> {
    vec![Message::User("Begin. fix it".into())]
}

/// A 40-line output, about 520 tokens.
fn body() -> String {
    format!("{}\n", "o".repeat(51)).repeat(40)
}

/// `turns` distinct bash turns, never re-fetched.
fn filler(h: &mut Vec<Message>, from: usize, turns: usize) {
    for t in from..from + turns {
        h.push(call(
            t,
            "bash",
            json!({"command": format!("echo step {t}")}),
        ));
        h.push(res(t, "bash", &body()));
    }
}

fn read(h: &mut Vec<Message>, n: usize, path: &str) {
    h.push(call(n, "read", json!({ "path": path })));
    h.push(res(n, "read", &body()));
}

fn edit(h: &mut Vec<Message>, n: usize, path: &str) {
    h.push(call(
        n,
        "edit",
        json!({"path": path, "old": "a", "new": "b"}),
    ));
    h.push(res(n, "edit", "replaced 1 line"));
}

fn digests(out: &[Message]) -> usize {
    out.iter()
        .filter(|m| matches!(m, Message::User(t) if t.starts_with("## Earlier turns, folded")))
        .count()
}

#[test]
fn without_refetches_it_is_exactly_folded_at_its_minimum() {
    let mut h = open();
    read(&mut h, 0, "a.rs");
    filler(&mut h, 1, 12);
    let st = State::new("fix it", "");
    assert_eq!(
        A.build(&st, &h),
        Folded {
            budget_tokens: 2_000
        }
        .build(&st, &h)
    );
    assert_eq!(
        A.folds(&h),
        Folded {
            budget_tokens: 2_000
        }
        .folds(&h)
    );
    assert_eq!(A.fold_budget(&h), Some(2_000));
}

#[test]
fn re_reading_a_folded_file_grows_the_budget() {
    let mut h = open();
    read(&mut h, 0, "a.rs");
    filler(&mut h, 1, 12);
    assert!(
        A.folds(&h).first().is_some_and(|&(a, b)| a == 0 && b >= 2),
        "a.rs is folded"
    );
    read(&mut h, 50, "a.rs");
    assert_eq!(A.fold_budget(&h), Some(3_000));
    // A ranged re-read of the same file counts too.
    let mut h2 = open();
    read(&mut h2, 0, "a.rs");
    filler(&mut h2, 1, 12);
    h2.push(call(
        50,
        "read",
        json!({"path": "a.rs", "start_line": 10, "end_line": 20}),
    ));
    h2.push(res(50, "read", "slice"));
    assert_eq!(A.fold_budget(&h2), Some(3_000));
}

#[test]
fn a_re_read_after_an_edit_is_not_a_refetch() {
    let mut h = open();
    read(&mut h, 0, "a.rs");
    filler(&mut h, 1, 12);
    edit(&mut h, 40, "a.rs");
    read(&mut h, 50, "a.rs");
    assert_eq!(A.fold_budget(&h), Some(2_000));
}

#[test]
fn a_repeat_of_a_call_still_in_view_is_not_a_refetch() {
    let mut h = open();
    filler(&mut h, 0, 12);
    read(&mut h, 40, "b.rs");
    read(&mut h, 41, "b.rs");
    assert_eq!(A.fold_budget(&h), Some(2_000));
}

#[test]
fn re_running_a_folded_command_is_not_a_refetch() {
    let mut h = open();
    h.push(call(0, "bash", json!({"command": "pytest -q"})));
    h.push(res(0, "bash", &body()));
    filler(&mut h, 1, 12);
    h.push(call(50, "bash", json!({"command": "pytest -q"})));
    h.push(res(50, "bash", &body()));
    assert_eq!(A.fold_budget(&h), Some(2_000));
}

#[test]
fn an_identical_folded_search_is_a_refetch() {
    let mut h = open();
    h.push(call(
        0,
        "grep",
        json!({"pattern": "fn parse", "path": "src"}),
    ));
    h.push(res(0, "grep", &body()));
    filler(&mut h, 1, 12);
    h.push(call(
        50,
        "grep",
        json!({"pattern": "fn parse", "path": "src"}),
    ));
    h.push(res(50, "grep", &body()));
    assert_eq!(A.fold_budget(&h), Some(3_000));
}

#[test]
fn growth_is_capped() {
    let mut h = open();
    for (n, p) in ["a.rs", "b.rs", "c.rs", "d.rs"].iter().enumerate() {
        read(&mut h, n, p);
    }
    filler(&mut h, 10, 12);
    for (n, p) in ["a.rs", "b.rs", "c.rs", "d.rs"].iter().enumerate() {
        read(&mut h, 60 + n, p);
    }
    assert_eq!(A.fold_budget(&h), Some(4_000));
}

#[test]
fn a_grown_budget_folds_less() {
    let mut h = open();
    for (n, p) in ["a.rs", "b.rs"].iter().enumerate() {
        read(&mut h, n, p);
    }
    filler(&mut h, 10, 8);
    for (n, p) in ["a.rs", "b.rs"].iter().enumerate() {
        read(&mut h, 60 + n, p);
    }
    filler(&mut h, 70, 8);
    let fixed = Folded {
        budget_tokens: 2_000,
    };
    assert!(A.fold_budget(&h) > Some(2_000));
    let st = State::new("fix it", "");
    let tail = |out: &[Message]| out.len() - 1 - digests(out);
    assert!(
        tail(&A.build(&st, &h)) > tail(&fixed.build(&st, &h)),
        "a larger budget keeps a longer verbatim tail"
    );
}

#[test]
fn the_prompt_only_changes_when_a_fold_happens() {
    let mut h = open();
    for t in 0..60 {
        let p = format!("f{}.rs", t % 5);
        match t % 4 {
            0 | 1 => read(&mut h, t, &p),
            2 => edit(&mut h, t, &p),
            _ => {
                h.push(call(t, "bash", json!({"command": "pytest -q"})));
                h.push(res(t, "bash", &body()));
            }
        }
    }
    let st = State::new("fix it", "");
    let mut folds_seen = 0;
    for n in 2..h.len() {
        let a = A.build(&st, &h[..n]);
        let b = A.build(&st, &h[..n + 1]);
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
    assert!(folds_seen > 0);
    assert!(
        A.fold_budget(&h) > Some(2_000),
        "this history re-reads folded files"
    );
}

#[test]
fn every_folding_strategy_reports_its_budget() {
    let mut h = open();
    filler(&mut h, 0, 3);
    assert_eq!(Folded { budget_tokens: 300 }.fold_budget(&h), Some(300));
    assert_eq!(
        Compact {
            budget_tokens: 300,
            min_dedup_chars: 200
        }
        .fold_budget(&h),
        Some(300)
    );
    assert_eq!(Conversational.fold_budget(&h), None);
    assert_eq!(Adaptive::default().fold_budget(&h), Some(16_000));
    assert_eq!(
        (
            Adaptive::default().step_tokens,
            Adaptive::default().max_tokens
        ),
        (8_000, 64_000)
    );
    assert_eq!(Adaptive::default().name(), "adaptive");
}

struct Scripted(Mutex<Vec<Response>>);

impl Provider for Scripted {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let mut s = self.0.lock().unwrap();
        Ok(if s.is_empty() {
            Response::default()
        } else {
            s.remove(0)
        })
    }
    fn model_id(&self) -> String {
        "scripted".into()
    }
}

#[test]
fn each_turn_records_the_budget_in_force() {
    let d = tempfile::tempdir().unwrap();
    let p = Scripted(Mutex::new(vec![Response {
        tool_calls: vec![ToolCall {
            id: "c0".into(),
            name: "bash".into(),
            arguments: json!({"command": "echo hi"}),
            signature: None,
        }],
        ..Default::default()
    }]));
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &A,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 3,
            max_gate_attempts: 1,
            max_duration: None,
            max_cost_usd: None,
        },
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };
    let (_, report) = runner.run(&mut State::new("x", "true"));
    assert!(!report.turn_usage.is_empty());
    assert!(report
        .turn_usage
        .iter()
        .all(|t| t.fold_budget == Some(2_000)));
}
