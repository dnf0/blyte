//! Multi-phase runs: the harness-owned sequence that autonomous TDD is built on.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use superstate_agent::context::StateOnly;
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{Budget, Outcome, Phase, Runner};
use superstate_agent::shell::{LocalShell, Shell};
use superstate_agent::state::State;
use superstate_provider::{Provider, ProviderError, Request, Response, ToolCall};

struct ScriptedProvider {
    script: Mutex<Vec<Response>>,
}

impl ScriptedProvider {
    fn new(script: Vec<Response>) -> Self {
        Self {
            script: Mutex::new(script),
        }
    }
}

impl Provider for ScriptedProvider {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let mut s = self.script.lock().unwrap();
        if s.is_empty() {
            return Ok(Response::default());
        }
        Ok(s.remove(0))
    }

    fn model_id(&self) -> String {
        "scripted-phases".into()
    }
}

fn done() -> Response {
    Response {
        text: "done".into(),
        ..Default::default()
    }
}

fn write(path: &str, content: &str) -> Response {
    Response {
        tool_calls: vec![ToolCall {
            id: format!("write_{path}"),
            name: "write".into(),
            arguments: json!({"path": path, "content": content}),
            signature: None,
        }],
        ..Default::default()
    }
}

fn sh(cmd: &str, dir: &Path) -> String {
    let out = LocalShell.exec(cmd, dir, Duration::from_secs(30)).unwrap();
    assert_eq!(out.exit_code, 0, "{cmd}: {}", out.merged());
    out.stdout
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    sh(
        "git init -q && git config user.email a@b && git config user.name c",
        d.path(),
    );
    std::fs::write(d.path().join("src.py"), "def f():\n    return 1\n").unwrap();
    sh("git add -A && git commit --no-gpg-sign -qm base", d.path());
    d
}

fn phase(id: &str, verify: &str) -> Phase {
    Phase {
        id: id.into(),
        instruction: format!("do {id}"),
        verify_cmd: verify.into(),
        scope_only: None,
        protect: None,
    }
}

fn budget(gates: u32) -> Budget {
    Budget {
        max_turns: 10,
        max_gate_attempts: gates,
        max_duration: None,
    }
}

fn runner<'a>(p: &'a ScriptedProvider, root: &Path, budget: Budget) -> Runner<'a> {
    Runner {
        provider: p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &StateOnly,
        root: root.to_path_buf(),
        budget,
        rollback: true,
        loop_breaker: true,
        on_event: None,
    }
}

#[test]
fn phases_run_in_order_with_a_checkpoint_between_them() {
    let d = repo();
    let p = ScriptedProvider::new(vec![
        write("a.txt", "a\n"),
        done(),
        write("b.txt", "b\n"),
        done(),
    ]);
    let mut st = State::new("two steps", "");
    let (outcome, report, phases) = runner(&p, d.path(), budget(2)).run_phases(
        &mut st,
        &[
            phase("first", "test -f a.txt"),
            phase("second", "test -f b.txt"),
        ],
    );
    assert_eq!(outcome, Outcome::Verified);
    let summary: Vec<(&str, Outcome)> = phases
        .iter()
        .map(|r| (r.id.as_str(), r.outcome.clone()))
        .collect();
    assert_eq!(
        summary,
        vec![("first", Outcome::Verified), ("second", Outcome::Verified)]
    );
    assert_eq!(report.gate_attempts, 2);
    assert_eq!(report.outcome_turns, 4);
    let log = sh("git log --format=%s", d.path());
    assert!(
        log.lines()
            .next()
            .unwrap()
            .contains("checkpoint phase first"),
        "{log}"
    );
    assert!(sh("git ls-files", d.path()).contains("a.txt"));
    assert_eq!(st.orchestrator.goal, "two steps");
}

#[test]
fn a_failed_phase_ends_the_run_and_later_phases_never_start() {
    let d = repo();
    let p = ScriptedProvider::new(vec![done()]);
    let mut st = State::new("stops early", "");
    let (outcome, _, phases) = runner(&p, d.path(), budget(1))
        .run_phases(&mut st, &[phase("first", "false"), phase("second", "true")]);
    assert_eq!(outcome, Outcome::GateFailed);
    assert_eq!(phases.len(), 1);
    assert_eq!(phases[0].outcome, Outcome::GateFailed);
}

#[test]
fn a_rollback_in_a_later_phase_keeps_the_earlier_phase_verified_work() {
    let d = repo();
    let p = ScriptedProvider::new(vec![
        write("a.txt", "a\n"),
        done(),
        write("junk.txt", "x\n"),
        done(),
        done(),
    ]);
    let mut st = State::new("keep phase one", "");
    let (outcome, _, _) = runner(&p, d.path(), budget(2)).run_phases(
        &mut st,
        &[
            phase("first", "test -f a.txt"),
            phase("second", "test -f never.txt"),
        ],
    );
    assert_eq!(outcome, Outcome::GateFailed);
    assert!(d.path().join("a.txt").exists(), "phase one's work survives");
    assert!(
        !d.path().join("junk.txt").exists(),
        "phase two's work is rolled back"
    );
}

