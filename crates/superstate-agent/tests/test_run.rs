//! The loop, driven by a scripted provider so the behaviour is deterministic.

use std::sync::Mutex;

use serde_json::json;
use superstate_agent::context::{ContextStrategy, Conversational, StateOnly};
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{Budget, Outcome, Runner};
use superstate_agent::shell::{LocalShell, Shell};
use superstate_agent::state::{ActionEntry, State};
use superstate_provider::{Message, Provider, ProviderError, Request, Response, ToolCall, Usage};

/// Replays a fixed list of responses and records every request it was given.
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
            // Stopping with no tool calls is how the model signals it is done.
            return Ok(Response::default());
        }
        Ok(s.remove(0))
    }
    fn model_id(&self) -> String {
        "scripted".into()
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
            id: "c0".into(),
            name: name.into(),
            arguments: args,
            signature: None,
        }],
        usage: Usage {
            input: 100,
            cached: 0,
            output: 10,
            thinking: 0,
        },
        ..Default::default()
    }
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let sh = LocalShell;
    sh.exec(
        "git init -q && git config user.email a@b && git config user.name c",
        d.path(),
        std::time::Duration::from_secs(30),
    )
    .unwrap();
    std::fs::write(d.path().join("marker"), "0\n").unwrap();
    sh.exec(
        "git add -A && git commit -qm base",
        d.path(),
        std::time::Duration::from_secs(30),
    )
    .unwrap();
    d
}

fn runner<'a>(
    p: &'a ScriptedProvider,
    root: &std::path::Path,
    ctx: &'a dyn ContextStrategy,
    rollback: bool,
) -> Runner<'a> {
    Runner {
        provider: p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: ctx,
        root: root.to_path_buf(),
        budget: Budget {
            max_turns: 12,
            max_gate_attempts: 3,
        },
        rollback,
        on_event: None,
    }
}

#[test]
fn the_gate_is_not_in_the_tool_surface() {
    // Invariant II. A gate the model can invoke is one it can decline to invoke, invoke
    // selectively, or report the result of. It must not be reachable at all.
    let names: Vec<String> = superstate_agent::tools::specs()
        .into_iter()
        .map(|t| t.name)
        .collect();
    for forbidden in [
        "verify",
        "gate",
        "check",
        "run_tests",
        "complete",
        "done",
        "finish",
    ] {
        assert!(
            !names.iter().any(|n| n == forbidden),
            "`{forbidden}` must not be callable by the model: {names:?}"
        );
    }
}

#[test]
fn stopping_triggers_the_gate_and_passing_it_ends_the_run() {
    let d = repo();
    let p = ScriptedProvider::new(vec![says("I think that is done.")]);
    let mut st = State::new("make it work", "true");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, true).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 1);
}

#[test]
fn the_model_cannot_finish_by_claiming_to_have_finished() {
    // It says it is done, confidently, repeatedly. The command disagrees, and the command wins.
    let d = repo();
    let p = ScriptedProvider::new(vec![
        says("Done! All tests pass."),
        says("Definitely finished now."),
        says("Complete."),
    ]);
    let mut st = State::new("fix it", "false");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::GateFailed);
    assert_eq!(
        rep.gate_attempts, 3,
        "every claim was tested, none accepted"
    );
}

#[test]
fn a_gate_that_cannot_run_is_a_failure_not_a_pass() {
    // Treating an execution error as success is how an ungated run gets mistaken for a
    // verified one -- the defect that made a whole family of earlier results unreadable.
    let d = repo();
    let p = ScriptedProvider::new(vec![says("done")]);
    let mut st = State::new("g", "this-command-does-not-exist-anywhere");
    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::GateFailed);
    assert!(
        st.runtime.failures[0].contains("exit"),
        "the failure must be recorded verbatim"
    );
}

#[test]
fn a_failed_gate_resets_the_tree_to_the_checkpoint() {
    // Invariant III: a failing attempt's edits must not survive into the next one.
    let d = repo();
    let p = ScriptedProvider::new(vec![
        calls(
            "write",
            json!({"path": "marker", "content": "vandalised\n"}),
        ),
        says("done"),
    ]);
    let mut st = State::new("g", "false");
    let (_, _) = runner(&p, d.path(), &Conversational, true).run(&mut st);
    assert_eq!(
        std::fs::read_to_string(d.path().join("marker")).unwrap(),
        "0\n",
        "the working tree must be back at the checkpoint"
    );
    assert!(st.runtime.rollbacks >= 1);
}

