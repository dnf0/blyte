use blyte_agent::run::Event;
use blyte_cli::repl::{
    format_event, handle_repl_command, run_repl_stream, ReplConfig, ReplControl, ReplSession,
};
use blyte_provider::Message;

#[test]
fn test_slash_help() {
    let config = ReplConfig {
        repo_root: std::path::PathBuf::from("."),
        model: None,
        gate: None,
        context_strategy: "state-only".to_string(),
        container: None,
        max_turns: 60,
        max_gates: 5,
        rollback: true,
        orchestrate: false,
        fold_budget: None,
    };
    let mut session = ReplSession::new(&config);
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/help", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("/help"));
    assert!(text.contains("/state"));
    assert!(text.contains("/diff"));
    assert!(text.contains("/gate"));
    assert!(text.contains("/clear"));
    assert!(text.contains("/exit"));
}

#[test]
fn test_slash_gate_get_and_set() {
    let config = ReplConfig {
        repo_root: std::path::PathBuf::from("."),
        model: None,
        gate: None,
        context_strategy: "state-only".to_string(),
        container: None,
        max_turns: 60,
        max_gates: 5,
        rollback: true,
        orchestrate: false,
        fold_budget: None,
    };
    let mut session = ReplSession::new(&config);

    // Initial inspection (no gate configured)
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/gate", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    let text = String::from_utf8(out).unwrap();
    assert!(text.to_lowercase().contains("no gate"));

    // Set gate
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/gate cargo test", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    assert_eq!(session.gate.as_deref(), Some("cargo test"));
    assert_eq!(session.state.orchestrator.verification_cmd, "cargo test");

    // Inspect updated gate
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/gate", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("cargo test"));
}

#[test]
fn test_slash_state_inspection() {
    let config = ReplConfig {
        repo_root: std::path::PathBuf::from("."),
        model: None,
        gate: Some("cargo test".to_string()),
        context_strategy: "state-only".to_string(),
        container: None,
        max_turns: 60,
        max_gates: 5,
        rollback: true,
        orchestrate: false,
        fold_budget: None,
    };
    let mut session = ReplSession::new(&config);
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/state", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    let text = String::from_utf8(out).unwrap();
    assert!(text.to_lowercase().contains("orchestrator"));
    assert!(text.to_lowercase().contains("runtime"));
}

#[test]
fn test_slash_exit() {
    let config = ReplConfig {
        repo_root: std::path::PathBuf::from("."),
        model: None,
        gate: None,
        context_strategy: "state-only".to_string(),
        container: None,
        max_turns: 60,
        max_gates: 5,
        rollback: true,
        orchestrate: false,
        fold_budget: None,
    };
    let mut session = ReplSession::new(&config);

    let mut out = Vec::new();
    let res1 = handle_repl_command(&mut session, "/exit", &mut out).unwrap();
    assert_eq!(res1, ReplControl::Exit);

    let mut out = Vec::new();
    let res2 = handle_repl_command(&mut session, "/quit", &mut out).unwrap();
    assert_eq!(res2, ReplControl::Exit);
}

#[test]
fn test_repl_stream_driving() {
    let config = ReplConfig {
        repo_root: std::path::PathBuf::from("."),
        model: None,
        gate: None,
        context_strategy: "state-only".to_string(),
        container: None,
        max_turns: 60,
        max_gates: 5,
        rollback: true,
        orchestrate: false,
        fold_budget: None,
    };
    let session = ReplSession::new(&config);

    let input = b"/help\n/gate cargo test\n/state\n/exit\n";
    let reader = std::io::Cursor::new(input);
    let mut writer = Vec::new();

    let res = run_repl_stream(reader, &mut writer, session);
    assert!(res.is_ok());

    let output = String::from_utf8(writer).unwrap();
    assert!(output.contains("blyte > "));
    assert!(output.contains("/help"));
    assert!(output.contains("Gate set to: cargo test"));
}

