# Native Harness Autonomous TDD Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** Let the native harness gate itself by autonomous TDD (RED then GREEN) so benchmark runs are no longer ungated.

**Spec:** `docs/superpowers/specs/2026-09-23-native-harness-autonomous-tdd-design.md`

**Architecture:**
- `superstate-agent` gains a harness-owned `Phase` and `Runner::run_phases`, which runs phases under one deadline and commits a checkpoint between them.
- The mechanical RED-scope and GREEN-tamper checks are appended to each phase's verification command by `Phase::gate(start)`, so the turn loop itself is unchanged.
- `superstate-cli` converts `superstate_core::tdd::tdd_milestones` into phases behind `--tdd`, and the report gains `gate_source` and `phases`.

**Tech Stack:** Rust 2021, `clap`, `serde`, `tempfile`.

## Global Constraints

- Base branch: `feat/native-harness-autonomous-tdd`. Each task runs in its own worktree `.wt/<task>` on branch `wip/<task>`, created from the base branch. Claude merges reviewed work back.
- Task 2 requires Task 1 merged.
- Zero warnings: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass before every commit.
- Conventional commits with `--no-gpg-sign`, using the exact messages below.
- Existing tests must pass unmodified, except where a step says to add a struct field to a literal.

---

### Task 1: `Phase` and `Runner::run_phases` (`superstate-agent`)

**Files:**
- Modify: `crates/superstate-agent/src/run.rs`
- Test: `crates/superstate-agent/tests/test_phases.rs` (create)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_phases.rs`:

```rust
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
        &[phase("first", "test -f a.txt"), phase("second", "test -f b.txt")],
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
        log.lines().next().unwrap().contains("checkpoint phase first"),
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
    let (outcome, _, phases) = runner(&p, d.path(), budget(1)).run_phases(
        &mut st,
        &[phase("first", "false"), phase("second", "true")],
    );
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
    assert!(!d.path().join("junk.txt").exists(), "phase two's work is rolled back");
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
    assert!(g.contains("git diff --quiet abc123 -- . ':(exclude)t.py'"), "{g}");
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
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_phases`. Expected: compile errors, since `Phase` and `run_phases` do not exist.

- [ ] **Step 3: Add `Phase` and `PhaseResult`.** In `crates/superstate-agent/src/run.rs`, directly above `pub struct Runner<'a> {`, insert:

```rust
/// One stage of a multi-phase run, with its own instruction and verification command.
///
/// Phases belong to the runtime, not the worker. The harness derives them (for example from the
/// repository, by autonomous TDD), and the worker can neither see nor edit the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase {
    pub id: String,
    pub instruction: String,
    pub verify_cmd: String,
    /// The only tracked path allowed to differ from the phase's starting commit when its gate
    /// passes. RED uses it: a worker that fixes the bug while "writing a failing test" makes
    /// the RED proof meaningless.
    pub scope_only: Option<String>,
    /// A path that must be unchanged from the phase's starting commit when its gate passes.
    /// GREEN uses it: the test that defines done may not be weakened after it was seen to fail.
    pub protect: Option<String>,
}

impl Phase {
    /// The command the gate actually runs: the phase's own command plus its mechanical checks,
    /// measured against `start`, the commit captured when the phase began. A worker that commits
    /// mid-phase cannot move that reference point.
    pub fn gate(&self, start: &str) -> String {
        let mut cmd = format!("( {} )", self.verify_cmd);
        if let Some(p) = &self.scope_only {
            cmd.push_str(&format!(
                " && {{ git diff --quiet {start} -- . ':(exclude){p}' || \
                 {{ echo 'superstate: in this phase only {p} may change; revert your other edits'; false; }}; }}"
            ));
        }
        if let Some(p) = &self.protect {
            cmd.push_str(&format!(
                " && {{ git diff --quiet {start} -- '{p}' || \
                 {{ echo 'superstate: {p} defines done and may not change in this phase'; false; }}; }}"
            ));
        }
        cmd
    }
}

/// How one phase of a multi-phase run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseResult {
    pub id: String,
    pub outcome: Outcome,
    pub turns: u32,
    pub gate_attempts: u32,
}
```

