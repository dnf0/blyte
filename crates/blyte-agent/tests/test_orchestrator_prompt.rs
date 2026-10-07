use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::orchestrator::{prompt_sections, PREAMBLE};
use blyte_agent::run::{Budget, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

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
