//! Fold notes: the first time a fold appears, the worker records what to keep from the turns
//! being folded, and that fold's digest carries the note from then on.

use std::sync::Mutex;

use serde_json::json;
use superstate_agent::context::{Compact, ContextStrategy, Folded};
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{
    render_fold_note, Budget, Outcome, RunReport, Runner, FOLD_NOTE_MARKER,
};
use superstate_agent::shell::LocalShell;
use superstate_agent::state::State;
use superstate_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};

const NOTE: &str = r#"{"findings": ["the bug is in parse() at src/lib.rs:42"], "ruled_out": ["upgrading serde"], "files": {"src/lib.rs": "the parser"}, "next": ["fix the off-by-one"]}"#;

/// Answers fold-note requests with `note_reply`, everything else from `script` in order.
struct Scripted {
    script: Mutex<Vec<Response>>,
    note_reply: String,
    seen: Mutex<Vec<Request>>,
}

fn is_note_request(req: &Request) -> bool {
    matches!(req.messages.last(), Some(Message::User(t)) if t.starts_with(FOLD_NOTE_MARKER))
}

impl Provider for Scripted {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        if is_note_request(req) {
            return Ok(Response {
                text: self.note_reply.clone(),
                ..Default::default()
            });
        }
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

/// Four turns, each reading a different 40-line file, then done. Folds under a 300-token budget.
fn run_with(note_reply: &str, fold_notes: bool) -> (Outcome, RunReport, Vec<Request>) {
    let d = tempfile::tempdir().unwrap();
    let body = format!("{}\n", "x".repeat(50)).repeat(40);
    let mut script = Vec::new();
    for i in 0..4 {
        std::fs::write(d.path().join(format!("f{i}.txt")), &body).unwrap();
        script.push(Response {
            tool_calls: vec![ToolCall {
                id: format!("c{i}"),
                name: "read".into(),
                arguments: json!({"path": format!("f{i}.txt")}),
                signature: None,
            }],
            ..Default::default()
        });
    }
    script.push(Response {
        text: "done".into(),
        ..Default::default()
    });
    let p = Scripted {
        script: Mutex::new(script),
        note_reply: note_reply.into(),
        seen: Mutex::new(Vec::new()),
    };
    let folded = Folded { budget_tokens: 300 };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &folded,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 1,
            max_duration: None,
            max_cost_usd: None,
        },
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };
    let mut st = State::new("read the files", "true");
    st.orchestrator.fold_notes = fold_notes;
    let (outcome, report) = runner.run(&mut st);
    let seen = p.seen.lock().unwrap().clone();
    (outcome, report, seen)
}

fn has_notes(req: &Request, needle: &str) -> bool {
    req.messages.iter().any(|m| {
        matches!(m, Message::User(t) if t.starts_with("## Earlier turns, folded")
            && t.contains("### Your notes from these turns")
            && t.contains(needle))
    })
}

#[test]
fn a_new_fold_gets_a_note_that_its_digest_then_carries() {
    let (outcome, report, seen) = run_with(NOTE, true);
    assert_eq!(outcome, Outcome::Verified);
    let notes: Vec<usize> = (0..seen.len())
        .filter(|&k| is_note_request(&seen[k]))
        .collect();
    assert!(!notes.is_empty(), "expected at least one fold-note request");
    assert_eq!(report.fold_notes as usize, notes.len());
    assert_eq!(report.fold_note_failures, 0);

    for &k in &notes {
        // Shares the previous main request's prefix, system prompt and tools, so it caches.
        let prev = (0..k)
            .rev()
            .find(|&j| !is_note_request(&seen[j]))
            .expect("a main request before the note request");
        let (note, main) = (&seen[k], &seen[prev]);
        assert!(note.messages.len() > main.messages.len());
        assert_eq!(&note.messages[..main.messages.len()], &main.messages[..]);
        assert_eq!(note.system, main.system);
        assert_eq!(note.tools.len(), main.tools.len());
    }

    let after = (notes[0] + 1..seen.len())
        .find(|&j| !is_note_request(&seen[j]))
        .expect("a main request after the first note");
    assert!(
        has_notes(&seen[after], "the bug is in parse() at src/lib.rs:42"),
        "the digest must carry the note"
    );
    assert!(has_notes(&seen[after], "`src/lib.rs`: the parser"));
}

#[test]
fn an_unusable_note_leaves_the_harness_digest_and_is_not_retried() {
    let (outcome, report, seen) = run_with("I'd rather keep working.", true);
    assert_eq!(outcome, Outcome::Verified);
    let asked = seen.iter().filter(|r| is_note_request(r)).count();
    assert!(asked > 0);
    assert_eq!(report.fold_notes, 0);
    assert_eq!(
        report.fold_note_failures as usize, asked,
        "one request per fold, no retries"
    );
    assert!(!seen.iter().any(|r| has_notes(r, "")));
}

#[test]
fn off_by_default() {
    let (_, report, seen) = run_with(NOTE, false);
    assert!(!seen.iter().any(is_note_request));
    assert_eq!(report.fold_notes + report.fold_note_failures, 0);
    assert!(!State::new("g", "v").orchestrator.fold_notes);
}

fn history(turns: usize) -> Vec<Message> {
    let mut h = vec![Message::User("Begin. fix it".into())];
    for t in 0..turns {
        h.push(Message::ToolCall {
            id: format!("c{t}"),
            name: "bash".into(),
            arguments: json!({"command": format!("cat f{t}.txt")}),
            signature: None,
        });
        h.push(Message::ToolResult {
            id: format!("c{t}"),
            name: "bash".into(),
            content: format!("{}\n", "y".repeat(50)).repeat(40),
            is_error: false,
        });
    }
    h
}

#[test]
fn a_stored_note_appears_only_in_its_own_folds_digest() {
    let f = Folded { budget_tokens: 300 };
    let h = history(6);
    let folds = f.folds(&h);
    assert!(folds.len() >= 2, "{folds:?}");
    assert_eq!(
        Compact {
            budget_tokens: 300,
            min_dedup_chars: 200
        }
        .folds(&h),
        folds
    );

    let mut st = State::new("fix it", "");
    let plain = f.build(&st, &h);
    st.runtime
        .fold_notes
        .insert(folds[0].1, "Established:\n- NOTE_ONE\n".into());
    let noted = f.build(&st, &h);
    assert_eq!(plain.len(), noted.len());
    let changed: Vec<usize> = (0..plain.len()).filter(|&i| plain[i] != noted[i]).collect();
    assert_eq!(changed, vec![1], "only the first fold's digest changes");
    let Message::User(t) = &noted[1] else {
        panic!("expected a digest");
    };
    assert!(
        t.contains("### Your notes from these turns\nEstablished:\n- NOTE_ONE"),
        "{t}"
    );

    // An empty note (a failed request) renders as the harness digest alone.
    st.runtime.fold_notes.insert(folds[0].1, String::new());
    assert_eq!(f.build(&st, &h), plain);
}

#[test]
fn notes_render_deterministically_from_json() {
    let r = render_fold_note(&format!("Here you go:\n```json\n{NOTE}\n```")).unwrap();
    assert_eq!(
        r,
        "Established:\n- the bug is in parse() at src/lib.rs:42\n\
         Ruled out:\n- upgrading serde\n\
         Files:\n- `src/lib.rs`: the parser\n\
         Still to do:\n- fix the off-by-one\n"
    );
    assert_eq!(render_fold_note("no json here"), None);
    assert_eq!(render_fold_note("{}"), None);
    assert_eq!(render_fold_note(r#"{"findings": []}"#), None);
}