- [ ] **Step 4: Run the loop against an absolute deadline.** In `impl<'a> Runner<'a>`:
  1. Rename `pub fn run(&self, state: &mut State) -> (Outcome, RunReport) {` to `fn run_until(&self, state: &mut State, deadline: Option<std::time::Instant>) -> (Outcome, RunReport) {`.
  2. In that function, delete the line `let started = std::time::Instant::now();`.
  3. Replace

```rust
            if let Some(limit) = self.budget.max_duration {
                if started.elapsed() >= limit {
                    return (Outcome::BudgetExhausted, report);
                }
            }
```

     with

```rust
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                return (Outcome::BudgetExhausted, report);
            }
```

  4. Directly above `fn run_until`, insert the public entry points:

```rust
    pub fn run(&self, state: &mut State) -> (Outcome, RunReport) {
        let deadline = self
            .budget
            .max_duration
            .map(|d| std::time::Instant::now() + d);
        self.run_until(state, deadline)
    }

    /// Run `phases` in order under one deadline. After each verified phase except the last, a
    /// checkpoint is committed, so the next phase's rollback baseline keeps it: a GREEN rollback
    /// must never delete RED's verified test. Turn and gate budgets apply per phase, matching
    /// the daemon's per-milestone retry budget. The deadline covers the whole run.
    pub fn run_phases(
        &self,
        state: &mut State,
        phases: &[Phase],
    ) -> (Outcome, RunReport, Vec<PhaseResult>) {
        let deadline = self
            .budget
            .max_duration
            .map(|d| std::time::Instant::now() + d);
        let task = state.orchestrator.goal.clone();
        let mut total = RunReport::default();
        let mut results = Vec::new();
        for (i, phase) in phases.iter().enumerate() {
            let Some(start) = self.checkpoint() else {
                return (
                    Outcome::Error("phases need a git repository: cannot read HEAD".into()),
                    total,
                    results,
                );
            };
            state.orchestrator.goal = format!(
                "{task}\n\n## Current milestone ({}/{}): {}\n{}",
                i + 1,
                phases.len(),
                phase.id,
                phase.instruction
            );
            state.orchestrator.verification_cmd = phase.gate(&start);
            let (outcome, report) = self.run_until(state, deadline);
            total.outcome_turns += report.outcome_turns;
            total.usage.input += report.usage.input;
            total.usage.cached += report.usage.cached;
            total.usage.output += report.usage.output;
            total.usage.thinking += report.usage.thinking;
            total.empty_turns += report.empty_turns;
            total.tool_calls += report.tool_calls;
            total.gate_attempts += report.gate_attempts;
            results.push(PhaseResult {
                id: phase.id.clone(),
                outcome: outcome.clone(),
                turns: report.outcome_turns,
                gate_attempts: report.gate_attempts,
            });
            if outcome != Outcome::Verified {
                state.orchestrator.goal = task;
                return (outcome, total, results);
            }
            if i + 1 < phases.len() {
                if let Err(e) = self.commit_checkpoint(&phase.id) {
                    state.orchestrator.goal = task;
                    return (Outcome::Error(e), total, results);
                }
            }
        }
        state.orchestrator.goal = task;
        (Outcome::Verified, total, results)
    }

    /// Commit the tree as the verified end of a phase. The identity is set inline because a
    /// task container often has no git user configured.
    fn commit_checkpoint(&self, phase_id: &str) -> Result<(), String> {
        let cmd = format!(
            "git add -A && git -c user.name=superstate -c user.email=superstate@localhost \
             commit --no-gpg-sign -q --allow-empty -m 'chore(superstate): checkpoint phase {phase_id}'"
        );
        match self.shell.exec(&cmd, &self.root, Duration::from_secs(120)) {
            Ok(o) if o.exit_code == 0 => Ok(()),
            Ok(o) => Err(format!(
                "checkpoint commit for phase {phase_id} failed: {}",
                o.merged().trim()
            )),
            Err(e) => Err(format!(
                "checkpoint commit for phase {phase_id} could not run: {e}"
            )),
        }
    }
```