#[test]
fn rollback_can_be_switched_off_because_it_is_not_free() {
    // One run reset a worker's COMMITTED feature five times (FINDINGS 13.2), so the policy is
    // a switch rather than a law.
    let d = repo();
    let p = ScriptedProvider::new(vec![
        calls("write", json!({"path": "marker", "content": "kept\n"})),
        says("done"),
    ]);
    let mut st = State::new("g", "false");
    let (_, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(
        std::fs::read_to_string(d.path().join("marker")).unwrap(),
        "kept\n"
    );
    assert_eq!(st.runtime.rollbacks, 0);
}

#[test]
fn the_worker_is_told_when_its_edits_were_reverted() {
    // Silently resetting leaves the model editing against a file it no longer has, and its
    // next `edit` fails on a string that is gone.
    let d = repo();
    let p = ScriptedProvider::new(vec![
        calls("write", json!({"path": "marker", "content": "x\n"})),
        says("done"),
        says("done"),
    ]);
    let mut st = State::new("g", "false");
    runner(&p, d.path(), &Conversational, true).run(&mut st);
    let last = p.seen.lock().unwrap().last().cloned().unwrap();
    let text = format!("{:?}", last.messages);
    assert!(
        text.contains("reverted"),
        "the reset must be stated to the worker: {text:.400}"
    );
}

#[test]
fn note_writes_only_the_agent_region() {
    let d = repo();
    let p = ScriptedProvider::new(vec![
        calls(
            "note",
            json!({"patch": {"cause": "span width", "verification_cmd": "true"}}),
        ),
        says("done"),
    ]);
    let mut st = State::new("g", "false");
    runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(st.agent.fields["cause"], "span width");
    assert_eq!(
        st.orchestrator.verification_cmd, "false",
        "the gate must be untouched"
    );
}

#[test]
fn the_system_prompt_states_the_gate_without_offering_it() {
    let d = repo();
    let p = ScriptedProvider::new(vec![says("done")]);
    let mut st = State::new("fix the parser", "npm test");
    runner(&p, d.path(), &Conversational, false).run(&mut st);
    let sys = p.seen.lock().unwrap()[0].system.clone();
    assert!(
        sys.contains("npm test"),
        "the worker must know the standard it is held to"
    );
    assert!(sys.contains("you do not decide the verdict"));
    assert!(
        sys.contains("Available:"),
        "the environment probe must reach the prompt"
    );
}

#[test]
fn state_only_context_does_not_grow_with_the_run() {
    // The whole claim of SKILL.state: constant prompt footprint. Conversational grows; this
    // must not.
    let d = repo();
    let script: Vec<_> = (0..6)
        .map(|i| calls("bash", json!({"command": format!("echo {i}")})))
        .collect();
    let p = ScriptedProvider::new(script);
    let mut st = State::new("g", "true");
    runner(&p, d.path(), &StateOnly, false).run(&mut st);
    let seen = p.seen.lock().unwrap();
    let sizes: Vec<usize> = seen.iter().map(|r| r.messages.len()).collect();
    assert!(
        sizes.iter().all(|&n| n <= 3),
        "state-only must stay bounded regardless of turn count, got {sizes:?}"
    );
}

#[test]
fn conversational_context_does_grow_which_is_the_thing_being_compared() {
    let d = repo();
    let script: Vec<_> = (0..6)
        .map(|i| calls("bash", json!({"command": format!("echo {i}")})))
        .collect();
    let p = ScriptedProvider::new(script);
    let mut st = State::new("g", "true");
    runner(&p, d.path(), &Conversational, false).run(&mut st);
    let seen = p.seen.lock().unwrap();
    let first = seen.first().unwrap().messages.len();
    let last = seen.last().unwrap().messages.len();
    assert!(
        last > first,
        "the baseline is meant to accumulate: {first} -> {last}"
    );
}

#[test]
fn a_provider_error_is_reported_as_an_error_not_as_failed_work() {
    struct Broken;
    impl Provider for Broken {
        fn complete(&self, _: &Request) -> Result<Response, ProviderError> {
            Err(ProviderError::Transport("network down".into()))
        }
        fn model_id(&self) -> String {
            "broken".into()
        }
    }
    let d = repo();
    let mut st = State::new("g", "true");
    let r = Runner {
        provider: &Broken,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget::default(),
        rollback: false,
        on_event: None,
    };
    let (outcome, _) = r.run(&mut st);
    assert!(
        matches!(outcome, Outcome::Error(_)),
        "infrastructure failure must not read as a
                     failed task: {outcome:?}"
    );
}

#[test]
fn the_turn_budget_bounds_a_model_that_never_stops() {
    let d = repo();
    let script: Vec<_> = (0..500)
        .map(|_| calls("bash", json!({"command": "true"})))
        .collect();
    let p = ScriptedProvider::new(script);
    let mut st = State::new("g", "true");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert_eq!(rep.outcome_turns, 12);
}

#[test]
fn test_actions_recorded_during_run() {
    // Verifies that Runner records ActionEntry objects chronologically into state.runtime.actions
    // for all invoked tools (bash, read, note), preserving action targets, summaries, and error flags.
    let d = repo();
    // Create a known file on disk so the 'read' tool succeeds with predictable line counts.
    std::fs::write(d.path().join("sample.txt"), "first line\nsecond line\n").unwrap();

    let p = ScriptedProvider::new(vec![
        calls("bash", json!({"command": "echo hello"})),
        calls("read", json!({"path": "sample.txt"})),
        calls("note", json!({"patch": {"analysis": "verified"}})),
        says("done"),
    ]);
    let mut st = State::new("test action ledger", "true");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.tool_calls, 3);

    // Ensure three action entries were captured in exact invocation sequence.
    assert_eq!(st.runtime.actions.len(), 3);

    // 1. Validate bash command action entry details.
    let bash_action = &st.runtime.actions[0];
    assert_eq!(bash_action.turn, 1);
    assert_eq!(bash_action.tool, "bash");
    assert_eq!(bash_action.target, "echo hello");
    assert_eq!(bash_action.summary, "exit 0");
    assert!(!bash_action.is_error);

    // 2. Validate file read action entry details.
    let read_action = &st.runtime.actions[1];
    assert_eq!(read_action.turn, 2);
    assert_eq!(read_action.tool, "read");
    assert_eq!(read_action.target, "sample.txt");
    assert_eq!(read_action.summary, "2 lines read");
    assert!(!read_action.is_error);

    // 3. Validate state note action entry details.
    let note_action = &st.runtime.actions[2];
    assert_eq!(note_action.turn, 3);
    assert_eq!(note_action.tool, "note");
    assert_eq!(note_action.target, "patch (1 keys)");
    assert_eq!(
        note_action.summary,
        "recorded; your state now has 1 field(s)"
    );
    assert!(!note_action.is_error);
}

