use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Runner, PEER_MESSAGE_PREFIX};
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
        claim_check: false,
        base_check: blyte_agent::run::BaseCheck::None,
    };
    runner.run(state);
    let sys = p.seen.lock().unwrap()[0].system.clone();
    sys
}

#[test]
fn instructions_replace_the_implementer_framing() {
    let mut state = State::new("build the feature", "true");
    state.brief.instructions = Some("## How you work\nX\n\n".to_string());
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
fn conversation_state_serializes_without_instructions() {
    let state = State::new("g", "");
    let val = serde_json::to_value(&state).unwrap();
    assert!(val.get("instructions").is_none());
    assert!(val["brief"].get("instructions").is_none());
}

#[test]
fn a_peer_message_is_never_the_users_first_request() {
    let peer_prompt = format!("{PEER_MESSAGE_PREFIX}api (shop). Not from user.\n\ntests are red");
    let mut peer_first = State::new(peer_prompt, "");
    peer_first.brief.instructions = Some("## How you work\nX\n\n".to_string());
    peer_first.runtime.conversation = Some(Vec::new());

    let sys1 = run_with_state(&mut peer_first);
    assert!(!sys1.contains("## Your user's first request"), "{sys1}");
    assert_eq!(peer_first.runtime.conversation_goal, None);

    peer_first.brief.goal = "fix the build".to_string();
    let sys2 = run_with_state(&mut peer_first);
    assert!(
        sys2.contains("## Your user's first request\nfix the build\n\n"),
        "{sys2}"
    );
    assert_eq!(
        peer_first.runtime.conversation_goal.as_deref(),
        Some("fix the build")
    );

    let mut normal = State::new("fix the build", "");
    normal.brief.instructions = Some("## How you work\nX\n\n".to_string());
    normal.runtime.conversation = Some(Vec::new());
    let sys_normal = run_with_state(&mut normal);
    assert!(
        sys_normal.contains("## Your user's first request\nfix the build\n\n"),
        "{sys_normal}"
    );
    assert_eq!(
        normal.runtime.conversation_goal.as_deref(),
        Some("fix the build")
    );
}
