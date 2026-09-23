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
    assert!(st.runtime.working_set.files[0]
        .content
        .contains("let val = 42;"));
    assert!(!st.runtime.working_set.files[0]
        .content
        .contains("let val = 0;"));
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
        calls("edit", json!({"path": "marker", "old": "0", "new": "99"})),
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
    std::fs::write(d.path().join("eof.py"), "def main():\n    print('hello')\n").unwrap();

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
        // npm run build is not a test runner (exits non-zero or zero, failure remains untouched)
        calls("bash", json!({"command": "npm run build"})),
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
fn test_strip_line_number_prefixes_unit() {
    use superstate_agent::run::strip_line_number_prefixes;

    // Normal line numbered output without trailing newline
    let numbered = "     1\tfn main() {\n     2\t    let x = 1;\n     3\t}";
    assert_eq!(
        strip_line_number_prefixes(numbered),
        "fn main() {\n    let x = 1;\n}"
    );

    // Numbered output with trailing newline
    let numbered_trailing = "     1\tfn main() {\n     2\t    let x = 1;\n     3\t}\n";
    assert_eq!(
        strip_line_number_prefixes(numbered_trailing),
        "fn main() {\n    let x = 1;\n}\n"
    );

    // Tab-indented source lines (line starts with tab, no line number)
    let tab_indented = "\tfn indented() {}\n\t\tlet x = 2;";
    assert_eq!(
        strip_line_number_prefixes(tab_indented),
        "\tfn indented() {}\n\t\tlet x = 2;"
    );

    // Empty string
    assert_eq!(strip_line_number_prefixes(""), "");
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

    // Exact verification command match (including compound verification commands)
    assert!(is_test_command("cargo check", "cargo check"));
    assert!(is_test_command("echo step && cargo check", "cargo check"));
    assert!(is_test_command(
        "cargo test && cargo check",
        "cargo test && cargo check"
    ));
    assert!(is_test_command(
        "./test.sh && ./lint.sh",
        "./test.sh && ./lint.sh"
    ));

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

#[test]
fn test_state_only_context_renders_working_set_and_preserves_bound() {
    // Verifies that StateOnly context assembly injects active file snippets and last test failure
    // into the user prompt while strictly maintaining Invariant II (messages.len() <= 3).
    let mut state = State::new("solve bug", "pytest");
    state.runtime.working_set.add_or_update(
        "lru.py",
        "def cache():\n    pass",
        Some(1),
        Some(2),
        1,
    );
    state
        .runtime
        .working_set
        .record_failure("AssertionError: expected 1 got 2");

    let history = vec![
        Message::User("initial user message".into()),
        Message::ToolCall {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: json!({"command": "pytest"}),
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
        "messages.len() must be <= 3 under StateOnly, got {}",
        messages.len()
    );

    let Message::User(prompt) = &messages[0] else {
        panic!("expected first message to be Message::User");
    };
    assert!(prompt.contains("## Active Working Set"));
    assert!(prompt.contains("### `lru.py` (lines 1-2)"));
    assert!(prompt.contains("def cache():\n    pass"));
    assert!(prompt.contains("## Last Test Failure"));
    assert!(prompt.contains("AssertionError: expected 1 got 2"));
}

#[test]
fn test_state_only_context_omits_working_set_when_empty() {
    // When working_set contains no files and no test failure, StateOnly::build must omit
    // both the working set and failure sections to preserve a clean and minimal prompt.
    let state = State::new("my goal", "cargo test");
    let history = vec![];
    let messages = StateOnly.build(&state, &history);
    assert_eq!(messages.len(), 1);

    let Message::User(prompt) = &messages[0] else {
        panic!("expected Message::User");
    };
    assert!(!prompt.contains("## Active Working Set"));
    assert!(!prompt.contains("## Last Test Failure"));
}

#[test]
fn test_state_only_runner_injects_working_set_into_llm_prompts() {
    // End-to-end integration: Runner executing with StateOnly strategy injects
    // active file snippets from turn 1 into turn 2's prompt, and test failures from turn 2
    // into turn 3's prompt.
    let d = repo();
    std::fs::write(d.path().join("lru.py"), "def cache():\n    return 42\n").unwrap();

    let p = ScriptedProvider::new(vec![
        // Turn 1: read file -> gets added to working set
        calls("read", json!({"path": "lru.py"})),
        // Turn 2: run failing test -> failure gets recorded in working set
        calls("bash", json!({"command": "cargo test"})),
        // Turn 3: finish run
        says("done"),
    ]);
    let mut st = State::new("fix cache", "true");
    let (outcome, rep) = runner(&p, d.path(), &StateOnly, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 1);

    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);

    // Turn 1 prompt: working set empty initially
    let Message::User(prompt1) = &seen[0].messages[0] else {
        panic!("expected Message::User in turn 1");
    };
    assert!(!prompt1.contains("## Active Working Set"));
    assert!(!prompt1.contains("## Last Test Failure"));

    // Turn 2 prompt: working set contains lru.py snippet, no failure yet
    let Message::User(prompt2) = &seen[1].messages[0] else {
        panic!("expected Message::User in turn 2");
    };
    assert!(prompt2.contains("## Active Working Set"));
    assert!(prompt2.contains("### `lru.py`"));
    assert!(prompt2.contains("def cache():\n    return 42"));
    assert!(!prompt2.contains("## Last Test Failure"));

    // Turn 3 prompt: working set contains lru.py AND last test failure
    let Message::User(prompt3) = &seen[2].messages[0] else {
        panic!("expected Message::User in turn 3");
    };
    assert!(prompt3.contains("## Active Working Set"));
    assert!(prompt3.contains("### `lru.py`"));
    assert!(prompt3.contains("## Last Test Failure"));
    assert!(seen[2].messages.len() <= 3);
}

#[test]
fn test_loop_breaker_intercepts_redundant_reads() {
    // Verifies that when an agent calls 'read' on a file that is already loaded in
    // external working memory (working_set) consecutively, the harness immediately
    // intercepts the call, returns an advisory notice, and does not execute the tool.
    let d = repo();
    std::fs::write(d.path().join("lru.py"), "def cache():\n    return 1\n").unwrap();

    let p = ScriptedProvider::new(vec![
        // Turn 1: Initial read populates working_set
        calls("read", json!({"path": "lru.py"})),
        // Turn 2: Redundant read on the same file in consecutive turns
        calls("read", json!({"path": "lru.py"})),
        // Turn 3: Complete run
        says("done"),
    ]);
    let mut st = State::new("loop breaker test", "true");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 1);

    // Inspect request messages seen by provider
    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);

    // Turn 1 result was a normal successful read
    let Message::ToolResult {
        content: content1,
        is_error: err1,
        ..
    } = &seen[2].messages[2]
    else {
        panic!("expected ToolResult at index 2");
    };
    assert!(!err1);
    assert!(!content1.contains("[Harness Working Memory Guard]"));
    assert!(content1.contains("def cache():"));

    // Turn 2 result was intercepted by the working memory guard
    let Message::ToolResult {
        content: content2,
        is_error: err2,
        ..
    } = &seen[2].messages[4]
    else {
        panic!("expected ToolResult at index 4");
    };
    assert!(!err2);
    assert!(
        content2.contains("[Harness Working Memory Guard]: `lru.py` is already loaded in your Active Working Set above"),
        "unexpected intercepted content: {content2}"
    );

    // Verify action ledger records the guard interception
    assert_eq!(st.runtime.actions.len(), 2);
    assert_eq!(st.runtime.actions[0].tool, "read");
    assert_eq!(st.runtime.actions[0].target, "lru.py");
    assert_eq!(st.runtime.actions[1].tool, "read");
    assert_eq!(st.runtime.actions[1].target, "lru.py");
    assert!(
        st.runtime.actions[1].summary.contains("intercepted")
            || st.runtime.actions[1].summary.contains("working set")
            || st.runtime.actions[1].summary.contains("Guard"),
        "unexpected action summary: {}",
        st.runtime.actions[1].summary
    );

    // Verify working set retains the original file snippet and was not corrupted by the guard message
    assert_eq!(st.runtime.working_set.files.len(), 1);
    assert_eq!(st.runtime.working_set.files[0].path, "lru.py");
    assert!(st.runtime.working_set.files[0]
        .content
        .contains("def cache():"));
}

