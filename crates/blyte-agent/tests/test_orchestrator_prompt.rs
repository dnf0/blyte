use std::collections::VecDeque;
use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::orchestrator::{prompt_sections, prompt_sections_with_dispatch, PREAMBLE};
use blyte_agent::run::{Budget, Outcome, Runner, ORCHESTRATOR_EXPLORE_NUDGE};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

struct RecordingProvider {
    seen: Mutex<Vec<Request>>,
}

impl Provider for RecordingProvider {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        Ok(Response {
            text: "done".into(),
            ..Default::default()
        })
    }

    fn model_id(&self) -> String {
        "recording".into()
    }
}

fn run_with_state(state: &mut State) -> String {
    let d = tempfile::tempdir().unwrap();
    let p = RecordingProvider {
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
    runner.run(state);
    let sys = p.seen.lock().unwrap()[0].system.clone();
    sys
}

#[test]
fn instructions_replace_the_implementer_framing() {
    let mut state = State::new("build the feature", "true");
    state.orchestrator.instructions = Some("## How you work\nX\n\n".to_string());
    let system = run_with_state(&mut state);

    assert!(system.starts_with("## How you work\nX"), "{system}");
    assert!(system.contains("## Your user's first request"), "{system}");
    assert!(system.contains("build the feature"), "{system}");
    assert!(!system.contains("You are a software engineer"), "{system}");
    assert!(!system.contains("How completion is decided"), "{system}");
}

#[test]
fn without_instructions_the_prompt_is_unchanged() {
    let mut state = State::new("build the feature", "true");
    let system = run_with_state(&mut state);

    assert!(system.contains("You are a software engineer"), "{system}");
    assert!(system.contains("How completion is decided"), "{system}");
}

#[test]
fn orchestrator_state_serializes_without_instructions() {
    let state = State::new("g", "");
    let val = serde_json::to_value(&state).unwrap();
    assert!(val.get("instructions").is_none());
    assert!(val["orchestrator"].get("instructions").is_none());
}

#[test]
fn preamble_starts_work_straight_away() {
    assert!(PREAMBLE.contains("do not run `--help`"));
    assert!(PREAMBLE.contains("reports a missing key itself"));

    let paragraph = "\
Start on the work at once. The skill below documents every blyte command you need, so do not run \
`--help`, and do not check the environment or `~/.config/blyte` for API keys: `blyte dispatch` reports a \
missing key itself. When the request names a plan, read it and dispatch its independent tasks in your \
first turns.";
    assert!(PREAMBLE.contains(paragraph));

    let sections = prompt_sections("dummy skill", None);
    assert!(sections.contains(paragraph));
}

#[test]
fn prompt_contains_dispatch_tool_instruction_only_when_configured() {
    use blyte_agent::orchestrator::DISPATCH_TOOL_INSTRUCTION;

    let without_dispatch = prompt_sections("dummy skill", None);
    assert!(!without_dispatch.contains(DISPATCH_TOOL_INSTRUCTION));

    let without_dispatch_explicit = prompt_sections_with_dispatch("dummy skill", None, false);
    assert!(!without_dispatch_explicit.contains(DISPATCH_TOOL_INSTRUCTION));
    assert!(without_dispatch_explicit.contains("Get oriented with a few cheap calls"));
    assert!(without_dispatch_explicit.contains("reviewing a dispatch's result"));
    assert!(!without_dispatch_explicit.contains("focused follow-up dispatch"));

    let with_dispatch = prompt_sections_with_dispatch("dummy skill", None, true);
    assert!(with_dispatch.contains(DISPATCH_TOOL_INSTRUCTION));
    assert!(with_dispatch.contains("## Delegating work"));
    assert!(with_dispatch.contains("Get oriented with a few cheap calls"));
    assert!(with_dispatch.contains("reviewing a dispatch's result"));
    assert!(with_dispatch.contains("Make the first dispatch early"));
    assert!(with_dispatch.contains("focused follow-up dispatch"));
}

const EXPLORE_REMINDER: &str = "You have made 8 tool calls without dispatching. Dispatch the work now from what you know; the implementer reads the code it changes. Keep reading only if you cannot yet say which files the task is about.";

struct ScriptedProvider {
    seen: Mutex<Vec<Request>>,
    responses: Mutex<VecDeque<Response>>,
}

impl ScriptedProvider {
    fn new(responses: Vec<Response>) -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into()),
        }
    }
}

impl Provider for ScriptedProvider {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Response {
                text: "done".into(),
                ..Default::default()
            }))
    }

    fn model_id(&self) -> String {
        "scripted".into()
    }
}

