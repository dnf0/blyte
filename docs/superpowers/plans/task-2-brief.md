# Task 2 Brief: Harness Working Set Lifecycle & Synchronization in `run.rs`

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-harness-observed-working-set`

## Context
Task 1 defined `WorkingSet` and `FileSnippet` in `Runtime` (R).
Task 2 wires the deterministic lifecycle into the tool execution loop in `crates/superstate-agent/src/run.rs`:
Whenever `read`, `edit`, or `bash` runs, the harness automatically updates `state.runtime.working_set` so external working memory is kept in sync without requiring the LLM to call `note`.

## Global Constraints
- Target crate: `crates/superstate-agent`
- Rust 2021 edition, zero compiler warnings (`cargo check -p superstate-agent`, `cargo clippy -p superstate-agent --all-targets -- -D warnings`)
- Clean execution on Unix platforms (macOS / Linux)
- Invariant I: `working_set` lives strictly in `Runtime` (R).
- Invariant III: In-memory string operations only; zero external LLM latency.
- Conventional commit message with `--no-gpg-sign`.

## Files to Modify / Create
- Modify: `crates/superstate-agent/src/run.rs`
- Modify: `crates/superstate-agent/tests/test_run.rs`

## Step-by-Step Instructions

### 1. Write the failing tests first (TDD RED phase)
In `crates/superstate-agent/tests/test_run.rs`:
- Add `test_working_set_synchronized_during_run`:
  Simulate a run where the model calls `read`, then `edit`, then `bash` passing test, then stops. Assert `state.runtime.working_set.files` contains the edited file with updated content and no failures.
- Add `test_working_set_tracks_test_failure`:
  Simulate a run where the model calls `bash` on a failing test suite. Assert `state.runtime.working_set.last_failure` is populated with the distilled error.
- Add `test_working_set_clears_test_failure_on_pass`:
  Simulate a run where a failing test failure is recorded and subsequently cleared when a test passes.

### 2. Run tests to verify they fail
Run: `cargo test -p superstate-agent --test test_run test_working_set`
Observe failure.

### 3. Implement minimal code (GREEN phase)
In `crates/superstate-agent/src/run.rs`, in the tool execution loop where `outcome` is processed:
- For `read`:
  ```rust
  if !outcome.is_error {
      let path = call.arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
      let start = call.arguments.get("start_line").and_then(|v| v.as_u64()).map(|v| v as usize);
      let end = call.arguments.get("end_line").and_then(|v| v.as_u64()).map(|v| v as usize);
      state.runtime.working_set.add_or_update(path, &outcome.content, start, end, turn);
  }
  ```
- For `edit`:
  ```rust
  if !outcome.is_error {
      let path = call.arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
      let old = call.arguments.get("old").and_then(|v| v.as_str()).unwrap_or("");
      let new = call.arguments.get("new").and_then(|v| v.as_str()).unwrap_or("");
      state.runtime.working_set.apply_edit(path, old, new, turn);
  }
  ```
- For `bash`:
  ```rust
  let cmd = call.arguments.get("command").and_then(|v| v.as_str()).unwrap_or("");
  let is_test_cmd = cmd.contains("test") || cmd.contains("pytest") || cmd.contains("unittest");
  if outcome.is_error && is_test_cmd {
      state.runtime.working_set.record_failure(&outcome.content);
  } else if !outcome.is_error && is_test_cmd {
      state.runtime.working_set.clear_failure();
  }
  ```

### 4. Run tests and linter
Run:
- `cargo test -p superstate-agent --test test_run`
- `cargo check -p superstate-agent --all-targets`
- `cargo clippy -p superstate-agent --all-targets -- -D warnings`

### 5. Commit
```bash
git add crates/superstate-agent/src/run.rs crates/superstate-agent/tests/test_run.rs
git commit --no-gpg-sign -m "feat(agent): synchronize working set on read, edit, and bash tool calls"
roborev status; roborev show HEAD
```

### 6. Write report
Save report to `/Users/danielfisher/repos/superstate/.wt/feat-harness-observed-working-set/docs/superpowers/plans/task-2-report.md`.