- [ ] **Step 5: Run the new tests.** Run: `cargo test -p superstate-agent --test test_phases`. Expected: 10 passed.

- [ ] **Step 6: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. (`superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` is known to be timing-flaky. If it alone fails, re-run once and report both results.)

- [ ] **Step 7: Commit.**

```bash
git add crates/superstate-agent/
git commit --no-gpg-sign -m "feat(agent): harness-owned phases with checkpoints, scope and tamper checks"
```

---

### Task 2: `--tdd`, and `gate_source`/`phases` in the report (`superstate-cli`)

Requires Task 1 merged into the base branch.

**Files:**
- Modify: `crates/superstate-cli/src/cli.rs`, `crates/superstate-cli/src/task_runner.rs`, `crates/superstate-cli/src/report.rs`, `crates/superstate-cli/src/repl.rs` (TaskConfig literal only), `crates/superstate-cli/tests/test_task_runner.rs` and `crates/superstate-cli/tests/test_run_report.rs` (TaskConfig literals only: add `tdd: false`)
- Test: `crates/superstate-cli/tests/test_tdd_phases.rs` (create)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-cli/tests/test_tdd_phases.rs`:

```rust
//! `--tdd`: autonomous TDD as native-harness phases, and how the report records it.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::json;
use superstate_agent::run::{Outcome, PhaseResult};
use superstate_cli::report::RunRecord;
use superstate_cli::task_runner::{tdd_phases, TaskConfig};

fn config() -> TaskConfig {
    TaskConfig {
        prompt: "fix it".into(),
        gate: None,
        context_strategy: "state-only".into(),
        model: Some("gemini-3.8-flash-high".into()),
        container: None,
        max_turns: 1000,
        max_gates: 5,
        rollback: true,
        root: PathBuf::from("."),
        loop_breaker: true,
        container_workdir: "/workspace".into(),
        max_seconds: None,
        report: None,
        tdd: false,
    }
}

#[test]
fn tdd_phases_are_red_then_green_from_superstate_core() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("pyproject.toml"), "").unwrap();
    let profile = superstate_core::tdd::detect_profile(d.path()).unwrap();
    let phases = tdd_phases(d.path()).unwrap();
    assert_eq!(phases.len(), 2);
    assert_eq!(phases[0].id, "tdd-reproduction-test");
    assert_eq!(phases[0].verify_cmd, profile.red_cmd);
    assert_eq!(phases[0].scope_only.as_deref(), Some(profile.reproduction_path));
    assert_eq!(phases[0].protect, None);
    assert_eq!(phases[1].id, "fix-implementation");
    assert_eq!(phases[1].verify_cmd, profile.green_cmd);
    assert_eq!(phases[1].scope_only, None);
    assert_eq!(phases[1].protect.as_deref(), Some(profile.reproduction_path));
}

#[test]
fn an_unrecognised_repository_is_an_error_never_ungated() {
    let d = tempfile::tempdir().unwrap();
    let err = tdd_phases(d.path()).unwrap_err();
    assert!(err.contains("autonomous TDD"), "{err}");
}

#[test]
fn gate_source_names_where_the_gate_came_from() {
    let none = RunRecord::from_error(&config(), "x", Duration::ZERO);
    assert_eq!((none.gate_source.as_str(), none.gated), ("none", false));

    let mut user = config();
    user.gate = Some("pytest".into());
    let user = RunRecord::from_error(&user, "x", Duration::ZERO);
    assert_eq!((user.gate_source.as_str(), user.gated), ("user", true));

    let mut tdd = config();
    tdd.tdd = true;
    let tdd = RunRecord::from_error(&tdd, "x", Duration::ZERO);
    assert_eq!((tdd.gate_source.as_str(), tdd.gated), ("tdd", true));
}