#[test]
fn test_rollback_appends_action_entry() {
    // Verifies that when a verification gate fails with rollback enabled, an ActionEntry
    // is recorded in state.runtime.actions documenting the rollback event and checkpoint target.
    let d = repo();
    // The verification command checks for existence of 'target.txt'.
    // On the first gate attempt, target.txt is missing -> gate fails and rollback occurs.
    // On the second attempt, the model writes target.txt -> gate passes.
    let p = ScriptedProvider::new(vec![
        calls("bash", json!({"command": "echo attempt1"})),
        says("done with attempt 1"),
        calls(
            "write",
            json!({"path": "target.txt", "content": "passed\n"}),
        ),
        says("done with attempt 2"),
    ]);
    let mut st = State::new("create target file", "test -f target.txt");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, true).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 2);
    assert_eq!(st.runtime.rollbacks, 1);

    // Identify the rollback entry in the recorded action history.
    let rollback_entry = st
        .runtime
        .actions
        .iter()
        .find(|a| a.tool == "rollback")
        .expect("rollback action entry must be recorded in runtime.actions");

    assert!(rollback_entry.target.starts_with("checkpoint "));
    assert_eq!(
        rollback_entry.summary,
        "Gate failed; working tree reset to verified state"
    );
    assert!(rollback_entry.is_error);
}

