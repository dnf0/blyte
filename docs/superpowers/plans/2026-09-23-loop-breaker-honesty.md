# Loop-Breaker Honesty Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** The loop-breaker and the prompt may only claim what an exit code actually shows.

**Why:** In the 2026-09-23 TDD-gated smoke run (`scc-bounded-memory-spilling`), GREEN ran 211 turns and never edited a file. The event log shows three things:
- During RED, the compile-only `go test -run '^$' ./...` (which runs no tests) exited 0.
- From then on, the #44 guard intercepted GREEN's first real `go test -run TestReproduceIssue ./...` as "Unit tests … have already passed". RED had just proved that test fails.
- `context.rs` added "## Verification Status: Unit tests are passing" to every GREEN prompt.

Root cause: "some test-like command exited 0" was treated as "the tests pass". That was true across different commands, and across phases.

**Architecture:** Four changes in `superstate-agent`, with no new dependencies:
1. `run_phases` writes a phase-start marker into the action ledger, and every loop-breaker inference looks no further back than the latest marker.
2. Guard 1 intercepts a test command only when the **identical** command already exited 0 with no edit since. A truncated ledger target counts as "unknown", so the command runs.
3. The "passing" claims (guard 2's advice and the prompt notice) require the **verification command itself** to have exited 0 since the last edit, in this phase, and they name that command instead of claiming "unit tests are passing".
4. `WorkingSet` remembers which command produced `last_failure`, and only that command passing clears it.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/native-harness-autonomous-tdd` (PR #47). The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` is known to be timing-flaky: if it alone fails, re-run once and report both results.
- Existing tests must pass unmodified.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Honest loop-breaker claims, scoped to the current phase

**Files:**
- Modify: `crates/superstate-agent/src/run.rs`, `crates/superstate-agent/src/context.rs`, `crates/superstate-agent/src/working_set.rs`
- Test: `crates/superstate-agent/tests/test_loop_breaker_honesty.rs` (create); `crates/superstate-agent/tests/test_phases.rs` (append one test)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_loop_breaker_honesty.rs`:

```rust
//! The loop-breaker may only claim what an exit code actually shows.
//!
//! Regression for the 2026-09-23 native TDD smoke run: a compile-only `go test -run '^$'`
//! exiting 0 made the guard and the prompt tell a GREEN worker its failing test was passing,
//! and the worker never edited a file.

use serde_json::json;
use superstate_agent::context::{ContextStrategy, StateOnly};
use superstate_agent::run::{
    current_phase_actions, extract_action_target, should_intercept_redundant_bash,
    verification_passed_since_edit, PHASE_MARKER_TOOL,
};
use superstate_agent::state::{ActionEntry, State};
use superstate_agent::working_set::WorkingSet;
use superstate_provider::{Message, ToolCall};

fn act(tool: &str, target: &str, summary: &str) -> ActionEntry {
    ActionEntry {
        turn: 1,
        tool: tool.into(),
        target: target.into(),
        summary: summary.into(),
        is_error: false,
    }
}

fn marker(id: &str) -> ActionEntry {
    act(PHASE_MARKER_TOOL, id, "phase started")
}

fn bash(cmd: &str) -> ToolCall {
    ToolCall {
        id: "call_bash".into(),
        name: "bash".into(),
        arguments: json!({"command": cmd}),
        signature: None,
    }
}

fn prompt_text(st: &State) -> String {
    StateOnly
        .build(st, &[])
        .iter()
        .map(|m| match m {
            Message::User(t) => t.clone(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_different_test_command_passing_does_not_block_this_one() {
    let actions = vec![
        act("write", "reproduce_issue_test.go", "wrote 95 lines"),
        act("bash", "go test -run '^$' ./...", "exit 0"),
    ];
    let guard = should_intercept_redundant_bash(
        &bash("go test -run TestReproduceIssue ./..."),
        &actions,
        &WorkingSet::default(),
        "",
    );
    assert!(guard.is_none(), "{guard:?}");
}

#[test]
fn the_identical_command_is_still_intercepted() {
    let actions = vec![
        act("write", "a.go", "wrote 3 lines"),
        act("bash", "go test ./...", "exit 0"),
    ];
    let msg = should_intercept_redundant_bash(
        &bash("go test ./..."),
        &actions,
        &WorkingSet::default(),
        "",
    )
    .expect("an identical passing command is intercepted");
    assert!(msg.contains("already exited 0"), "{msg}");
    assert!(!msg.contains("have already passed"), "{msg}");
}

#[test]
fn an_edit_since_the_pass_lets_it_run_again() {
    let actions = vec![
        act("write", "a.go", "wrote 3 lines"),
        act("bash", "go test ./...", "exit 0"),
        act("edit", "a.go", "replaced 1 line"),
    ];
    assert!(should_intercept_redundant_bash(
        &bash("go test ./..."),
        &actions,
        &WorkingSet::default(),
        ""
    )
    .is_none());
}

#[test]
fn a_truncated_command_is_never_treated_as_identical() {
    let long = "go test -run 'TestAVeryLongNameThatGoesOnAndOn|TestAnotherOne' ./...";
    let target = extract_action_target("bash", &json!({"command": long}));
    assert!(target.ends_with("..."), "{target}");
    let actions = vec![
        act("write", "a.go", "wrote 3 lines"),
        act("bash", &target, "exit 0"),
    ];
    assert!(
        should_intercept_redundant_bash(&bash(long), &actions, &WorkingSet::default(), "")
            .is_none()
    );
}

#[test]
fn a_pass_in_an_earlier_phase_does_not_carry_into_the_next() {
    let actions = vec![
        act("write", "a.go", "wrote 3 lines"),
        act("bash", "go test ./...", "exit 0"),
        marker("fix-implementation"),
    ];
    assert!(should_intercept_redundant_bash(
        &bash("go test ./..."),
        &actions,
        &WorkingSet::default(),
        ""
    )
    .is_none());
}

#[test]
fn current_phase_actions_start_after_the_latest_marker() {
    let actions = vec![
        act("bash", "a", "exit 0"),
        marker("red"),
        act("bash", "b", "exit 0"),
        marker("green"),
        act("bash", "c", "exit 0"),
    ];
    let current = current_phase_actions(&actions);
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].target, "c");
    assert_eq!(current_phase_actions(&actions[..1]).len(), 1);
}

#[test]
fn only_the_verification_command_itself_counts_as_verified() {
    let v = "go test ./...";
    let compile_only = vec![
        act("write", "a.go", "wrote 3 lines"),
        act("bash", "go test -run '^$' ./...", "exit 0"),
    ];
    assert!(!verification_passed_since_edit(&compile_only, v));

    let real = vec![act("write", "a.go", "wrote 3 lines"), act("bash", v, "exit 0")];
    assert!(verification_passed_since_edit(&real, v));
    assert!(!verification_passed_since_edit(&real, ""));

    let mut next_phase = real.clone();
    next_phase.push(marker("green"));
    assert!(!verification_passed_since_edit(&next_phase, v));
}

#[test]
fn inspection_advice_never_claims_tests_pass_from_a_compile_only_check() {
    let actions = vec![
        act("write", "reproduce_issue_test.go", "wrote 95 lines"),
        act("bash", "go test -run '^$' ./...", "exit 0"),
        act("bash", "git status", "exit 0"),
    ];
    let msg = should_intercept_redundant_bash(
        &bash("git status"),
        &actions,
        &WorkingSet::default(),
        "go test ./...",
    )
    .expect("an immediately repeated inspection is intercepted");
    assert!(!msg.contains("already passed"), "{msg}");
    assert!(!msg.contains("exited 0 after your last edit"), "{msg}");
}

#[test]
fn the_prompt_never_claims_tests_pass_from_a_compile_only_check() {
    let mut st = State::new("fix it", "go test ./...");
    st.runtime.actions = vec![
        act("write", "reproduce_issue_test.go", "wrote 95 lines"),
        act("bash", "go test -run '^$' ./...", "exit 0"),
    ];
    let text = prompt_text(&st);
    assert!(!text.contains("## Verification Status"), "{text}");

    st.runtime.actions.push(act("bash", "go test ./...", "exit 0"));
    let text = prompt_text(&st);
    assert!(
        text.contains("The verification command `go test ./...` exited 0 after your last edit"),
        "{text}"
    );
    assert!(!text.contains("Unit tests are passing"), "{text}");
}

#[test]
fn a_different_command_passing_does_not_erase_a_recorded_failure() {
    let mut ws = WorkingSet::default();
    ws.record_failure_for(
        "go test -run TestReproduceIssue ./...",
        "--- FAIL: TestReproduceIssue",
    );
    ws.clear_failure_for("go test -run '^$' ./...");
    assert!(ws.last_failure.is_some());
    ws.clear_failure_for("go test -run TestReproduceIssue ./...");
    assert!(ws.last_failure.is_none());
    assert!(ws.last_failure_cmd.is_none());
}
```

Append to the end of `crates/superstate-agent/tests/test_phases.rs`:

```rust
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
        &[phase("first", "test -f a.txt"), phase("second", "test -f b.txt")],
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
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_loop_breaker_honesty --test test_phases`. Expected: compile errors (`PHASE_MARKER_TOOL`, `current_phase_actions`, `verification_passed_since_edit`, `record_failure_for`, `clear_failure_for` and `last_failure_cmd` do not exist).

- [ ] **Step 3: Remember which command failed.** In `crates/superstate-agent/src/working_set.rs`:
  1. In `struct WorkingSet`, add after `pub last_failure: Option<String>,`:

```rust
    /// The command whose failure `last_failure` holds. Only that command passing clears it:
    /// a compile-only check exiting 0 must not erase a real test failure from the prompt.
    #[serde(default)]
    pub last_failure_cmd: Option<String>,
```

  2. Replace the body of `clear_failure` so it also forgets the command:

```rust
    pub fn clear_failure(&mut self) {
        self.last_failure = None;
        self.last_failure_cmd = None;
    }
```

  3. Add directly after `clear_failure`:

```rust
    /// Record a failure together with the command that produced it.
    pub fn record_failure_for(&mut self, cmd: &str, output: &str) {
        self.record_failure(output);
        self.last_failure_cmd = Some(cmd.trim().to_string());
    }

    /// Clear the recorded failure only if `cmd` is the command that produced it. A failure
    /// recorded without a command (older state) is cleared by any passing test command, as before.
    pub fn clear_failure_for(&mut self, cmd: &str) {
        let same = match &self.last_failure_cmd {
            None => true,
            Some(c) => c == cmd.trim(),
        };
        if same {
            self.clear_failure();
        }
    }
```

- [ ] **Step 4: Phase markers and the two inference helpers.** In `crates/superstate-agent/src/run.rs`, add directly above `pub struct Phase {` (and above its doc comment):

```rust
/// The action-ledger tool name `run_phases` writes when a phase begins. Loop-breaker inferences
/// look no further back than the latest marker: what passed in RED says nothing about GREEN.
pub const PHASE_MARKER_TOOL: &str = "phase";

/// Actions since the current phase began, or all of them when the run has no phases.
pub fn current_phase_actions(actions: &[ActionEntry]) -> &[ActionEntry] {
    match actions.iter().rposition(|a| a.tool == PHASE_MARKER_TOOL) {
        Some(i) => &actions[i + 1..],
        None => actions,
    }
}

/// Whether, in the current phase and since its last edit, the worker ran the verification
/// command itself and it exited 0. That is the only exit code that predicts the gate. Another
/// test-like command passing (a compile-only `go test -run '^$'`, a single test) says nothing
/// about it. Treating it as if it did told a GREEN worker its failing test was passing.
pub fn verification_passed_since_edit(actions: &[ActionEntry], verification_cmd: &str) -> bool {
    let v = verification_cmd.trim();
    if v.is_empty() || v == "true" || v == "false" {
        return false;
    }
    let phase = current_phase_actions(actions);
    let edited = phase
        .iter()
        .any(|a| (a.tool == "edit" || a.tool == "write") && !a.is_error);
    edited
        && phase
            .iter()
            .rev()
            .take_while(|a| a.tool != "edit" && a.tool != "write")
            .any(|a| {
                a.tool == "bash"
                    && !a.is_error
                    && !a.summary.starts_with("intercepted")
                    && a.target.trim() == v
            })
}
```

- [ ] **Step 5: Mark each phase start.** In `Runner::run_phases`, directly after `state.orchestrator.verification_cmd = phase.gate(&start);`, insert:

```rust
            state.runtime.actions.push(ActionEntry {
                turn: 0,
                tool: PHASE_MARKER_TOOL.into(),
                target: phase.id.clone(),
                summary: format!("phase {}/{} started", i + 1, phases.len()),
                is_error: false,
            });
```

- [ ] **Step 6: Guard 1 intercepts only an identical command.** In `should_intercept_redundant_bash`:
  1. Rename the parameter `working_set: &crate::working_set::WorkingSet,` to `_working_set: &crate::working_set::WorkingSet,` (it is no longer read; the signature stays for callers).
  2. Replace the whole block from `// Guard 1: Redundant test execution after tests have already passed without file modifications` through the closing `}` of `if is_test && working_set.last_failure.is_none() { … }` with:

```rust
    // Guard 1: re-running a command that already exited 0, with no edit since, cannot tell the
    // worker anything new. Only the IDENTICAL command counts: a different test command passing
    // says nothing about this one. A target the ledger truncated cannot be compared, so it runs.
    if is_test_command(cmd, verification_cmd) {
        let target = extract_action_target("bash", &call.arguments);
        let already_passed = !target.ends_with("...")
            && current_phase_actions(actions)
                .iter()
                .rev()
                .take_while(|a| a.tool != "edit" && a.tool != "write")
                .any(|a| {
                    a.tool == "bash"
                        && a.target == target
                        && !a.is_error
                        && !a.summary.starts_with("intercepted")
                });
        if already_passed {
            return Some(format!(
                "[Harness Working Memory Guard]: `{cmd}` already exited 0 and no file has changed since, \
                 so it would exit 0 again. If your change is complete, stop calling tools to submit it \
                 for verification."
            ));
        }
    }
```

  3. Delete the `let has_edit = …;` statement and the whole `let tests_passing = …;` statement from guard 2.
  4. Replace `if last_was_same || recent_same >= 2 || tests_passing {` with:

```rust
    let verified = verification_passed_since_edit(actions, verification_cmd);
    if last_was_same || recent_same >= 2 || verified {
```

  5. Replace `let advice = if tests_passing {` with `let advice = if verified {`, and replace that branch's `format!(…)` with:

```rust
            format!(
                "[Harness Working Memory Guard]: `{cmd}` inspection intercepted: the verification \
                 command `{}` exited 0 after your last edit. If your change is complete, stop calling \
                 tools to submit it.",
                verification_cmd.trim()
            )
```

     Leave the `else` branch unchanged.

- [ ] **Step 7: Clear failures only for the same command.** In `Runner::run_until`'s `"bash" =>` arm, replace `state.runtime.working_set.record_failure(&outcome.content);` with `state.runtime.working_set.record_failure_for(cmd, &outcome.content);` and `state.runtime.working_set.clear_failure();` with `state.runtime.working_set.clear_failure_for(cmd);`.

- [ ] **Step 8: An honest prompt notice.** In `crates/superstate-agent/src/context.rs`, in `StateOnly::build`'s `else` branch, replace everything from `let has_edit = state` through the closing `}` of `if tests_passing { … }` with:

```rust
            // Only the verification command itself exiting 0 predicts the gate. Claiming "tests
            // are passing" from any test-like command told a GREEN worker its failing test passed.
            if crate::run::verification_passed_since_edit(
                &state.runtime.actions,
                &state.orchestrator.verification_cmd,
            ) {
                user_prompt.push_str(&format!(
                    "\n## Verification Status\nThe verification command `{}` exited 0 after your \
                     last edit. If your change is complete, stop calling tools to submit it.\n",
                    state.orchestrator.verification_cmd.trim()
                ));
            }
```

- [ ] **Step 9: Run the new tests.** Run: `cargo test -p superstate-agent --test test_loop_breaker_honesty --test test_phases`. Expected: 10 and 11 passed.

- [ ] **Step 10: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. If an existing test fails, stop and report its name and assertion. Do not change it.

- [ ] **Step 11: Commit.**

```bash
git add crates/superstate-agent/
git commit --no-gpg-sign -m "fix(agent): loop-breaker claims only what an exit code shows, per phase"
```
