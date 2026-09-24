# Folded Default and Completeness Review Gate Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** (1) Make `folded` the native default context. (2) Before a passing verification is accepted, run an independent **completeness review** of the change against the task, which can send the worker back.

**Why (superstate-eval FINDINGS §13.36–13.38):**
- **`folded` is the best measured context.** It ties `conversational` (4/4 passes on `build-cython-ext`, and the same results on the gated canary) while staying bounded on long tasks. The CLI still defaults to `conversational` and the REPL to `state-only`, the worst context measured.
- **The gate certified unfinished work.** On the gated canary, all four superstate runs passed RED and GREEN on the **first** attempt with zero rollbacks, yet the grader found the work incomplete in three: 92/94 and 28/31 target tests. The worker's own reproduction test was narrower than the task, so the harness stopped early, and on katex agy, which kept working, passed. The external gate removes self-*declared* completion but not self-*defined* completion. The missing behaviours were named in the task text, so a reviewer comparing the diff to the task has a real chance of catching them.

**Design of the review gate:**
- `Orchestrator.review: bool`: part of what "done" means, so it is set by the caller and never by the worker.
- When the verification command passes and `review` is on, the harness diffs the tree against the run's baseline (`git add -A --intent-to-add && git diff <baseline>`) and asks **the same model, with no tools**, whether the change meets every requirement in the goal. The reply is strict JSON.
- **Critical/High findings** go back to the worker as a user message, and the loop continues **without rollback**: the work is mostly right, so discarding it would be wrong.
- **At most `MAX_REVIEW_ROUNDS` (2) rejections.** After that, the passing exit code stands.
- **Any failure to review accepts:** no baseline, an empty diff, a provider error, or an unparseable reply. A reviewer that can't run must not block completion.
- **Review tokens count in `RunReport.usage`.** `RunReport.reviews` and `review_rejections` are reported.
- **In a phased (`--tdd`) run, only the final phase is reviewed.**
- Exposed as `--review`, and carried in the `--report` JSON.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/folded-default-review-gate`. Each task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back. Task 2 requires Task 1.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` and `superstate-cli`'s `test_cli_daemon_detached_forwarding` are known to be timing-flaky: if one alone fails, re-run once and report both results.
- Existing tests must pass unmodified, except for the specific test edits and struct-literal field additions named in each task.
- Commit with `--no-gpg-sign` and the exact messages below.

---

### Task 1: `folded` is the default context

**Files:**
- Modify: `crates/superstate-cli/src/cli.rs`, `crates/superstate-cli/src/repl.rs`, `crates/superstate-cli/tests/test_cli_dispatch.rs` (the one default assertion)

- [ ] **Step 1: Update the default assertion first.** In `crates/superstate-cli/tests/test_cli_dispatch.rs`, replace `assert_eq!(cli.context, "conversational");` with `assert_eq!(cli.context, "folded");`.

- [ ] **Step 2: Confirm it fails.** Run: `cargo test -p superstate --test test_cli_dispatch`. Expected: FAIL (`"conversational"` != `"folded"`).