fn read_response(turn: usize) -> Response {
    Response {
        tool_calls: vec![ToolCall {
            id: format!("c{turn}"),
            name: "read".into(),
            arguments: json!({"path": "sample.txt"}),
            signature: None,
        }],
        ..Default::default()
    }
}

fn count_explore_reminders(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|m| matches!(m, Message::User(t) if t == EXPLORE_REMINDER))
        .count()
}

#[test]
fn orchestrator_run_nudges_once_after_eight_reads() {
    assert_eq!(ORCHESTRATOR_EXPLORE_NUDGE, 8);
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("sample.txt"), "hello\n").unwrap();

    let mut responses: Vec<Response> = (1..=9).map(read_response).collect();
    responses.push(Response {
        text: "done".into(),
        ..Default::default()
    });
    let p = ScriptedProvider::new(responses);

    let mut state = State::new("plan and dispatch", "");
    state.orchestrator.instructions = Some("## How you work\nOrchestrate.\n\n".to_string());

    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 15,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };

    let (outcome, _) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Finished);

    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 10);

    for (i, req) in seen.iter().enumerate().take(8) {
        assert_eq!(
            count_explore_reminders(&req.messages),
            0,
            "request {} should not have the reminder",
            i + 1
        );
    }

    assert_eq!(
        seen[8].messages.last(),
        Some(&Message::User(EXPLORE_REMINDER.to_string())),
        "request after the 8th tool call must end with the reminder"
    );

    assert_eq!(count_explore_reminders(&seen[9].messages), 1);
}

#[test]
fn interactive_orchestrator_run_never_nudges() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("sample.txt"), "hello\n").unwrap();

    let mut responses: Vec<Response> = (1..=9).map(read_response).collect();
    responses.push(Response {
        text: "done".into(),
        ..Default::default()
    });
    let p = ScriptedProvider::new(responses);

    // The REPL keeps a conversation; there the person may only be asking questions.
    let mut state = State::new("explain the code", "");
    state.orchestrator.instructions = Some("## How you work\nOrchestrate.\n\n".to_string());
    state.runtime.conversation = Some(Vec::new());

    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 15,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };

    let (outcome, _) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Finished);
    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 10);
    for req in seen.iter() {
        assert_eq!(count_explore_reminders(&req.messages), 0);
    }
}

#[test]
fn orchestrator_run_with_early_dispatch_never_nudges() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("sample.txt"), "hello\n").unwrap();

    for dispatch_call in [
        ToolCall {
            id: "c3".into(),
            name: "bash".into(),
            arguments: json!({"command": "true # blyte dispatch X --plan p.md --plan-task \"Task X\""}),
            signature: None,
        },
        ToolCall {
            id: "c3".into(),
            name: "dispatch".into(),
            arguments: json!({"task": "Task X"}),
            signature: None,
        },
    ] {
        let mut responses: Vec<Response> = (1..=9).map(read_response).collect();
        responses[2] = Response {
            tool_calls: vec![dispatch_call],
            ..Default::default()
        };
        responses.push(Response {
            text: "done".into(),
            ..Default::default()
        });
        let p = ScriptedProvider::new(responses);

        let mut state = State::new("plan and dispatch", "");
        state.orchestrator.instructions = Some("## How you work\nOrchestrate.\n\n".to_string());

        let runner = Runner {
            provider: &p,
            shell: &LocalShell,
            fs: &LocalFs,
            context: &Conversational,
            root: d.path().to_path_buf(),
            budget: Budget {
                max_turns: 15,
                max_gate_attempts: 1,
                max_duration: None,
            },
            rollback: false,
            on_event: None,
            research: false,
            claim_check: false,
        };

        let (outcome, _) = runner.run(&mut state);
        assert_eq!(outcome, Outcome::Finished);

        let seen = p.seen.lock().unwrap();
        assert_eq!(seen.len(), 10);
        for req in seen.iter() {
            assert_eq!(count_explore_reminders(&req.messages), 0);
        }
    }
}

#[test]
fn non_orchestrator_run_with_nine_reads_never_nudges() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("sample.txt"), "hello\n").unwrap();

    let mut responses: Vec<Response> = (1..=9).map(read_response).collect();
    responses.push(Response {
        text: "done".into(),
        ..Default::default()
    });
    let p = ScriptedProvider::new(responses);

    let mut state = State::new("implement feature", "true");
    state.runtime.conversation = Some(Vec::new());

    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 15,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };

    let (outcome, _) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);

    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 10);
    for req in seen.iter() {
        assert_eq!(count_explore_reminders(&req.messages), 0);
    }
    let final_history = state.runtime.conversation.as_ref().unwrap();
    assert_eq!(count_explore_reminders(final_history), 0);
}