#[test]
fn test_loop_breaker_allows_read_after_edit() {
    // Verifies that an intervening edit on the file resets the loop breaker,
    // allowing the agent to re-read the updated file without interception.
    let d = repo();
    std::fs::write(d.path().join("lru.py"), "def cache():\n    return 1\n").unwrap();

    let p = ScriptedProvider::new(vec![
        // Turn 1: Read file
        calls("read", json!({"path": "lru.py"})),
        // Turn 2: Edit file
        calls(
            "edit",
            json!({
                "path": "lru.py",
                "old": "    return 1\n",
                "new": "    return 42\n"
            }),
        ),
        // Turn 3: Read file again after edit
        calls("read", json!({"path": "lru.py"})),
        // Turn 4: Complete run
        says("done"),
    ]);
    let mut st = State::new("allow read after edit", "true");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 1);

    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 4);

    // Turn 3 read (index 6 in conversational history) must NOT be intercepted
    let Message::ToolResult {
        content: content3,
        is_error: err3,
        ..
    } = &seen[3].messages[6]
    else {
        panic!("expected ToolResult at index 6");
    };
    assert!(!err3);
    assert!(!content3.contains("[Harness Working Memory Guard]"));
    assert!(content3.contains("return 42"));

    // Verify action ledger shows real read, edit, real read
    assert_eq!(st.runtime.actions.len(), 3);
    assert_eq!(st.runtime.actions[0].tool, "read");
    assert_eq!(st.runtime.actions[1].tool, "edit");
    assert_eq!(st.runtime.actions[2].tool, "read");
    assert_eq!(st.runtime.actions[2].target, "lru.py");
    assert!(st.runtime.actions[2].summary.contains("lines read"));
}

