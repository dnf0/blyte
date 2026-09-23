# Harness-Observed Action Ledger Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement an episodic, harness-observed Action Ledger in `superstate-agent` to eliminate the "Goldfish" effect under `StateOnly` context bounding, ensuring models retain full awareness of past actions and errors while preserving $\mathcal{O}(1)$ prompt order of magnitude.

**Architecture:** The harness objectively intercepts tool calls and formats 1-line deterministic summaries into `state.runtime.actions` without LLM calls. In `StateOnly::build`, this ledger is rendered as `## Actions Taken So Far` inside the initial user message, retaining the strict `messages.len() <= 3` prompt invariant.

**Tech Stack:** Rust 2021, `superstate-agent`, `serde`, `serde_json`, `regex`.

## Global Constraints

- Target crate: `crates/superstate-agent` (with CLI verification in `crates/superstate-cli`).
- Rust 2021 edition, zero compiler warnings (`cargo check --workspace --all-targets`).
- Strict linter compliance: zero clippy warnings (`cargo clippy --workspace --all-targets -- -D warnings`).
- Invariant I: Type safety across partitions: Action ledger lives in `Runtime` ($R$) and cannot be modified by `apply_agent_patch`.
- Invariant II: Prompt context bound in `StateOnly`: `messages.len() <= 3` remains true for any turn count.
- Invariant III: Deterministic zero-latency extraction: no intermediate LLM calls for summarization.

---

### Task 1: Add `ActionEntry` and `Runtime.actions` to `crates/superstate-agent/src/state.rs`

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-agent/src/state.rs`
- Modify: `crates/superstate-agent/tests/test_state.rs`

**Interfaces:**
- Produces:
  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  pub struct ActionEntry {
      pub turn: u32,
      pub tool: String,
      pub target: String,
      pub summary: String,
      pub is_error: bool,
  }
  ```
  `Runtime.actions: Vec<ActionEntry>` with `#[serde(default)]`.

- [ ] **Step 1: Write failing unit tests for `ActionEntry` serialization and default backward compatibility**
  In `crates/superstate-agent/tests/test_state.rs`, add tests:
  - `test_action_entry_serialization`: verify roundtrip JSON serialization of `ActionEntry`.
  - `test_runtime_actions_default_backward_compatibility`: deserialize a JSON string representing `Runtime` without an `"actions"` key, asserting `runtime.actions.is_empty()`.
  - `test_agent_patch_cannot_mutate_runtime_actions`: verify that calling `state.apply_agent_patch(&json!({"actions": "hacked"}))` leaves `runtime.actions` untouched.

- [ ] **Step 2: Run tests to observe failure**
  Run `cargo test -p superstate-agent --test test_state` to verify it fails to compile due to missing `ActionEntry` and `actions` field.

- [ ] **Step 3: Implement `ActionEntry` and update `Runtime` in `crates/superstate-agent/src/state.rs`**
  Add `ActionEntry` struct with docstrings.
  Add `pub actions: Vec<ActionEntry>` with `#[serde(default)]` to `Runtime`.

- [ ] **Step 4: Run tests to verify pass**
  Run `cargo test -p superstate-agent --test test_state` and verify all tests pass.

- [ ] **Step 5: Commit changes**
  `git add crates/superstate-agent/src/state.rs crates/superstate-agent/tests/test_state.rs`
  `git commit --no-gpg-sign -m "feat(agent): add ActionEntry and actions ledger to Runtime state"`

---