#[test]
fn scope_only_rejects_a_change_to_any_other_tracked_file() {
    let d = repo();
    let p = ScriptedProvider::new(vec![
        write("src.py", "def f():\n    return 2\n"),
        write("t.txt", "t\n"),
        done(),
    ]);
    let mut st = State::new("red", "");
    let red = Phase {
        scope_only: Some("t.txt".into()),
        ..phase("red", "test -f t.txt")
    };
    let (outcome, _, _) = runner(&p, d.path(), budget(1)).run_phases(&mut st, &[red]);
    assert_eq!(outcome, Outcome::GateFailed);
    assert!(st
        .runtime
        .failures
        .last()
        .unwrap()
        .contains("only t.txt may change"));
}

#[test]
fn scope_only_allows_the_named_path() {
    let d = repo();
    let p = ScriptedProvider::new(vec![write("t.txt", "t\n"), done()]);
    let mut st = State::new("red", "");
    let red = Phase {
        scope_only: Some("t.txt".into()),
        ..phase("red", "test -f t.txt")
    };
    let (outcome, _, _) = runner(&p, d.path(), budget(1)).run_phases(&mut st, &[red]);
    assert_eq!(outcome, Outcome::Verified);
}

#[test]
fn protect_rejects_changing_the_file_that_defines_done() {
    let d = repo();
    let p = ScriptedProvider::new(vec![
        write("t.txt", "v1\n"),
        done(),
        write("t.txt", "v2\n"),
        done(),
    ]);
    let mut st = State::new("green", "");
    let green = Phase {
        protect: Some("t.txt".into()),
        ..phase("green", "true")
    };
    let (outcome, _, phases) = runner(&p, d.path(), budget(1))
        .run_phases(&mut st, &[phase("red", "test -f t.txt"), green]);
    assert_eq!(outcome, Outcome::GateFailed);
    assert_eq!(phases[1].outcome, Outcome::GateFailed);
    assert!(st
        .runtime
        .failures
        .last()
        .unwrap()
        .contains("t.txt defines done"));
}

#[test]
fn protect_passes_when_the_file_is_untouched() {
    let d = repo();
    let p = ScriptedProvider::new(vec![write("t.txt", "v1\n"), done(), done()]);
    let mut st = State::new("green", "");
    let green = Phase {
        protect: Some("t.txt".into()),
        ..phase("green", "true")
    };
    let (outcome, _, _) = runner(&p, d.path(), budget(1))
        .run_phases(&mut st, &[phase("red", "test -f t.txt"), green]);
    assert_eq!(outcome, Outcome::Verified);
}

#[test]
fn gate_appends_the_mechanical_checks_against_the_phase_start() {
    let both = Phase {
        scope_only: Some("t.py".into()),
        protect: Some("t.py".into()),
        ..phase("x", "pytest")
    };
    let g = both.gate("abc123");
    assert!(g.starts_with("( pytest )"), "{g}");
    assert!(
        g.contains("git diff --quiet abc123 -- . ':(exclude)t.py'"),
        "{g}"
    );
    assert!(g.contains("git diff --quiet abc123 -- 't.py'"), "{g}");
    assert_eq!(phase("y", "make test").gate("abc123"), "( make test )");
}

#[test]
fn the_deadline_covers_every_phase() {
    let d = repo();
    let p = ScriptedProvider::new(vec![]);
    let mut st = State::new("no time", "");
    let b = Budget {
        max_turns: 10,
        max_gate_attempts: 1,
        max_duration: Some(Duration::ZERO),
    };
    let (outcome, _, phases) = runner(&p, d.path(), b)
        .run_phases(&mut st, &[phase("first", "true"), phase("second", "true")]);
    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert_eq!(phases.len(), 1);
    assert_eq!(phases[0].turns, 0);
}

#[test]
fn phases_need_a_git_repository() {
    let d = tempfile::tempdir().unwrap();
    let p = ScriptedProvider::new(vec![]);
    let mut st = State::new("no repo", "");
    let (outcome, _, phases) =
        runner(&p, d.path(), budget(1)).run_phases(&mut st, &[phase("first", "true")]);
    match outcome {
        Outcome::Error(e) => assert!(e.contains("git"), "{e}"),
        other => panic!("expected an error, got {other:?}"),
    }
    assert!(phases.is_empty());
}

#[test]
fn each_phase_start_is_marked_in_the_action_ledger() {
    let d = repo();
    let p = ScriptedProvider::new(vec![
        write("a.txt", "a\n"),
        done(),
        write("b.txt", "b\n"),
        done(),
    ]);
    let mut st = State::new("two steps", "");
    let (outcome, _, _) = runner(&p, d.path(), budget(2)).run_phases(
        &mut st,
        &[
            phase("first", "test -f a.txt"),
            phase("second", "test -f b.txt"),
        ],
    );
    assert_eq!(outcome, Outcome::Verified);
    let markers: Vec<&str> = st
        .runtime
        .actions
        .iter()
        .filter(|a| a.tool == superstate_agent::run::PHASE_MARKER_TOOL)
        .map(|a| a.target.as_str())
        .collect();
    assert_eq!(markers, vec!["first", "second"]);
}