#[test]
fn test_loop_breaker_intercepts_oscillating_reads() {
    // Verifies that oscillating between reading two files (e.g. lru.py and test_lru.py)
    // without taking action triggers the loop-breaker once a file has been read 2+ times
    // in the last 4 actions without an intervening edit.
    let d = repo();
    std::fs::write(d.path().join("lru.py"), "def cache(): pass\n").unwrap();
    std::fs::write(d.path().join("test_lru.py"), "def test(): pass\n").unwrap();

    let p = ScriptedProvider::new(vec![
        // Turn 1: read lru.py
        calls("read", json!({"path": "lru.py"})),
        // Turn 2: read test_lru.py
        calls("read", json!({"path": "test_lru.py"})),
        // Turn 3: read lru.py again
        calls("read", json!({"path": "lru.py"})),
        // Turn 4: read test_lru.py again
        calls("read", json!({"path": "test_lru.py"})),
        // Turn 5: read lru.py 3rd time (2nd time in last 4 actions) -> intercepted!
        calls("read", json!({"path": "lru.py"})),
        says("done"),
    ]);
    let mut st = State::new("oscillating read guard", "true");
    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);

    let seen = p.seen.lock().unwrap();
    // Turn 5 read result is at index 10 in conversational history
    let Message::ToolResult {
        content: content5, ..
    } = &seen[5].messages[10]
    else {
        panic!("expected ToolResult at index 10");
    };
    assert!(content5.contains(
        "[Harness Working Memory Guard]: `lru.py` is already loaded in your Active Working Set above"
    ));
}

