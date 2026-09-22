//! The loop, driven by a scripted provider so the behaviour is deterministic.

use std::sync::Mutex;

use serde_json::json;
use superstate_agent::context::{ContextStrategy, Conversational, StateOnly};
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{Budget, Outcome, Runner};
use superstate_agent::shell::{LocalShell, Shell};
use superstate_agent::state::State;
use superstate_provider::{Provider, ProviderError, Request, Response, ToolCall, Usage};

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
