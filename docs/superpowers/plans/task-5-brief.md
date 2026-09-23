# Task 5: End-to-End CLI Verification and Invariant Checks

Working Directory: `/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness`

## What to Implement

1. **Create Integration Test File**:
   `crates/superstate-cli/tests/test_action_ledger_e2e.rs`:
   - Test 1: `test_repl_state_command_displays_action_ledger`:
     - Construct a `ReplSession` with initial state.
     - Add several `ActionEntry` items to `session.state.runtime.actions` (e.g. read, bash, note).
     - Call `handle_repl_command(&mut session, "/state", &mut output)`.
     - Verify that the output contains `"actions": [` with the recorded action entries (turn, tool, target, summary, is_error).
   - Test 2: `test_repl_session_records_actions_across_turns`:
     - Test REPL prompt execution or multi-turn simulation ensuring `state.runtime.actions` accumulates actions across turns.
   - Test 3: `test_state_only_invariant_preservation_e2e`:
     - Verify that for a session configured with `StateOnly`, prompt context contains `## Actions Taken So Far` while the total message vector length remains bounded (`<= 3`).

2. **REPL State Command Enhancement (if desirable)**:
   In `crates/superstate-cli/src/repl.rs`:
   - Ensure `/state` output clearly shows the actions count or formatted action summary if desired, or verify that the JSON serialization of `Runtime` includes `"actions"`.

3. **Workspace-Wide Verification**:
   - `cargo check --workspace --all-targets`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test --workspace`

4. **Commit & Roborev Review**:
   - `git add crates/superstate-cli/tests/test_action_ledger_e2e.rs crates/superstate-cli/src/repl.rs`
   - `git commit --no-gpg-sign -m "test(cli): add end-to-end verification for action ledger in REPL"`
   - `roborev status; roborev show HEAD`
   - Write report to `docs/superpowers/plans/task-5-report.md`.