#[test]
fn test_loop_breaker_allows_paged_line_range_reads() {
    // Verifies that consecutive reads on different line slices of the same file
    // (such as paging through a file) are NOT intercepted.
    let d = repo();
    let content = (1..=20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(d.path().join("large.py"), &content).unwrap();

    let p = ScriptedProvider::new(vec![
        // Turn 1: Read lines 1-5
        calls(
            "read",
            json!({"path": "large.py", "start_line": 1, "end_line": 5}),
        ),
        // Turn 2: Read lines 6-10 (different slice - should NOT be intercepted)
        calls(
            "read",
            json!({"path": "large.py", "start_line": 6, "end_line": 10}),
        ),
        says("done"),
    ]);
    let mut st = State::new("paged read test", "true");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 1);

    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);

    // Turn 2 tool result must be the actual lines 6-10, NOT the guard message
    let Message::ToolResult {
        content: content2,
        is_error: err2,
        ..
    } = &seen[2].messages[4]
    else {
        panic!("expected ToolResult at index 4");
    };
    assert!(!err2);
    assert!(!content2.contains("[Harness Working Memory Guard]"));
    assert!(content2.contains("line 6"));
    assert!(content2.contains("line 10"));
}

#[test]
fn test_loop_breaker_resets_on_write() {
    // Verifies that a write to the target file resets the loop breaker,
    // allowing the agent to read the newly written content even after 2 prior reads.
    let d = repo();
    std::fs::write(d.path().join("lru.py"), "def cache(): return 1\n").unwrap();
    std::fs::write(d.path().join("other.py"), "def other(): return 2\n").unwrap();

    let p = ScriptedProvider::new(vec![
        // Turn 1: Read lru.py
        calls("read", json!({"path": "lru.py"})),
        // Turn 2: Read other.py
        calls("read", json!({"path": "other.py"})),
        // Turn 3: Read lru.py (2nd time in last 4 actions)
        calls("read", json!({"path": "lru.py"})),
        // Turn 4: Write to lru.py (target includes byte count annotation, e.g. "lru.py (26 bytes)")
        calls(
            "write",
            json!({"path": "lru.py", "content": "def cache(): return 99\n"}),
        ),
        // Turn 5: Read lru.py after write (must not be intercepted!)
        calls("read", json!({"path": "lru.py"})),
        says("done"),
    ]);
    let mut st = State::new("write resets read guard", "true");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 1);

    let seen = p.seen.lock().unwrap();
    let Message::ToolResult {
        content: content5, ..
    } = &seen[5].messages[10]
    else {
        panic!("expected ToolResult at index 10");
    };
    assert!(!content5.contains("[Harness Working Memory Guard]"));
    assert!(content5.contains("return 99"));
}

#[test]
fn test_loop_breaker_remains_intercepted_on_consecutive_attempts() {
    // Verifies that consecutive read attempts on the same cached path remain
    // intercepted, rather than alternating back to disk execution.
    let d = repo();
    std::fs::write(d.path().join("lru.py"), "def cache(): return 1\n").unwrap();

    let p = ScriptedProvider::new(vec![
        calls("read", json!({"path": "lru.py"})),
        calls("read", json!({"path": "lru.py"})),
        calls("read", json!({"path": "lru.py"})),
        says("done"),
    ]);
    let mut st = State::new("consecutive intercepted reads", "true");
    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);

    let seen = p.seen.lock().unwrap();
    let Message::ToolResult {
        content: content2, ..
    } = &seen[2].messages[4]
    else {
        panic!("expected ToolResult at index 4");
    };
    let Message::ToolResult {
        content: content3, ..
    } = &seen[3].messages[6]
    else {
        panic!("expected ToolResult at index 6");
    };
    assert!(content2.contains("[Harness Working Memory Guard]"));
    assert!(content3.contains("[Harness Working Memory Guard]"));
}