#[test]
fn phase_results_are_recorded_in_order() {
    let r = RunRecord::from_error(&config(), "x", Duration::ZERO).with_phases(&[
        PhaseResult {
            id: "tdd-reproduction-test".into(),
            outcome: Outcome::Verified,
            turns: 3,
            gate_attempts: 1,
        },
        PhaseResult {
            id: "fix-implementation".into(),
            outcome: Outcome::GateFailed,
            turns: 9,
            gate_attempts: 5,
        },
    ]);
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(
        v["phases"],
        json!([
            {"id": "tdd-reproduction-test", "outcome": "verified", "turns": 3, "gate_attempts": 1},
            {"id": "fix-implementation", "outcome": "gate_failed", "turns": 9, "gate_attempts": 5}
        ])
    );
    let plain = serde_json::to_value(RunRecord::from_error(&config(), "x", Duration::ZERO)).unwrap();
    assert_eq!(plain["phases"], json!([]));
}

#[test]
fn tdd_conflicts_with_an_explicit_gate() {
    let out = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["fix it", "--tdd", "--gate", "pytest"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn the_binary_reports_an_undetectable_repository_as_a_tdd_error() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("run.json");
    let out = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .args(["fix it", "--tdd", "--report"])
        .arg(&report)
        .current_dir(dir.path())
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("report must exist"))
            .unwrap();
    assert_eq!(v["outcome"], "error");
    assert_eq!(v["gate_source"], "tdd");
    assert!(v["error"].as_str().unwrap().contains("autonomous TDD"));
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate --test test_tdd_phases`. Expected: compile errors, since `tdd_phases`, `TaskConfig.tdd`, `gate_source`, `with_phases` and `PhaseResult` import sites do not exist yet. (The package name is `superstate`; the library is `superstate_cli`.)

- [ ] **Step 3: Add the flag.** In `crates/superstate-cli/src/cli.rs`, in `struct Cli`, insert directly after the `pub report: Option<PathBuf>,` field:

```rust
    /// Gate by autonomous TDD: derive a RED (failing reproduction test) then GREEN (fix) phase
    /// from the repository's own test runner. Fails closed on an unrecognised repository
    #[arg(long, conflicts_with = "gate")]
    pub tdd: bool,
```

- [ ] **Step 4: Extend `TaskConfig`.** In `crates/superstate-cli/src/task_runner.rs`, add at the end of `struct TaskConfig`:

```rust
    /// Gate by autonomous TDD phases instead of a single command (`--tdd`).
    pub tdd: bool,
```

In `impl From<&crate::cli::Cli> for TaskConfig`, add after `report: cli.report.clone(),`:

```rust
            tdd: cli.tdd,
```

Add `tdd: false,` as the last field of every other `TaskConfig { … }` literal: `src/repl.rs`, the four in `tests/test_task_runner.rs`, and `config()` in `tests/test_run_report.rs`.

- [ ] **Step 5: Convert milestones to phases.** In `task_runner.rs`, add this function directly above `pub fn run_task(`:

```rust
/// Autonomous TDD's RED/GREEN milestones as harness phases. The wording, commands and
/// reproduction path come from `superstate_core::tdd` unchanged, so native and daemon runs are
/// given identical instructions. RED may change only the reproduction test; GREEN may not
/// change it at all.
pub fn tdd_phases(
    repo_root: &std::path::Path,
) -> Result<Vec<superstate_agent::run::Phase>, String> {
    let milestones = superstate_core::tdd::tdd_milestones(repo_root)
        .map_err(|e| format!("autonomous TDD: {e}"))?;
    let repro = milestones
        .first()
        .and_then(|m| m.files.first())
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| "autonomous TDD: no reproduction path".to_string())?;
    Ok(milestones
        .into_iter()
        .enumerate()
        .map(|(i, m)| superstate_agent::run::Phase {
            id: m.id.unwrap_or_else(|| format!("phase-{}", i + 1)),
            instruction: m.title,
            verify_cmd: m.verify_cmd,
            scope_only: (i == 0).then(|| repro.clone()),
            protect: (i > 0).then(|| repro.clone()),
        })
        .collect())
}
```

- [ ] **Step 6: Run phases when asked.** In `run_task_with_state_and_writer`:
  1. Directly after `let started = std::time::Instant::now();`, insert:

```rust
    // Derived before the provider, so an unrecognised repository fails closed without an API call.
    let phases = if config.tdd {
        Some(tdd_phases(&config.root)?)
    } else {
        None
    };
```

  2. Replace

```rust
    let (outcome, report) = runner.run(state);
    if let Some(path) = &config.report {
```

     with

```rust
    let (outcome, report, phase_results) = match &phases {
        Some(p) => runner.run_phases(state, p),
        None => {
            let (o, r) = runner.run(state);
            (o, r, Vec::new())
        }
    };
    if let Some(path) = &config.report {
```

  3. In the same block, change `started.elapsed(),\n        );` of the `from_run` call to end with `.with_phases(&phase_results)`. The statement becomes:

```rust
        let record = crate::report::RunRecord::from_run(
            config,
            &model_id,
            &outcome,
            &report,
            state,
            started.elapsed(),
        )
        .with_phases(&phase_results);
```

- [ ] **Step 7: Report `gate_source` and `phases`.** In `crates/superstate-cli/src/report.rs`:
  1. Change the import `use superstate_agent::run::{Outcome, RunReport};` to `use superstate_agent::run::{Outcome, PhaseResult, RunReport};`.
  2. Add, directly above `#[derive(Debug, Clone, Serialize, PartialEq)]\npub struct RunRecord`:

```rust
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PhaseRecord {
    pub id: String,
    pub outcome: String,
    pub turns: u32,
    pub gate_attempts: u32,
}

/// The report's name for an outcome, and its error message if it carries one.
fn outcome_name(outcome: &Outcome) -> (&'static str, Option<String>) {
    match outcome {
        Outcome::Verified => ("verified", None),
        Outcome::BudgetExhausted => ("budget_exhausted", None),
        Outcome::GateFailed => ("gate_failed", None),
        Outcome::Error(e) => ("error", Some(e.clone())),
    }
}
```

  3. In `struct RunRecord`, insert after `pub gated: bool,`:

```rust
    /// Where the gate came from: `none` (ungated), `user` (`--gate`) or `tdd` (`--tdd`).
    pub gate_source: String,
    /// Per-phase results for a multi-phase run; empty otherwise.
    pub phases: Vec<PhaseRecord>,
```

  4. In `RunRecord::base`, replace the `gated: config … ,` initialiser with:

```rust
            gated: config.tdd
                || config
                    .gate
                    .as_deref()
                    .is_some_and(|g| !g.trim().is_empty()),
            gate_source: if config.tdd {
                "tdd"
            } else if config.gate.as_deref().is_some_and(|g| !g.trim().is_empty()) {
                "user"
            } else {
                "none"
            }
            .into(),
            phases: Vec::new(),
```

  5. In `RunRecord::from_run`, replace the `let (outcome_name, error) = match outcome { … };` block with `let (outcome_name, error) = outcome_name(outcome);`.
  6. Add this method inside `impl RunRecord`, after `from_run`:

```rust
    /// Attach per-phase results, in the order the phases ran.
    pub fn with_phases(mut self, results: &[PhaseResult]) -> Self {
        self.phases = results
            .iter()
            .map(|r| PhaseRecord {
                id: r.id.clone(),
                outcome: outcome_name(&r.outcome).0.into(),
                turns: r.turns,
                gate_attempts: r.gate_attempts,
            })
            .collect();
        self
    }
```

- [ ] **Step 8: Run the new and existing report tests.** Run: `cargo test -p superstate --test test_tdd_phases --test test_run_report`. Expected: 6 and 7 passed.

- [ ] **Step 9: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. (Same known flaky MCP test as Task 1.)

- [ ] **Step 10: Commit.**

```bash
git add -A crates/superstate-cli/
git commit --no-gpg-sign -m "feat(cli): --tdd gates the native harness by autonomous TDD phases"
```