#[test]
fn test_state_only_context_omits_actions_when_empty() {
    // When state.runtime.actions is empty, StateOnly::build must omit the action ledger section
    // to preserve a minimal, clean prompt for the initial turn or until actions occur.
    let state = State::new("my goal", "cargo test");
    let history = vec![];
    let messages = StateOnly.build(&state, &history);
    assert_eq!(messages.len(), 1);

    let Message::User(prompt) = &messages[0] else {
        panic!("expected Message::User");
    };
    assert!(!prompt.contains("## Actions Taken So Far"));
}

#[test]
fn test_state_only_context_formats_action_ledger() {
    // When actions exist, StateOnly::build formats them chronologically into the initial user prompt,
    // retaining Invariant II by keeping the total message count bounded (messages.len() <= 3).
    let mut state = State::new("solve bug", "pytest");
    state.runtime.actions.push(ActionEntry {
        turn: 1,
        tool: "read".into(),
        target: "src/main.rs (lines 1-20)".into(),
        summary: "20 lines read".into(),
        is_error: false,
    });
    state.runtime.actions.push(ActionEntry {
        turn: 2,
        tool: "bash".into(),
        target: "cargo test".into(),
        summary: "exit 101".into(),
        is_error: true,
    });

    // Provide history with tool call and result to verify bounded message length.
    let history = vec![
        Message::User("initial user message".into()),
        Message::ToolCall {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: json!({"command": "cargo test"}),
            signature: None,
        },
        Message::ToolResult {
            id: "tc_1".into(),
            name: "bash".into(),
            content: "test failure".into(),
            is_error: true,
        },
    ];

    let messages = StateOnly.build(&state, &history);
    assert!(
        messages.len() <= 3,
        "messages.len() must be <= 3, got {}",
        messages.len()
    );

    let Message::User(prompt) = &messages[0] else {
        panic!("expected first message to be Message::User");
    };
    assert!(prompt.contains("## Actions Taken So Far"));
    assert!(prompt.contains("- Turn 1: `read` `src/main.rs (lines 1-20)` -> 20 lines read"));
    assert!(prompt.contains("- Turn 2: `bash` `cargo test` -> exit 101 [ERROR]"));
}

#[test]
fn test_state_only_runner_injects_action_ledger_in_prompts() {
    // Verifies end-to-end integration: Runner executing with StateOnly strategy injects
    // actions executed in turn 1 into the turn 2 request prompt.
    let d = repo();
    let p = ScriptedProvider::new(vec![
        calls("bash", json!({"command": "echo step1"})),
        says("done"),
    ]);
    let mut st = State::new("step goal", "true");
    let (outcome, rep) = runner(&p, d.path(), &StateOnly, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 1);

    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);

    // Turn 1 prompt must not contain actions as none have run yet.
    let Message::User(first_prompt) = &seen[0].messages[0] else {
        panic!("expected Message::User in turn 1");
    };
    assert!(!first_prompt.contains("## Actions Taken So Far"));

    // Turn 2 prompt must contain the action ledger capturing turn 1's bash invocation.
    let Message::User(second_prompt) = &seen[1].messages[0] else {
        panic!("expected Message::User in turn 2");
    };
    assert!(second_prompt.contains("## Actions Taken So Far"));
    assert!(second_prompt.contains("- Turn 1: `bash` `echo step1` -> exit 0"));
}