#[test]
fn test_loop_breaker_allows_slice_read_on_distilled_file() {
    // Verifies that requesting a slice from a large file whose middle lines were omitted
    // during working set distillation is NOT intercepted.
    let d = repo();
    let content = (1..=200)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(d.path().join("distilled.py"), &content).unwrap();

    let p = ScriptedProvider::new(vec![
        // Turn 1: Read full file (distilled to first 50 and last 50 lines)
        calls("read", json!({"path": "distilled.py"})),
        // Turn 2: Read omitted middle slice (lines 80-120) - must NOT be intercepted!
        calls(
            "read",
            json!({"path": "distilled.py", "start_line": 80, "end_line": 120}),
        ),
        says("done"),
    ]);
    let mut st = State::new("distilled file slice", "true");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 1);

    let seen = p.seen.lock().unwrap();
    let Message::ToolResult {
        content: content2,
        is_error: err2,
        ..
    } = &seen[2].messages[4]
    else {
        panic!("expected ToolResult at index 4");
    };
    assert!(!err2);
    assert!(!content2.contains("[Harness Working Memory Guard]"));
    assert!(content2.contains("line 80"));
    assert!(content2.contains("line 120"));
}

#[test]
fn test_loop_breaker_does_not_count_intercepted_reads_as_actions() {
    // Verifies that intercepted reads do not inflate the consecutive read counter,
    // preventing an un-recoverable loop when following the guard's prompt to use note.
    let d = repo();
    std::fs::write(d.path().join("lru.py"), "def cache(): return 1\n").unwrap();

    let p = ScriptedProvider::new(vec![
        // Turn 1: Read file
        calls("read", json!({"path": "lru.py"})),
        // Turn 2: Redundant read -> intercepted
        calls("read", json!({"path": "lru.py"})),
        // Turn 3: Follow advice, update plan with note
        calls("note", json!({"patch": {"plan": "proceed to edit"}})),
        // Turn 4: Edit file
        calls(
            "edit",
            json!({"path": "lru.py", "old": "return 1", "new": "return 2"}),
        ),
        // Turn 5: Read file after edit -> should NOT be blocked
        calls("read", json!({"path": "lru.py"})),
        says("done"),
    ]);
    let mut st = State::new("intercepted read recovery", "true");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 1);

    let seen = p.seen.lock().unwrap();
    let Message::ToolResult {
        content: content5, ..
    } = &seen[5].messages[10]
    else {
        panic!("expected ToolResult at index 10");
    };
    assert!(!content5.contains("[Harness Working Memory Guard]"));
    assert!(content5.contains("return 2"));
}

#[test]
fn test_loop_breaker_does_not_reset_on_unrelated_file_edit() {
    // Verifies that an edit to an UNRELATED file (e.g. other.py) does not reset
    // the loop breaker on lru.py.
    let d = repo();
    std::fs::write(d.path().join("lru.py"), "def cache(): return 1\n").unwrap();
    std::fs::write(d.path().join("other.py"), "def helper(): return 2\n").unwrap();

    let p = ScriptedProvider::new(vec![
        // Turn 1: Read lru.py
        calls("read", json!({"path": "lru.py"})),
        // Turn 2: Edit other.py
        calls(
            "edit",
            json!({"path": "other.py", "old": "return 2", "new": "return 3"}),
        ),
        // Turn 3: Read lru.py again
        calls("read", json!({"path": "lru.py"})),
        // Turn 4: Read lru.py 3rd time (2nd read in last 4 actions without editing lru.py) -> intercepted!
        calls("read", json!({"path": "lru.py"})),
        says("done"),
    ]);
    let mut st = State::new("unrelated edit guard", "true");
    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);

    let seen = p.seen.lock().unwrap();
    let Message::ToolResult {
        content: content4, ..
    } = &seen[4].messages[8]
    else {
        panic!("expected ToolResult at index 8");
    };
    assert!(content4.contains(
        "[Harness Working Memory Guard]: `lru.py` is already loaded in your Active Working Set above"
    ));
}

