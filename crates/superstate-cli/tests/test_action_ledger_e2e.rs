//! End-to-end integration tests for action ledger tracking and bounded context invariant in REPL and solver.
//!
//! Validates:
//! 1. REPL `/state` command formats and displays the `Runtime.actions` ledger correctly.
//! 2. Multi-turn sessions accumulate chronological `ActionEntry` records across turns, including rollbacks.
//! 3. Sessions configured with `StateOnly` context strategy preserve the bounded prompt invariant (`<= 3` messages)
//!    while surfacing the complete action ledger in `## Actions Taken So Far`.

use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::json;
use superstate_agent::context::{ContextStrategy, StateOnly};
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{Budget, Outcome, Runner};
use superstate_agent::shell::{LocalShell, Shell};
use superstate_agent::state::{ActionEntry, State};
use superstate_cli::repl::{handle_repl_command, ReplConfig, ReplControl, ReplSession};
use superstate_provider::{Message, Provider, ProviderError, Request, Response, ToolCall, Usage};

/// Replays a script of provider responses while capturing all requests for invariant inspection.
struct ScriptedProvider {
    script: Mutex<Vec<Response>>,
    seen: Mutex<Vec<Request>>,
}

impl ScriptedProvider {
    fn new(script: Vec<Response>) -> Self {
        Self {
            script: Mutex::new(script),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Provider for ScriptedProvider {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        let mut s = self.script.lock().unwrap();
        if s.is_empty() {
            return Ok(Response::default());
        }
        Ok(s.remove(0))
    }

    fn model_id(&self) -> String {
        "scripted-e2e-test".into()
    }
}

fn says(text: &str) -> Response {
    Response {
        text: text.into(),
        ..Default::default()
    }
}

fn calls(name: &str, args: serde_json::Value) -> Response {
    Response {
        tool_calls: vec![ToolCall {
            id: format!("call_{name}"),
            name: name.into(),
            arguments: args,
            signature: None,
        }],
        usage: Usage {
            input: 50,
            cached: 0,
            output: 20,
            thinking: 0,
        },
        ..Default::default()
    }
}

fn create_temp_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("failed to create temp git dir");
    let shell = LocalShell;
    shell
        .exec(
            "git init -q && git config user.email test@superstate.local && git config user.name tester",
            dir.path(),
            std::time::Duration::from_secs(30),
        )
        .expect("git init failed");
    std::fs::write(dir.path().join("README.md"), "# Test Repo\n").unwrap();
    shell
        .exec(
            "git add -A && git commit -qm 'initial commit'",
            dir.path(),
            std::time::Duration::from_secs(30),
        )
        .expect("git commit failed");
    dir
}

#[test]
fn test_repl_state_command_displays_action_ledger() {
    let config = ReplConfig {
        repo_root: PathBuf::from("."),
        model: Some("gemini-2.5-flash".to_string()),
        gate: Some("cargo test".to_string()),
        context_strategy: "state-only".to_string(),
        container: None,
        max_turns: 60,
        max_gates: 5,
        rollback: true,
    };
    let mut session = ReplSession::new(&config);

    // Populate action entries in runtime state representing diverse tool outcomes.
    session.state.runtime.actions.push(ActionEntry {
        turn: 1,
        tool: "read".into(),
        target: "src/main.rs".into(),
        summary: "25 lines read".into(),
        is_error: false,
    });
    session.state.runtime.actions.push(ActionEntry {
        turn: 2,
        tool: "bash".into(),
        target: "cargo test".into(),
        summary: "exit 0".into(),
        is_error: false,
    });
    session.state.runtime.actions.push(ActionEntry {
        turn: 3,
        tool: "note".into(),
        target: "patch (1 keys)".into(),
        summary: "recorded; your state now has 1 field(s)".into(),
        is_error: false,
    });

    let mut output = Vec::new();
    let res = handle_repl_command(&mut session, "/state", &mut output).unwrap();
    assert_eq!(res, ReplControl::Handled);

    let text = String::from_utf8(output).expect("output must be valid UTF-8");

    // Invariant: /state output displays structured sections and includes actions array.
    assert!(text.contains("=== Superstate SKILL.state ==="));
    assert!(text.contains("Runtime:"));
    assert!(
        text.contains("\"actions\": ["),
        "Runtime JSON must include actions array"
    );

    // Verify all recorded action fields are accurately represented in output.
    assert!(text.contains("\"turn\": 1"));
    assert!(text.contains("\"tool\": \"read\""));
    assert!(text.contains("\"target\": \"src/main.rs\""));
    assert!(text.contains("\"summary\": \"25 lines read\""));
    assert!(text.contains("\"is_error\": false"));

    assert!(text.contains("\"turn\": 2"));
    assert!(text.contains("\"tool\": \"bash\""));
    assert!(text.contains("\"target\": \"cargo test\""));
    assert!(text.contains("\"summary\": \"exit 0\""));

    assert!(text.contains("\"turn\": 3"));
    assert!(text.contains("\"tool\": \"note\""));
    assert!(text.contains("\"target\": \"patch (1 keys)\""));
    assert!(text.contains("\"summary\": \"recorded; your state now has 1 field(s)\""));
}

#[test]
fn test_repl_session_records_actions_across_turns() {
    let repo = create_temp_git_repo();
    let config = ReplConfig {
        repo_root: repo.path().to_path_buf(),
        model: Some("gemini-2.5-flash".to_string()),
        gate: Some("test -f artifact.txt".to_string()),
        context_strategy: "state-only".to_string(),
        container: None,
        max_turns: 10,
        max_gates: 3,
        rollback: true,
    };
    let mut session = ReplSession::new(&config);

    // Turn 1: Model creates a file using bash tool.
    // Turn 2: Gate fails (artifact.txt missing) -> triggers rollback action entry.
    // Turn 3: Model creates artifact.txt -> gate succeeds.
    let provider = ScriptedProvider::new(vec![
        calls("bash", json!({"command": "echo 'step1' > step1.txt"})),
        says("Finished first step, ready for gate"),
        calls(
            "write",
            json!({"path": "artifact.txt", "content": "verified\n"}),
        ),
        says("Artifact created, ready for final check"),
    ]);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &StateOnly,
        root: repo.path().to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 3,
        },
        rollback: true,
        on_event: None,
    };

    let (outcome, report) = runner.run(&mut session.state);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.gate_attempts, 2);
    assert_eq!(session.state.runtime.rollbacks, 1);

    // Ensure actions accumulated chronologically across all turns:
    // 1. bash (step1)
    // 2. rollback (gate failed because artifact.txt was not yet present)
    // 3. write (artifact.txt)
    assert!(
        session.state.runtime.actions.len() >= 3,
        "actions must accumulate across all turns; got {}",
        session.state.runtime.actions.len()
    );

    let tools: Vec<&str> = session
        .state
        .runtime
        .actions
        .iter()
        .map(|a| a.tool.as_str())
        .collect();
    assert_eq!(tools, vec!["bash", "rollback", "write"]);

    // Verify rollback action properties
    let rollback_entry = &session.state.runtime.actions[1];
    assert!(rollback_entry.is_error);
    assert!(rollback_entry.target.starts_with("checkpoint "));
    assert!(rollback_entry.summary.contains("Gate failed"));

    // Verify REPL /state command reflects accumulated actions
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/state", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    let state_text = String::from_utf8(out).unwrap();
    assert!(state_text.contains("\"tool\": \"bash\""));
    assert!(state_text.contains("\"tool\": \"rollback\""));
    assert!(state_text.contains("\"tool\": \"write\""));

    // Verify /clear resets the accumulated actions
    let mut clear_out = Vec::new();
    let res = handle_repl_command(&mut session, "/clear", &mut clear_out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    assert!(session.state.runtime.actions.is_empty());
}