#[test]
fn test_format_event() {
    let e1 = Event::Turn {
        n: 1,
        text: "Thinking about plan",
    };
    let s1 = format_event(&e1);
    assert!(s1.contains('1') && s1.contains("Thinking about plan"));

    let e2 = Event::Tool {
        name: "bash",
        input: "git status".to_string(),
        summary: "clean working tree".to_string(),
        is_error: false,
    };
    let s2 = format_event(&e2);
    assert!(s2.contains('●') && s2.contains("bash") && s2.contains("git status"));

    let e3 = Event::Tool {
        name: "bash",
        input: "cargo test".to_string(),
        summary: "exit 101".to_string(),
        is_error: true,
    };
    let s3 = format_event(&e3);
    assert!(s3.contains('●') && s3.contains("cargo test"));

    let e4 = Event::Gate {
        attempt: 1,
        passed: true,
        output: "ok",
    };
    let s4 = format_event(&e4);
    assert!(s4.contains("✓ Gate passed"));

    let e5 = Event::Gate {
        attempt: 2,
        passed: false,
        output: "failed",
    };
    let s5 = format_event(&e5);
    assert!(s5.contains("✗ Gate failed"));

    let e6 = Event::RolledBack { to: "abc1234" };
    let s6 = format_event(&e6);
    assert!(s6.contains("↺ Rolled back working tree to abc1234"));
}

#[test]
fn test_format_event_turn_full_text() {
    let text = "x".repeat(299) + "Z";
    let event = Event::Turn { n: 3, text: &text };
    let formatted = format_event(&event);
    assert_eq!(formatted, format!("[Turn 3] {text}"));
    assert!(formatted.contains('Z'));
    assert!(!formatted.ends_with("..."));
}

#[test]
fn test_format_event_turn_multiline() {
    let text = "Plan:\n1. a\n2. b\nShall I go ahead?";
    let event = Event::Turn { n: 1, text };
    assert_eq!(
        format_event(&event),
        "[Turn 1] Plan:\n1. a\n2. b\nShall I go ahead?"
    );
}

#[test]
fn test_format_event_turn_multibyte() {
    let text = "é".repeat(100);
    let event = Event::Turn { n: 1, text: &text };
    let formatted = format_event(&event);
    assert_eq!(formatted.chars().count(), "[Turn 1] ".len() + 100);
}

#[test]
fn test_prompt_submission() {
    let config = ReplConfig {
        repo_root: std::path::PathBuf::from("."),
        model: None,
        gate: None,
        context_strategy: "state-only".to_string(),
        container: None,
        max_turns: 60,
        max_gates: 5,
        rollback: true,
        orchestrate: false,
        fold_budget: None,
    };
    let mut session = ReplSession::new(&config);
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "Fix the broken parser", &mut out).unwrap();
    assert_eq!(
        res,
        ReplControl::SubmitPrompt("Fix the broken parser".to_string())
    );
}

#[test]
fn a_gate_start_renders_as_gate_running() {
    let e = Event::GateStart { attempt: 2 };
    assert_eq!(format_event(&e), "⏳ Gate running (attempt 2)");
}

#[test]
fn test_clear_starts_new_conversation() {
    let config = ReplConfig {
        repo_root: std::path::PathBuf::from("."),
        model: None,
        gate: None,
        context_strategy: "state-only".to_string(),
        container: None,
        max_turns: 60,
        max_gates: 5,
        rollback: true,
        orchestrate: false,
        fold_budget: None,
    };
    let mut session = ReplSession::new(&config);
    session.state.runtime.conversation = Some(vec![Message::User("hi".into())]);
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/clear", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    assert_eq!(session.state.runtime.conversation, Some(Vec::new()));
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("Conversation cleared"));
}

#[test]
fn test_state_counts_conversation_messages() {
    let config = ReplConfig {
        repo_root: std::path::PathBuf::from("."),
        model: None,
        gate: None,
        context_strategy: "state-only".to_string(),
        container: None,
        max_turns: 60,
        max_gates: 5,
        rollback: true,
        orchestrate: false,
        fold_budget: None,
    };
    let mut session = ReplSession::new(&config);
    session.state.runtime.conversation = Some(vec![
        Message::User("secret question".into()),
        Message::Assistant("secret answer".into()),
    ]);
    let mut out = Vec::new();
    let res = handle_repl_command(&mut session, "/state", &mut out).unwrap();
    assert_eq!(res, ReplControl::Handled);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("conversation: 2 messages"));
    assert!(!text.contains("secret question"));
    assert!(!text.contains("secret answer"));
}
