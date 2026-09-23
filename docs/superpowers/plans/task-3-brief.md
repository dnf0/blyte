# Task 3: Wire Action Ledger Logging and Rollback Tracking into Runner::run

Working Directory: `/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness`

## What to Implement

In `crates/superstate-agent/src/run.rs`:
1. **Tool Call Logging**:
   In `Runner::run`, inside the tool call execution loop (around lines 245-274):
   After executing each tool (whether built-in tool or `"note"`), construct an `ActionEntry` using `extract_action_target` and `extract_action_summary`:
   ```rust
   let target = extract_action_target(&call.name, &call.arguments);
   let summary = extract_action_summary(&call.name, &outcome.content, outcome.is_error);
   state.runtime.actions.push(crate::state::ActionEntry {
       turn: report.turns,
       tool: call.name.clone(),
       target,
       summary,
       is_error: outcome.is_error,
   });
   ```
   (Note: can also use `target` and `summary` for `Event::Tool` if clean, or keep them consistent).

2. **Rollback Action Logging**:
   When a gate fails and rollback is executed (around lines 292-300):
   ```rust
   if self.rollback {
       if let Some(base) = &baseline {
           self.reset_to(base);
           state.runtime.rollbacks += 1;
           let short_sha = &base[..base.len().min(8)];
           self.emit(Event::RolledBack {
               to: short_sha,
           });
           state.runtime.actions.push(crate::state::ActionEntry {
               turn: report.turns,
               tool: "rollback".into(),
               target: format!("checkpoint {short_sha}"),
               summary: "Gate failed; working tree reset to verified state".into(),
               is_error: true,
           });
       }
   }
   ```

---

## Testing Requirements (TDD)

In `crates/superstate-agent/tests/test_run.rs`:
Add tests:
1. `test_actions_recorded_during_run`:
   Run a `Runner` with a `ScriptedProvider` making several tool calls (e.g. `bash`, `read`, `note`).
   Verify `state.runtime.actions` contains entries for each tool with correct `turn`, `tool`, `target`, `summary`, and `is_error` status.
2. `test_rollback_appends_action_entry`:
   Run a `Runner` with rollback enabled (`runner.rollback = true;`) where a gate fails on the first attempt and passes on the second.
   Verify `state.runtime.actions` contains an `ActionEntry` with `tool == "rollback"`, `target.starts_with("checkpoint ")`, `is_error == true`.

## Execution Steps
1. Write the tests in `crates/superstate-agent/tests/test_run.rs`.
2. Run `cargo test -p superstate-agent --test test_run` to observe test failures.
3. Update `crates/superstate-agent/src/run.rs` to record the actions and rollback entry.
4. Run `cargo test -p superstate-agent --test test_run` and verify all tests pass.
5. Run full crate tests: `cargo test -p superstate-agent` and clippy: `cargo clippy -p superstate-agent --all-targets -- -D warnings`.
6. Commit: `git add crates/superstate-agent/src/run.rs crates/superstate-agent/tests/test_run.rs && git commit --no-gpg-sign -m "feat(agent): record tool calls and rollbacks into state.runtime.actions"`
7. Run `roborev status; roborev show HEAD`.
8. Write completion report to `docs/superpowers/plans/task-3-report.md`.