#[test]
fn test_working_set_synchronized_during_run() {
    // Verifies that during a run, calling read tracks the file snippet with line numbers stripped,
    // calling edit updates multi-line cached content in memory, and running a passing test leaves
    // last_failure clear.
    let d = repo();
    std::fs::write(
        d.path().join("calc.rs"),
        "fn calculate() -> i32 {\n    let val = 0;\n    val\n}\n",
    )
    .unwrap();

    let p = ScriptedProvider::new(vec![
        calls("read", json!({"path": "calc.rs"})),
        calls(
            "edit",
            json!({
                "path": "calc.rs",
                "old": "    let val = 0;\n    val",
                "new": "    let val = 42;\n    val"
            }),
        ),
        calls("bash", json!({"command": "cargo test --help"})),
        says("done"),
    ]);
    let mut st = State::new("sync working set", "true");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 1);

    // Working set must contain the edited file snippet with multi-line updated content without line-number prefixes
    assert_eq!(st.runtime.working_set.files.len(), 1);
    assert_eq!(st.runtime.working_set.files[0].path, "calc.rs");
    assert!(st.runtime.working_set.files[0].content.contains("let val = 42;"));
    assert!(!st.runtime.working_set.files[0].content.contains("let val = 0;"));
    assert_eq!(st.runtime.working_set.files[0].last_accessed_turn, 2);
    assert!(st.runtime.working_set.last_failure.is_none());
}

#[test]
fn test_working_set_tracks_test_failure() {
    // When a bash command executing tests fails, the harness automatically captures
    // and distills the failure output into state.runtime.working_set.last_failure.
    let d = repo();
    let p = ScriptedProvider::new(vec![
        calls("bash", json!({"command": "cargo test"})),
        says("done"),
    ]);
    let mut st = State::new("track failure", "true");
    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);

    let failure = st
        .runtime
        .working_set
        .last_failure
        .as_deref()
        .expect("last_failure should be populated");
    assert!(failure.contains("could not find `Cargo.toml`") || failure.contains("exit"));
}

#[test]
fn test_working_set_clears_test_failure_on_pass() {
    // A subsequent passing test execution clears the previously recorded failure
    // from external working memory.
    let d = repo();
    let p = ScriptedProvider::new(vec![
        calls("bash", json!({"command": "cargo test"})),
        calls("bash", json!({"command": "cargo test --help"})),
        says("done"),
    ]);
    let mut st = State::new("clear failure", "true");
    st.runtime
        .working_set
        .record_failure("initial failure to clear");
    assert!(st.runtime.working_set.last_failure.is_some());

    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);

    assert!(
        st.runtime.working_set.last_failure.is_none(),
        "passing test command must clear last_failure"
    );
}

#[test]
fn test_benign_commands_do_not_affect_working_set_failure() {
    // Commands operating on file paths containing 'test' (e.g. ls tests/, cat tests/foo, grep "test")
    // must not falsely record failures or prematurely clear an active test failure.
    let d = repo();
    std::fs::create_dir_all(d.path().join("tests")).unwrap();
    std::fs::write(d.path().join("tests/foo.rs"), "test helper\n").unwrap();

    let p = ScriptedProvider::new(vec![
        // Benign command that exits 0
        calls("bash", json!({"command": "ls tests/"})),
        // Benign command that exits non-zero (grep returns 1 when pattern not found)
        calls(
            "bash",
            json!({"command": "grep 'nonexistent_pattern' tests/foo.rs"}),
        ),
        says("done"),
    ]);
    let mut st = State::new("preserve failure", "true");
    st.runtime
        .working_set
        .record_failure("real compiler test failure");

    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);

    assert_eq!(
        st.runtime.working_set.last_failure.as_deref(),
        Some("real compiler test failure"),
        "benign commands touching tests/ or grep must not alter last_failure"
    );
}

#[test]
fn test_working_set_updates_on_write() {
    // Overwriting a file via the write tool updates the cached snippet in the working set.
    let d = repo();
    let p = ScriptedProvider::new(vec![
        calls("read", json!({"path": "marker"})),
        calls(
            "write",
            json!({"path": "marker", "content": "rewritten marker content\n"}),
        ),
        says("done"),
    ]);
    let mut st = State::new("write working set", "true");
    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);

    assert_eq!(st.runtime.working_set.files.len(), 1);
    assert_eq!(st.runtime.working_set.files[0].path, "marker");
    assert_eq!(
        st.runtime.working_set.files[0].content,
        "rewritten marker content\n"
    );
}