#[test]
fn test_state_only_invariant_preservation_e2e() {
    let repo = create_temp_git_repo();
    let mut state = State::new("implement robust state tracking", "true");

    // Simulate 8 turns of tool actions to populate action history.
    let script: Vec<Response> = (1..=8)
        .map(|i| calls("bash", json!({"command": format!("echo turn_{i}")})))
        .collect();
    let provider = ScriptedProvider::new(script);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &StateOnly,
        root: repo.path().to_path_buf(),
        budget: Budget {
            max_turns: 12,
            max_gate_attempts: 2,
        },
        rollback: false,
        on_event: None,
    };

    let (outcome, _) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(state.runtime.actions.len(), 8);

    // Invariant check across all captured requests:
    // Every request produced under StateOnly MUST have messages.len() <= 3.
    let seen_requests = provider.seen.lock().unwrap();
    assert_eq!(seen_requests.len(), 9); // 8 tool turns + 1 final turn

    for (turn_idx, req) in seen_requests.iter().enumerate() {
        assert!(
            req.messages.len() <= 3,
            "Turn {} request exceeded message budget: got {} messages (expected <= 3)",
            turn_idx + 1,
            req.messages.len()
        );

        // From turn 2 onwards (after turn 1 recorded an action), prompt must contain ## Actions Taken So Far.
        if turn_idx > 0 {
            let Message::User(ref prompt) = req.messages[0] else {
                panic!("first message in StateOnly request must be Message::User");
            };
            assert!(
                prompt.contains("## Actions Taken So Far"),
                "Turn {} prompt missing action ledger header",
                turn_idx + 1
            );
            // Verify prior action is present in the prompt.
            assert!(
                prompt.contains(&format!("Turn {turn_idx}: `bash` `echo turn_{turn_idx}`")),
                "Turn {} prompt missing entry for prior turn",
                turn_idx + 1
            );
        }
    }

    // Direct context assembly check: even with substantial history, StateOnly bounds to <= 3 messages.
    let fake_history = vec![
        Message::User("initial user goal".into()),
        Message::ToolCall {
            id: "call_1".into(),
            name: "bash".into(),
            arguments: json!({"command": "echo 1"}),
            signature: None,
        },
        Message::ToolResult {
            id: "call_1".into(),
            name: "bash".into(),
            content: "output 1".into(),
            is_error: false,
        },
        Message::ToolCall {
            id: "call_2".into(),
            name: "bash".into(),
            arguments: json!({"command": "echo 2"}),
            signature: None,
        },
        Message::ToolResult {
            id: "call_2".into(),
            name: "bash".into(),
            content: "output 2".into(),
            is_error: false,
        },
    ];

    let messages = StateOnly.build(&state, &fake_history);
    assert!(
        messages.len() <= 3,
        "StateOnly.build with large history must remain <= 3, got {}",
        messages.len()
    );

    let Message::User(ref prompt) = messages[0] else {
        panic!("expected primary user message");
    };
    assert!(prompt.contains("## Actions Taken So Far"));
    for i in 1..=8 {
        assert!(prompt.contains(&format!("Turn {i}: `bash` `echo turn_{i}` -> exit 0")));
    }
}
