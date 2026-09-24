//! The state prompt keeps a stable, growing prefix, so Gemini's implicit cache (prefixes of at
//! least 4,096 tokens) can apply. The append-only action ledger comes before everything that
//! changes turn to turn.

use blyte_agent::context::{ContextStrategy, StateOnly, StateWindow};
use blyte_agent::state::{ActionEntry, State};
use blyte_agent::working_set::FileSnippet;
use blyte_provider::Message;

fn act(turn: u32, target: &str) -> ActionEntry {
    ActionEntry {
        turn,
        tool: "bash".into(),
        target: target.into(),
        summary: "exit 0".into(),
        is_error: false,
    }
}

fn snippet(path: &str, content: &str) -> FileSnippet {
    FileSnippet {
        path: path.into(),
        content: content.into(),
        start_line: None,
        end_line: None,
        last_accessed_turn: 1,
    }
}

fn prompt(strategy: &dyn ContextStrategy, st: &State) -> String {
    match &strategy.build(st, &[])[0] {
        Message::User(t) => t.clone(),
        other => panic!("expected the state prompt first, got {other:?}"),
    }
}

fn busy_state() -> State {
    let mut st = State::new("fix the numpy aliases", "pytest -q");
    st.runtime.actions = vec![act(1, "grep -rn n.float"), act(2, "pytest -q")];
    st.runtime.working_set.files = vec![snippet("a.py", "x = 1")];
    st.runtime
        .working_set
        .record_failure("E   AttributeError: n.float");
    st.agent.fields.insert(
        "hypothesis".into(),
        serde_json::json!("numpy 2 removed n.float"),
    );
    st
}

#[test]
fn the_ledger_comes_before_everything_that_changes() {
    let text = prompt(&StateOnly, &busy_state());
    let pos = |h: &str| {
        text.find(h)
            .unwrap_or_else(|| panic!("missing {h}:\n{text}"))
    };
    let task = pos("## Task");
    let ledger = pos("## Actions Taken So Far");
    let notes = pos("## Your state");
    let working = pos("## Active Working Set");
    let failure = pos("## Last Test Failure");
    assert!(task < ledger, "{text}");
    assert!(ledger < notes, "{text}");
    assert!(notes < working, "{text}");
    assert!(working < failure, "{text}");
}

#[test]
fn the_next_turn_starts_with_this_turns_task_and_ledger() {
    let before = busy_state();
    let mut after = before.clone();
    // A new action, and every volatile section changes.
    after.runtime.actions.push(act(3, "edit a.py"));
    after.runtime.working_set.files = vec![snippet("b.py", "y = 2")];
    after.runtime.working_set.clear_failure();
    after
        .agent
        .fields
        .insert("next".into(), serde_json::json!("rerun pytest"));

    for strategy in [&StateOnly as &dyn ContextStrategy, &StateWindow::default()] {
        let a = prompt(strategy, &before);
        let b = prompt(strategy, &after);
        let stable = &a[..a.find("\n## Your state").expect("notes section")];
        assert!(
            b.starts_with(stable),
            "turn N+1 must start with turn N's task and ledger\n--- N ---\n{a}\n--- N+1 ---\n{b}"
        );
        assert!(stable.contains("grep -rn n.float") && stable.contains("pytest -q"));
    }
}

#[test]
fn with_no_actions_the_task_is_followed_by_the_notes() {
    let st = State::new("fix it", "");
    let text = prompt(&StateOnly, &st);
    assert!(!text.contains("## Actions Taken So Far"));
    assert!(text.find("## Task").unwrap() < text.find("## Your state").unwrap());
}
