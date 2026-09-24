//! A turn with several tool calls is one block: its calls, then its results, in call order.
//! Every provider encoding and context strategy keeps a block whole.

use std::sync::Mutex;

use serde_json::json;
use superstate_agent::context::{ContextStrategy, Conversational, Folded, StateOnly, StateWindow};
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{Budget, Outcome, Runner};
use superstate_agent::shell::LocalShell;
use superstate_agent::state::State;
use superstate_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};

struct Scripted {
    script: Mutex<Vec<Response>>,
    seen: Mutex<Vec<Request>>,
}

impl Provider for Scripted {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        let mut s = self.script.lock().unwrap();
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

fn tc(id: &str, path: &str, sig: Option<&str>) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "read".into(),
        arguments: json!({"path": path}),
        signature: sig.map(String::from),
    }
}

fn call(n: usize, sig: bool) -> Message {
    Message::ToolCall {
        id: format!("c{n}"),
        name: "bash".into(),
        arguments: json!({"command": format!("grep -rn thing{n} src")}),
        signature: sig.then(|| format!("sig{n}")),
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

/// Opening message, then `turns` blocks of two parallel calls each: [Ca, Cb, Ra, Rb].
fn batched(turns: usize, body_chars: usize) -> Vec<Message> {
    let mut h = vec![Message::User("Begin. fix it".into())];
    for t in 0..turns {
        let (a, b) = (2 * t + 1, 2 * t + 2);
        h.push(call(a, true));
        h.push(call(b, false));
        h.push(result(a, &"x".repeat(body_chars)));
        h.push(result(b, &"y".repeat(body_chars)));
    }
    h
}

fn kinds(msgs: &[Message]) -> Vec<String> {
    msgs.iter()
        .map(|m| match m {
            Message::User(_) => "U".into(),
            Message::Assistant(_) => "A".into(),
            Message::ToolCall { id, .. } => format!("C:{id}"),
            Message::ToolResult { id, .. } => format!("R:{id}"),
        })
        .collect()
}

/// Every result's call is present, and every call's result is present.
fn blocks_whole(msgs: &[Message]) -> bool {
    let calls: Vec<&String> = msgs
        .iter()
        .filter_map(|m| match m {
            Message::ToolCall { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    let results: Vec<&String> = msgs
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    calls.iter().all(|c| results.contains(c)) && results.iter().all(|r| calls.contains(r))
}

#[test]
fn the_loop_records_a_turns_calls_then_its_results() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "a\n").unwrap();
    std::fs::write(d.path().join("b.txt"), "b\n").unwrap();
    let p = Scripted {
        script: Mutex::new(vec![
            Response {
                tool_calls: vec![tc("r1", "a.txt", Some("s1")), tc("r2", "b.txt", None)],
                ..Default::default()
            },
            Response {
                text: "done".into(),
                ..Default::default()
            },
        ]),
        seen: Mutex::new(Vec::new()),
    };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    };
    let mut st = State::new("read both", "true");
    let (outcome, report) = runner.run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.tool_calls, 2);
    let second = &p.seen.lock().unwrap()[1];
    assert_eq!(
        kinds(&second.messages),
        vec!["U", "C:r1", "C:r2", "R:r1", "R:r2"]
    );
}

#[test]
fn state_only_carries_the_whole_last_block() {
    let out = StateOnly.build(&State::new("fix it", ""), &batched(3, 10));
    assert_eq!(kinds(&out[1..]), vec!["C:c5", "C:c6", "R:c5", "R:c6"]);
}

#[test]
fn state_window_carries_whole_blocks() {
    let w = StateWindow {
        results: 1,
        lines_per_result: 60,
    };
    let out = w.build(&State::new("fix it", ""), &batched(3, 10));
    assert_eq!(kinds(&out[1..]), vec!["C:c5", "C:c6", "R:c5", "R:c6"]);
    assert!(blocks_whole(&out));
}

#[test]
fn folded_cuts_only_between_blocks() {
    let st = State::new("fix it", "");
    let out = Folded {
        budget_tokens: 2_000,
    }
    .build(&st, &batched(80, 200));
    let digests: Vec<&String> = out
        .iter()
        .filter_map(|m| match m {
            Message::User(t) if t.starts_with("## Earlier turns, folded") => Some(t),
            _ => None,
        })
        .collect();
    assert!(!digests.is_empty(), "expected a fold");
    let tail = &out[1 + digests.len()..];
    assert!(
        matches!(tail[0], Message::ToolCall { .. }),
        "{:?}",
        kinds(tail)
    );
    assert!(blocks_whole(tail), "{:?}", kinds(tail));
    // Both calls of a folded block are summarised, each with its own result.
    assert!(
        digests[0].contains("`grep -rn thing1 src`"),
        "{}",
        digests[0]
    );
    assert!(
        digests[0].contains("`grep -rn thing2 src`"),
        "{}",
        digests[0]
    );
    assert!(!digests[0].contains("no result"), "{}", digests[0]);
}

#[test]
fn the_system_prompt_invites_batching_independent_calls() {
    let d = tempfile::tempdir().unwrap();
    let p = Scripted {
        script: Mutex::new(vec![Response {
            text: "done".into(),
            ..Default::default()
        }]),
        seen: Mutex::new(Vec::new()),
    };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 2,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    };
    runner.run(&mut State::new("anything", "true"));
    let system = p.seen.lock().unwrap()[0].system.clone();
    assert!(system.contains("## Working efficiently"), "{system}");
    assert!(
        system.contains("several tool calls in one turn"),
        "{system}"
    );
}

#[test]
fn folded_digests_pair_by_position_not_by_repeating_gemini_ids() {
    // Gemini issues no ids; the adapter numbers calls call_0, call_1 ... afresh every turn.
    let read = |id: &str, path: &str| Message::ToolCall {
        id: id.into(),
        name: "read".into(),
        arguments: json!({"path": path}),
        signature: None,
    };
    let lines = |id: &str, n: usize| Message::ToolResult {
        id: id.into(),
        name: "read".into(),
        content: "line\n".repeat(n),
        is_error: false,
    };
    let mut h = vec![Message::User("Begin.".into())];
    for t in 0..60 {
        h.push(read("call_0", &format!("first_{t}.rs")));
        h.push(read("call_1", &format!("second_{t}.rs")));
        h.push(lines("call_0", 2 + t));
        h.push(lines("call_1", 20 + t));
        h.push(Message::Assistant("x".repeat(400)));
    }
    let out = Folded {
        budget_tokens: 1_000,
    }
    .build(&State::new("fix it", ""), &h);
    let digest = out
        .iter()
        .find_map(|m| match m {
            Message::User(t) if t.starts_with("## Earlier turns, folded") => Some(t.clone()),
            _ => None,
        })
        .expect("a fold");
    for t in 0..3 {
        assert!(
            digest.contains(&format!("`first_{t}.rs` -> {} lines read", 2 + t)),
            "{digest}"
        );
        assert!(
            digest.contains(&format!("`second_{t}.rs` -> {} lines read", 20 + t)),
            "{digest}"
        );
    }
}