- [ ] **Step 3: Change the defaults.**
  1. In `crates/superstate-cli/src/cli.rs`, in the `context` argument, replace `default_value = "conversational",` with `default_value = "folded",`.
  2. In `crates/superstate-cli/src/repl.rs`, replace `context_strategy: "state-only".to_string(),` (in the REPL's default config) with `context_strategy: "folded".to_string(),`.

- [ ] **Step 4: Run it.** Run: `cargo test -p superstate --test test_cli_dispatch`. Expected: pass.

- [ ] **Step 5: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. If another test asserts the old REPL default (`"state-only"` from `ReplConfig`'s default), stop and report its name; do not change it.

- [ ] **Step 6: Commit.**

```bash
git add crates/superstate-cli/
git commit --no-gpg-sign -m "feat(cli): folded is the default context"
```

---

### Task 2: Completeness review gate (`--review`)

Requires Task 1 merged.

**Files:**
- Modify: `crates/superstate-agent/src/state.rs`, `crates/superstate-agent/src/run.rs`, `crates/superstate-cli/src/cli.rs`, `crates/superstate-cli/src/task_runner.rs`, `crates/superstate-cli/src/report.rs`, `crates/superstate-cli/src/repl.rs` (TaskConfig literal only)
- Modify (struct-literal field additions only): `crates/superstate-agent/tests/test_working_set.rs` (`Orchestrator { … }`), `crates/superstate-cli/tests/test_run_report.rs` (`RunReport { … }` and `config()`), `crates/superstate-cli/tests/test_task_runner.rs` (4× `TaskConfig { … }`), `crates/superstate-cli/tests/test_tdd_phases.rs` (`config()`)
- Test: `crates/superstate-agent/tests/test_review_gate.rs` (create)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_review_gate.rs`:

```rust
//! The completeness review gate: after the verification command passes, an independent review of
//! the change against the goal may send the worker back, at most twice; any failure to review
//! accepts; only the final phase of a phased run is reviewed.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use superstate_agent::context::Conversational;
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{Budget, Outcome, Phase, Runner};
use superstate_agent::shell::{LocalShell, Shell};
use superstate_agent::state::State;
use superstate_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};

struct Scripted {
    script: Mutex<Vec<Response>>,
    seen: Mutex<Vec<Request>>,
}

impl Scripted {
    fn new(script: Vec<Response>) -> Self {
        Self {
            script: Mutex::new(script),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Provider for Scripted {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        let mut s = self.script.lock().unwrap();
        Ok(if s.is_empty() { Response::default() } else { s.remove(0) })
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

fn write(path: &str) -> Response {
    Response {
        tool_calls: vec![ToolCall {
            id: format!("w_{path}"),
            name: "write".into(),
            arguments: json!({"path": path, "content": "x\n"}),
            signature: None,
        }],
        ..Default::default()
    }
}

const BLOCKING: &str = r#"```json
{"summary": "incomplete", "findings": [{"severity": "High", "file": "src/lib.rs", "line_range": [1, 2],
 "problem": "separator suppression is not applied to the last row", "suggested_fix": "handle the final row"}],
 "has_blocking_issues": true}
```"#;
const CLEAN: &str = r#"{"summary": "complete", "findings": [], "has_blocking_issues": false}"#;
const LOW_ONLY: &str = r#"{"summary": "nits", "findings": [{"severity": "Low", "file": "a", "line_range": [1, 1],
 "problem": "naming", "suggested_fix": "rename"}], "has_blocking_issues": true}"#;

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let sh = |c: &str| {
        let o = LocalShell.exec(c, d.path(), Duration::from_secs(30)).unwrap();
        assert_eq!(o.exit_code, 0, "{c}: {}", o.merged());
    };
    sh("git init -q && git config user.email a@b && git config user.name c");
    std::fs::write(d.path().join("seed.txt"), "seed\n").unwrap();
    sh("git add -A && git commit --no-gpg-sign -qm base");
    d
}

fn runner<'a>(p: &'a Scripted, root: &Path) -> Runner<'a> {
    Runner {
        provider: p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: root.to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 5,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    }
}

fn reviewed_state(verify: &str) -> State {
    let mut st = State::new("implement separator suppression for every row", verify);
    st.orchestrator.review = true;
    st
}

#[test]
fn a_blocking_review_sends_the_worker_back_then_a_clean_one_accepts() {
    let d = repo();
    let p = Scripted::new(vec![
        write("a.txt"),
        says("done"),
        says(BLOCKING),
        says("fixed the last row"),
        says(CLEAN),
    ]);
    let mut st = reviewed_state("true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!((report.reviews, report.review_rejections), (2, 1));

    let seen = p.seen.lock().unwrap();
    assert_eq!(seen.len(), 5);
    // Review requests carry no tools and the reviewer's own instructions.
    for i in [2, 4] {
        assert!(seen[i].tools.is_empty());
        assert!(seen[i]
            .system
            .contains("You review whether a code change fully accomplishes a task"));
        let Message::User(prompt) = &seen[i].messages[0] else {
            panic!("review prompt must be a user message");
        };
        assert!(prompt.contains("implement separator suppression for every row"));
        assert!(prompt.contains("a.txt"), "the diff must include the new file: {prompt}");
    }
    // The worker's next turn sees the unmet requirement.
    let feedback = format!("{:?}", seen[3].messages);
    assert!(feedback.contains("found requirements it does not meet"), "{feedback}");
    assert!(feedback.contains("separator suppression is not applied to the last row"));
}

#[test]
fn without_review_a_passing_gate_is_accepted_at_once() {
    let d = repo();
    let p = Scripted::new(vec![write("a.txt"), says("done")]);
    let mut st = State::new("anything", "true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.reviews, 0);
    assert_eq!(p.seen.lock().unwrap().len(), 2);
}

#[test]
fn an_unparseable_review_accepts() {
    let d = repo();
    let p = Scripted::new(vec![write("a.txt"), says("done"), says("I think it's fine")]);
    let mut st = reviewed_state("true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!((report.reviews, report.review_rejections), (1, 0));
}

#[test]
fn review_rounds_are_capped_then_the_exit_code_stands() {
    let d = repo();
    let p = Scripted::new(vec![
        write("a.txt"),
        says("done"),
        says(BLOCKING),
        says("done again"),
        says(BLOCKING),
        says("done a third time"),
    ]);
    let mut st = reviewed_state("true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!((report.reviews, report.review_rejections), (2, 2));
    assert_eq!(p.seen.lock().unwrap().len(), 6);
}

#[test]
fn an_empty_diff_is_not_reviewed() {
    let d = repo();
    let p = Scripted::new(vec![says("done")]);
    let mut st = reviewed_state("true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.reviews, 0);
    assert_eq!(p.seen.lock().unwrap().len(), 1);
}

#[test]
fn low_severity_findings_do_not_block() {
    let d = repo();
    let p = Scripted::new(vec![write("a.txt"), says("done"), says(LOW_ONLY)]);
    let mut st = reviewed_state("true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!((report.reviews, report.review_rejections), (1, 0));
}

#[test]
fn a_phased_run_reviews_only_its_final_phase() {
    let d = repo();
    let p = Scripted::new(vec![
        write("a.txt"),
        says("red done"),
        write("b.txt"),
        says("green done"),
        says(CLEAN),
    ]);
    let mut st = reviewed_state("");
    let phase = |id: &str, verify: &str| Phase {
        id: id.into(),
        instruction: format!("do {id}"),
        verify_cmd: verify.into(),
        scope_only: None,
        protect: None,
    };
    let (outcome, report, _) = runner(&p, d.path()).run_phases(
        &mut st,
        &[phase("red", "test -f a.txt"), phase("green", "test -f b.txt")],
    );
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.reviews, 1);
    assert!(st.orchestrator.review, "the caller's setting is restored");
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_review_gate`. Expected: compile errors, because `Orchestrator.review`, `RunReport.reviews` and `review_rejections` do not exist.

- [ ] **Step 3: `Orchestrator.review`.** In `crates/superstate-agent/src/state.rs`:
  1. In `pub struct Orchestrator`, add after `pub verification_cmd: String,`:

```rust
    /// Before a passing verification is accepted, an independent review of the change against the
    /// goal may send the worker back (at most `run::MAX_REVIEW_ROUNDS` times). Part of what "done"
    /// means, so it is set by the caller, never by the worker.
    #[serde(default)]
    pub review: bool,
```

  2. In `State::new`, add `review: false,` to the `Orchestrator { … }` literal.
  3. In `crates/superstate-agent/tests/test_working_set.rs`, add `review: false,` to the `Orchestrator { … }` literal.

- [ ] **Step 4: Report fields.** In `crates/superstate-agent/src/run.rs`, in `pub struct RunReport`, add as its last fields:

```rust
    /// Completeness reviews run after a passing verification.
    pub reviews: u32,
    /// Reviews that sent the worker back with unmet requirements.
    pub review_rejections: u32,
```

In `Runner::run_phases`, directly after `total.provider_retries += report.provider_retries;`, add:

```rust
            total.reviews += report.reviews;
            total.review_rejections += report.review_rejections;
```

- [ ] **Step 5: The reviewer.** In `run.rs`:
  1. Directly above `pub const MAX_PROVIDER_RETRIES: u32 = 6;`, add:

```rust
/// Review rejections allowed before a passing verification is accepted as-is. The exit code stays
/// the verdict: a review can only send the worker back, and only this many times.
pub const MAX_REVIEW_ROUNDS: u32 = 2;

/// The completeness reviewer's instructions. It judges only whether the task is done, not style:
/// a reviewer that blocks on naming or refactoring would spend turns on work nobody asked for.
const REVIEW_SYSTEM: &str = "You review whether a code change fully accomplishes a task. You are \
not its author. Compare the diff against every behaviour the task statement requires. Report only \
requirements the change does not implement or implements incorrectly, each with severity \
\"Critical\" or \"High\". Do not report style, naming, refactoring, documentation or hypothetical \
improvements. Respond with strict JSON only: {\"summary\": \"<one sentence>\", \"findings\": \
[{\"severity\": \"Critical\" or \"High\", \"file\": \"<path>\", \"line_range\": [<start>, <end>], \
\"problem\": \"<the unmet requirement>\", \"suggested_fix\": \"<what to change>\"}], \
\"has_blocking_issues\": true or false}. If every requirement is met, return an empty findings \
list and has_blocking_issues false.";

#[derive(serde::Deserialize)]
struct ReviewVerdict {
    #[serde(default)]
    findings: Vec<ReviewFinding>,
}

#[derive(serde::Deserialize)]
struct ReviewFinding {
    #[serde(default)]
    severity: String,
    #[serde(default)]
    file: String,
    #[serde(default)]
    problem: String,
    #[serde(default)]
    suggested_fix: String,
}
```

  2. Add this method to the `impl<'a> Runner<'a>` block that contains `fn run_gate` (directly after `run_gate`):

```rust
    /// An independent review of the change against the goal, by the same model with no tools.
    /// `Some(feedback)` when it names Critical or High unmet requirements. Any failure to review
    /// (no baseline, empty diff, provider error, unparseable reply) accepts: the exit code remains
    /// the verdict, and a reviewer that cannot run must not be able to block completion.
    fn review(
        &self,
        state: &State,
        baseline: Option<&str>,
        report: &mut RunReport,
    ) -> Option<String> {
        let base = baseline?;
        let out = self
            .shell
            .exec(
                &format!("git add -A --intent-to-add && git diff {base}"),
                &self.root,
                Duration::from_secs(60),
            )
            .ok()?;
        if out.exit_code != 0 || out.stdout.trim().is_empty() {
            return None;
        }
        let prompt = format!(
            "## Task\n{}\n\n## Change (git diff against the starting point)\n```diff\n{}\n```\n",
            state.orchestrator.goal,
            tools::distil(&out.stdout, 1500)
        );
        let req = Request {
            system: REVIEW_SYSTEM.into(),
            messages: vec![Message::User(prompt)],
            tools: Vec::new(),
            max_output_tokens: None,
        };
        let resp = self.provider.complete(&req).ok()?;
        report.usage.input += resp.usage.input;
        report.usage.cached += resp.usage.cached;
        report.usage.output += resp.usage.output;
        report.usage.thinking += resp.usage.thinking;
        report.reviews += 1;

        let text = resp.text;
        let (start, end) = (text.find('{')?, text.rfind('}')?);
        if end < start {
            return None;
        }
        let verdict: ReviewVerdict = serde_json::from_str(&text[start..=end]).ok()?;
        let blocking: Vec<&ReviewFinding> = verdict
            .findings
            .iter()
            .filter(|f| matches!(f.severity.to_ascii_lowercase().as_str(), "critical" | "high"))
            .collect();
        if blocking.is_empty() {
            return None;
        }
        let mut feedback = String::from(
            "The verification command passed, but an independent review of your change against \
             the task found requirements it does not meet:\n",
        );
        for f in blocking {
            feedback.push_str(&format!("- {} (in {}): {}\n", f.problem, f.file, f.suggested_fix));
        }
        feedback.push_str("\nAddress them, then stop again; the verification command will run again.");
        Some(feedback)
    }
```

- [ ] **Step 6: Consult it when the gate passes.** In `Runner::run_until`:
  1. Directly above `for turn in 1..=self.budget.max_turns {`, add `let mut review_rounds = 0;`.
  2. Replace

```rust
            if passed {
                return (Outcome::Verified, report);
            }
```

     with

```rust
            if passed {
                // The exit code is necessary, not sufficient: a worker-written gate is only as
                // complete as the worker's reading of the task (superstate-eval FINDINGS 13.38).
                if state.orchestrator.review && review_rounds < MAX_REVIEW_ROUNDS {
                    if let Some(feedback) = self.review(state, baseline.as_deref(), &mut report) {
                        review_rounds += 1;
                        report.review_rejections += 1;
                        history.push(Message::User(feedback));
                        continue;
                    }
                }
                return (Outcome::Verified, report);
            }
```

- [ ] **Step 7: Review only the final phase.** In `Runner::run_phases`:
  1. Directly after `let task = state.orchestrator.goal.clone();`, add `let wants_review = state.orchestrator.review;`.
  2. Directly after `state.orchestrator.verification_cmd = phase.gate(&start);`, add `state.orchestrator.review = wants_review && i + 1 == phases.len();`.
  3. Immediately before **every** `return` in `run_phases` (the early git-repository error, the non-verified outcome, the checkpoint failure, and the final `(Outcome::Verified, total, results)`), add `state.orchestrator.review = wants_review;`. For the final expression, add the statement before it.

- [ ] **Step 8: `--review` on the CLI and in the report.**
  1. In `crates/superstate-cli/src/cli.rs`, in `struct Cli`, directly after the `pub tdd: bool,` field, add:

```rust
    /// Before accepting a passing verification, review the change against the task with the same
    /// model; unmet requirements send the worker back (at most twice)
    #[arg(long)]
    pub review: bool,
```

  2. In `crates/superstate-cli/src/task_runner.rs`, add `pub review: bool,` (with doc comment `/// Completeness review before a passing verification is accepted (`--review`).`) as the last field of `struct TaskConfig`; add `review: cli.review,` after `tdd: cli.tdd,` in `From<&Cli>`; and as the first statement inside `run_task_with_state_and_writer`, before `let started`, add `state.orchestrator.review = config.review;`.
  3. Add `review: false,` as the last field of every other `TaskConfig { … }` literal: `src/repl.rs`, the four in `tests/test_task_runner.rs`, and the `config()` functions in `tests/test_run_report.rs` and `tests/test_tdd_phases.rs`.
  4. In `crates/superstate-cli/src/report.rs`: add `pub reviews: u32,` and `pub review_rejections: u32,` directly after `pub provider_retries: u32,` in `RunRecord`; `reviews: 0,` and `review_rejections: 0,` after `provider_retries: 0,` in `base`; and `reviews: report.reviews,` and `review_rejections: report.review_rejections,` after `provider_retries: report.provider_retries,` in `from_run`.
  5. In `crates/superstate-cli/tests/test_run_report.rs`, add `reviews: 0,` and `review_rejections: 0,` as the last fields of the `RunReport { … }` literal.

- [ ] **Step 9: Run the new tests.** Run: `cargo test -p superstate-agent --test test_review_gate`. Expected: 7 passed.

- [ ] **Step 10: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [ ] **Step 11: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(agent): completeness review gate before a passing verification is accepted"
```