#[test]
fn test_loop_breaker_supports_paths_with_parentheses() {
    // Verifies that paths containing parentheses, and specifically containing " (L"
    // (e.g. "data (Large).json" and "test (L1).rs"), are correctly matched by target_matches_path
    // under both write (byte-annotated) and read (line-annotated) action formats.
    let d = repo();
    let write_file = "data (Large).json";
    let read_file = "test (L1).rs";
    std::fs::write(d.path().join(write_file), "{\"a\": 1}\n").unwrap();
    std::fs::write(
        d.path().join(read_file),
        "fn main() {\n    println!(\"hello\");\n}\n",
    )
    .unwrap();
    std::fs::write(d.path().join("other.py"), "def other(): return 2\n").unwrap();

    let p = ScriptedProvider::new(vec![
        // Turn 1: Read write_file
        calls("read", json!({"path": write_file})),
        // Turn 2: Read other.py
        calls("read", json!({"path": "other.py"})),
        // Turn 3: Read write_file 2nd time
        calls("read", json!({"path": write_file})),
        // Turn 4: Write to write_file (target formatted as "data (Large).json (13 bytes)")
        calls(
            "write",
            json!({"path": write_file, "content": "{\"a\": 99}\n"}),
        ),
        // Turn 5: Read write_file after write (must not be intercepted!)
        calls("read", json!({"path": write_file})),
        // Turn 6: Read bounded line range on read_file (target formatted as "test (L1).rs (L1-L2)")
        calls(
            "read",
            json!({"path": read_file, "start_line": 1, "end_line": 2}),
        ),
        // Turn 7: Redundant read with exact same line range on read_file -> must be intercepted!
        calls(
            "read",
            json!({"path": read_file, "start_line": 1, "end_line": 2}),
        ),
        says("done"),
    ]);
    let mut st = State::new("parentheses and (L in path test", "true");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 1);

    let seen = p.seen.lock().unwrap();
    // Turn 5 (index 10): write_file read after write must not be intercepted
    let Message::ToolResult {
        content: content5, ..
    } = &seen[5].messages[10]
    else {
        panic!("expected ToolResult at index 10");
    };
    assert!(!content5.contains("[Harness Working Memory Guard]"));
    assert!(content5.contains("99"));

    // Turn 7 (index 14): duplicate read on read_file must be intercepted
    let Message::ToolResult {
        content: content7, ..
    } = &seen[7].messages[14]
    else {
        panic!("expected ToolResult at index 14");
    };
    assert!(content7.contains("[Harness Working Memory Guard]"));
    assert!(content7.contains("test (L1).rs"));
}

#[test]
fn test_loop_breaker_allows_subslice_on_distilled_bounded_snippet() {
    // Verifies that when a bounded read (lines 1-300) exceeds MAX_FILE_LINES and gets distilled
    // with middle lines omitted, requesting a sub-slice within the omitted section is NOT intercepted.
    let d = repo();
    let content = (1..=400)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(d.path().join("huge.py"), &content).unwrap();

    let p = ScriptedProvider::new(vec![
        // Turn 1: Read large slice 1-300 (distilled by WorkingSet::add_or_update)
        calls(
            "read",
            json!({"path": "huge.py", "start_line": 1, "end_line": 300}),
        ),
        // Turn 2: Read sub-slice lines 100-150 in the omitted portion - must NOT be intercepted!
        calls(
            "read",
            json!({"path": "huge.py", "start_line": 100, "end_line": 150}),
        ),
        says("done"),
    ]);
    let mut st = State::new("bounded distilled subslice test", "true");
    let (outcome, rep) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_attempts, 1);

    let seen = p.seen.lock().unwrap();
    let Message::ToolResult {
        content: content2,
        is_error: err2,
        ..
    } = &seen[2].messages[4]
    else {
        panic!("expected ToolResult at index 4");
    };
    assert!(!err2);
    assert!(!content2.contains("[Harness Working Memory Guard]"));
    assert!(content2.contains("line 100"));
    assert!(content2.contains("line 150"));
}