### Task 2: Deterministic Target and Outcome Extraction Helpers in `run.rs`

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-agent/src/run.rs`
- Create: `crates/superstate-agent/tests/test_action_extraction.rs`

**Interfaces:**
- Produces:
  ```rust
  pub(crate) fn extract_action_target(tool: &str, args: &serde_json::Value) -> String;
  pub(crate) fn extract_action_summary(tool: &str, content: &str, is_error: bool) -> String;
  ```

- [ ] **Step 1: Write unit tests for target and summary extraction across all tools**
  Create `crates/superstate-agent/tests/test_action_extraction.rs` covering:
  - `read`: target contains path and line range; summary counts lines.
  - `write`: target contains path; summary reports write status.
  - `edit`: target contains path; summary reports replacement or error.
  - `bash`: target contains clamped command; summary reports `exit 0` or parses last error line for exit > 0.
  - `glob`: target contains pattern; summary reports matched file count.
  - `grep`: target contains pattern and path; summary reports match count.
  - `note`: target contains patch summary; summary reports key count.

- [ ] **Step 2: Run tests to observe failure**
  Run `cargo test -p superstate-agent --test test_action_extraction` to verify it fails to compile.

- [ ] **Step 3: Implement extraction helpers in `crates/superstate-agent/src/run.rs`**
  Implement `extract_action_target` and `extract_action_summary` with string slicing, line counting, and error extraction logic.

- [ ] **Step 4: Run tests to verify pass**
  Run `cargo test -p superstate-agent --test test_action_extraction` and verify all tests pass.

- [ ] **Step 5: Commit changes**
  `git add crates/superstate-agent/src/run.rs crates/superstate-agent/tests/test_action_extraction.rs`
  `git commit --no-gpg-sign -m "feat(agent): implement deterministic action target and outcome summarizers"`

---

### Task 3: Wire Action Ledger Logging & Rollback Tracking in `Runner::run`

- **Depends on:** Task 2

**Files:**
- Modify: `crates/superstate-agent/src/run.rs`
- Modify: `crates/superstate-agent/tests/test_run.rs`

**Interfaces:**
- Consumes: `extract_action_target`, `extract_action_summary`, `ActionEntry`
- Produces: Mutated `state.runtime.actions` after each tool call and rollback event.

- [ ] **Step 1: Write integration test in `test_run.rs` for action recording and rollback tracking**
  Add `test_runner_records_actions_in_runtime_ledger` and `test_runner_records_rollback_in_action_ledger`:
  - Run multi-turn mock provider with `bash` and `read`.
  - Assert `state.runtime.actions.len() == 2`.
  - Trigger gate failure with rollback enabled: assert `state.runtime.actions` contains a final entry with `tool: "rollback"`, `is_error: true`.

- [ ] **Step 2: Run tests to observe failure**
  Run `cargo test -p superstate-agent --test test_run test_runner_records_actions` to observe failure.

- [ ] **Step 3: Update `Runner::run` in `crates/superstate-agent/src/run.rs`**
  - When tool execution completes, create `ActionEntry` and push to `state.runtime.actions`.
  - When gate fails and rollback runs, push rollback `ActionEntry` to `state.runtime.actions`.

- [ ] **Step 4: Run tests to verify pass**
  Run `cargo test -p superstate-agent --test test_run` and verify all tests pass.

- [ ] **Step 5: Commit changes**
  `git add crates/superstate-agent/src/run.rs crates/superstate-agent/tests/test_run.rs`
  `git commit --no-gpg-sign -m "feat(agent): wire action logging and rollback tracking into Runner loop"`

---

### Task 4: Wire Context Assembly in `StateOnly::build`

- **Depends on:** Task 3

**Files:**
- Modify: `crates/superstate-agent/src/context.rs`
- Modify: `crates/superstate-agent/tests/test_run.rs`

**Interfaces:**
- Produces: `StateOnly::build` rendering `## Actions Taken So Far` into `Message::User` while keeping `messages.len() <= 3`.

- [ ] **Step 1: Write test for `StateOnly` context formatting with action ledger**
  In `crates/superstate-agent/tests/test_run.rs`, add `test_state_only_includes_action_ledger_in_user_prompt`:
  - Populate `state.runtime.actions` with 3 entries.
  - Call `StateOnly.build(&state, &history)`.
  - Assert that `messages.len() <= 3`.
  - Assert that `messages[0]` text contains `"## Actions Taken So Far"` and lists all 3 actions with `[Turn N]` and summaries.

- [ ] **Step 2: Run test to observe failure**
  Run `cargo test -p superstate-agent --test test_run test_state_only_includes_action_ledger` to observe failure.

- [ ] **Step 3: Update `StateOnly::build` in `crates/superstate-agent/src/context.rs`**
  Format `state.runtime.actions` into `## Actions Taken So Far` markdown section in the primary user message.

- [ ] **Step 4: Run tests to verify pass**
  Run `cargo test -p superstate-agent --test test_run` and verify all tests pass.

- [ ] **Step 5: Commit changes**
  `git add crates/superstate-agent/src/context.rs crates/superstate-agent/tests/test_run.rs`
  `git commit --no-gpg-sign -m "feat(agent): format action ledger into StateOnly prompt context"`

---

### Task 5: End-to-End CLI Verification and Invariant Checks

- **Depends on:** Task 4

**Files:**
- Create: `crates/superstate-cli/tests/test_action_ledger_e2e.rs`
- Modify: `crates/superstate-cli/src/repl.rs` (ensure `/state` displays actions count)

**Interfaces:**
- Verified end-to-end action tracking through CLI REPL and headless solver.

- [ ] **Step 1: Write end-to-end integration test**
  In `crates/superstate-cli/tests/test_action_ledger_e2e.rs`, simulate multi-turn prompt in the REPL stream, run `/state`, and verify the output contains `"actions": [` with recorded events.

- [ ] **Step 2: Run test to observe failure/pass**
  Run `cargo test -p superstate --test test_action_ledger_e2e`.

- [ ] **Step 3: Update REPL / CLI state formatting if needed**
  Ensure `/state` pretty-prints the action count and recent actions.

- [ ] **Step 4: Full workspace checks**
  - `cargo check --workspace --all-targets`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`

- [ ] **Step 5: Commit changes**
  `git add crates/superstate-cli/tests/test_action_ledger_e2e.rs crates/superstate-cli/src/repl.rs`
  `git commit --no-gpg-sign -m "test(cli): add end-to-end verification for action ledger in REPL"`
