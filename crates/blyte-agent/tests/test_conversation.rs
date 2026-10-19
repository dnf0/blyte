use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::{Runtime, State};
use blyte_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

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

#[test]
fn second_prompt_continues_the_conversation() {
    let d = tempfile::tempdir().unwrap();
    let p = Scripted {
        script: Mutex::new(vec![
            Response {
                text: "Which file?".into(),
                ..Default::default()
            },
            Response {
                text: "Got it.".into(),
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
        on_event: None,
        research: false,
        claim_check: false,
    };
    let mut state = State::new("first", "");
    state.runtime.conversation = Some(Vec::new());

    let (outcome1, _) = runner.run(&mut state);
    assert_eq!(outcome1, Outcome::Verified);

    state.orchestrator.goal = "src/a.rs".into();
    let (outcome2, _) = runner.run(&mut state);
    assert_eq!(outcome2, Outcome::Verified);

    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[1].messages,
        vec![
            Message::User("Begin. first".into()),
            Message::Assistant("Which file?".into()),
            Message::User("src/a.rs".into()),
        ]
    );
}

#[test]
fn system_prompt_is_stable_across_prompts() {
    let d = tempfile::tempdir().unwrap();
    let p = Scripted {
        script: Mutex::new(vec![
            Response {
                text: "Which file?".into(),
                ..Default::default()
            },
            Response {
                text: "Got it.".into(),
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
        on_event: None,
        research: false,
        claim_check: false,
    };
    let mut state = State::new("first", "");
    state.runtime.conversation = Some(Vec::new());

    runner.run(&mut state);
    state.orchestrator.goal = "src/a.rs".into();
    runner.run(&mut state);

    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].system, seen[0].system);
    assert!(seen[1].system.contains("first"));
    assert!(!seen[1].system.contains("src/a.rs"));
}

#[test]
fn without_conversation_each_run_is_fresh() {
    let d = tempfile::tempdir().unwrap();
    let p = Scripted {
        script: Mutex::new(vec![
            Response {
                text: "Which file?".into(),
                ..Default::default()
            },
            Response {
                text: "Got it.".into(),
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
        on_event: None,
        research: false,
        claim_check: false,
    };
    let mut state = State::new("first", "");
    assert_eq!(state.runtime.conversation, None);

    runner.run(&mut state);
    state.orchestrator.goal = "src/a.rs".into();
    runner.run(&mut state);

    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[1].messages,
        vec![Message::User("Begin. src/a.rs".into())]
    );
    assert_eq!(state.runtime.conversation, None);
}

#[test]
fn conversation_is_stored_after_budget_exhausted() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "hello\n").unwrap();
    let p = Scripted {
        script: Mutex::new(vec![Response {
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "read".into(),
                arguments: json!({"path": "a.txt"}),
                signature: None,
            }],
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
            max_turns: 1,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };
    let mut state = State::new("read a file", "");
    state.runtime.conversation = Some(Vec::new());

    let (outcome, _) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::BudgetExhausted);

    let conv = state
        .runtime
        .conversation
        .as_ref()
        .expect("conversation should be Some");
    assert_eq!(conv.len(), 3);
    assert_eq!(conv[0], Message::User("Begin. read a file".into()));
    assert!(matches!(&conv[1], Message::ToolCall { id, name, .. } if id == "c1" && name == "read"));
    assert!(
        matches!(&conv[2], Message::ToolResult { id, name, content, .. } if id == "c1" && name == "read" && content.contains("hello"))
    );
}

#[test]
fn serialized_runtime_without_conversation_is_unchanged() {
    let val = serde_json::to_value(Runtime::default()).unwrap();
    assert!(val.get("conversation").is_none());
    assert!(val.get("conversation_goal").is_none());
    assert_eq!(val, json!({"rollbacks": 0}));
}