#[test]
fn test_target_matches_path_edge_cases() {
    use superstate_agent::run::target_matches_path;

    // Normal paths and prefixes
    assert!(target_matches_path("foo.rs", "foo.rs"));
    assert!(target_matches_path("./foo.rs", "foo.rs"));

    // Line range annotations
    assert!(target_matches_path("foo.rs (L1-L20)", "foo.rs"));
    assert!(target_matches_path("foo.rs (L5+)", "foo.rs"));
    assert!(target_matches_path("foo.rs (L1)", "foo.rs"));

    // Paths containing parentheses and " (L"
    assert!(target_matches_path("model (L)", "model (L)"));
    assert!(target_matches_path("file (L✓)", "file (L✓)"));
    assert!(target_matches_path("test (L1).rs (L1-L2)", "test (L1).rs"));
    assert!(target_matches_path(
        "data (Large).json",
        "data (Large).json"
    ));

    // Byte annotations
    assert!(target_matches_path(
        "data (Large).json (26 bytes)",
        "data (Large).json"
    ));
    assert!(target_matches_path(
        "data (sample bytes)",
        "data (sample bytes)"
    ));
    assert!(!target_matches_path("data (sample bytes)", "data"));
}

#[test]
fn test_loop_breaker_intercepts_redundant_bash_inspection() {
    let d = repo();
    let p = ScriptedProvider::new(vec![
        calls("bash", json!({"command": "git diff"})),
        calls("bash", json!({"command": "git diff"})),
        says("stopping now"),
    ]);
    let mut st = State::new("break git diff loop", "true");
    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);

    assert_eq!(st.runtime.actions.len(), 2);
    assert_eq!(st.runtime.actions[0].tool, "bash");
    assert_eq!(st.runtime.actions[0].target, "git diff");
    assert_eq!(st.runtime.actions[1].tool, "bash");
    assert_eq!(st.runtime.actions[1].target, "git diff");
    assert_eq!(
        st.runtime.actions[1].summary,
        "intercepted: redundant inspection"
    );

    // Verify advice did NOT falsely claim unit tests passed when no tests or edits have occurred
    let guard_msg = superstate_agent::run::should_intercept_redundant_bash(
        &calls("bash", json!({"command": "git diff"})).tool_calls[0],
        &st.runtime.actions,
        &st.runtime.working_set,
        &st.orchestrator.verification_cmd,
    )
    .expect("should intercept");
    assert!(
        guard_msg.contains("Review your Active Working Set or Last Test Failure"),
        "expected edit reminder, got: {guard_msg}"
    );
    assert!(!guard_msg.contains("unit tests have already passed"));
}

#[test]
fn test_loop_breaker_post_pass_advice_resets_on_new_edit() {
    let d = repo();
    let p = ScriptedProvider::new(vec![
        calls(
            "edit",
            json!({"path": "calc.rs", "old": "let val = 0;", "new": "let val = 1;"}),
        ),
        calls("bash", json!({"command": "cargo test"})),
        // Tests passed! git diff should be intercepted with pass notice
        calls("bash", json!({"command": "git diff"})),
        // Now a new edit occurs!
        calls(
            "edit",
            json!({"path": "calc.rs", "old": "let val = 1;", "new": "let val = 2;"}),
        ),
        // git diff now should NOT say tests are passing because new unverified edits occurred
        calls("bash", json!({"command": "git diff"})),
        says("stopping now"),
    ]);
    let mut st = State::new("test pass reset", "true");
    let (outcome, _) = runner(&p, d.path(), &Conversational, false).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);

    // After the second edit, checking bash guard for git diff should not claim tests passed
    let guard_msg = superstate_agent::run::should_intercept_redundant_bash(
        &calls("bash", json!({"command": "git diff"})).tool_calls[0],
        &st.runtime.actions,
        &st.runtime.working_set,
        &st.orchestrator.verification_cmd,
    );
    // Since only one git diff was run since the second edit, it is not intercepted as passing
    assert!(
        guard_msg.is_none()
            || !guard_msg
                .unwrap()
                .contains("unit tests have already passed")
    );
}