#[test]
fn test_working_set_normalizes_leading_dot_slash() {
    // Reading with './marker' and editing with 'marker' must resolve to the same snippet entry.
    let d = repo();
    let p = ScriptedProvider::new(vec![
        calls("read", json!({"path": "./marker"})),
        calls(
            "edit",
            json!({"path": "marker", "old": "0", "new": "99"}),
        ),
        says("done"),
    ]);
    let mut st = State::new("normalize path", "true");
    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);

    assert_eq!(st.runtime.working_set.files.len(), 1);
    assert_eq!(st.runtime.working_set.files[0].path, "marker");
    assert!(st.runtime.working_set.files[0].content.contains("99"));
}

#[test]
fn test_working_set_multiline_edit_at_eof_with_trailing_newline() {
    // Multi-line edit at EOF with trailing newline matches preserved raw file snippet content.
    let d = repo();
    std::fs::write(
        d.path().join("eof.py"),
        "def main():\n    print('hello')\n",
    )
    .unwrap();

    let p = ScriptedProvider::new(vec![
        calls("read", json!({"path": "eof.py"})),
        calls(
            "edit",
            json!({
                "path": "eof.py",
                "old": "    print('hello')\n",
                "new": "    print('goodbye')\n"
            }),
        ),
        says("done"),
    ]);
    let mut st = State::new("eof edit", "true");
    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);

    assert_eq!(st.runtime.working_set.files.len(), 1);
    assert_eq!(
        st.runtime.working_set.files[0].content,
        "def main():\n    print('goodbye')\n"
    );
}

#[test]
fn test_package_manager_and_flagged_test_commands() {
    // Verifies that:
    // 1. `npm run build` does NOT clear or record test failures
    // 2. `cargo -q test` (flags before subcommand) IS detected as a test command
    // 3. Benign commands containing 'true' (e.g. `rm -rf tmp || true`) do NOT clear failures
    //    even when verification_cmd is "true".
    let d = repo();
    let p = ScriptedProvider::new(vec![
        // Command with 'true' substring should not clear failure when verification_cmd is "true"
        calls("bash", json!({"command": "rm -rf nonexistent_dir || true"})),
        // npm run build is not a test runner
        calls("bash", json!({"command": "echo 'build ok' && exit 0"})),
        // cargo with flag before test subcommand is a test runner and clears failure
        calls("bash", json!({"command": "cargo -q test --help"})),
        says("done"),
    ]);
    let mut st = State::new("pkg manager test", "true");
    st.runtime
        .working_set
        .record_failure("initial compiler test failure");

    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);

    assert!(
        st.runtime.working_set.last_failure.is_none(),
        "cargo -q test --help must clear test failure"
    );
}

#[test]
fn test_is_test_command_unit() {
    use superstate_agent::run::is_test_command;

    // Direct runners
    assert!(is_test_command("pytest", "true"));
    assert!(is_test_command("pytest tests/test_foo.py", "true"));
    assert!(is_test_command("./venv/bin/pytest", "true"));
    assert!(is_test_command("jest", "true"));
    assert!(is_test_command("vitest", "true"));

    // Subcommands with flags
    assert!(is_test_command("cargo test", "true"));
    assert!(is_test_command("cargo -q test", "true"));
    assert!(is_test_command("cargo +nightly test --all", "true"));
    assert!(is_test_command("go test ./...", "true"));
    assert!(is_test_command("npm test", "true"));
    assert!(is_test_command("npm run test", "true"));
    assert!(is_test_command("yarn run --silent test", "true"));
    assert!(is_test_command("python -m unittest", "true"));
    assert!(is_test_command("python3 -m pytest", "true"));
    assert!(is_test_command("make -C build test", "true"));

    // Exact verification command match
    assert!(is_test_command("cargo check", "cargo check"));
    assert!(is_test_command("echo step && cargo check", "cargo check"));

    // Benign commands that must NOT be treated as test commands
    assert!(!is_test_command("npm run build", "true"));
    assert!(!is_test_command("npm run lint", "true"));
    assert!(!is_test_command("yarn start", "true"));
    assert!(!is_test_command("ls tests/", "true"));
    assert!(!is_test_command("cat tests/foo.py", "true"));
    assert!(!is_test_command("git checkout test-branch", "true"));
    assert!(!is_test_command("grep 'test' marker", "true"));
    assert!(!is_test_command("rm -rf tmp || true", "true"));
    assert!(!is_test_command("echo '{\"valid\": true}'", "true"));
}



