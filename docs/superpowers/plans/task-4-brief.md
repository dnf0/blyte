# Task 4: Wire Context Assembly in StateOnly::build

Working Directory: `/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness`

## What to Implement

In `crates/superstate-agent/src/context.rs`:
Update `StateOnly::build` (lines 53-81) to format `state.runtime.actions` into the first user message when actions are present:

1. **Format Action Ledger**:
   - If `state.runtime.actions.is_empty()`, do NOT add the `## Actions Taken So Far` section. The user prompt remains:
     ```markdown
     ## Task
     {goal}

     ## Your state
     {json_agent_fields}
     ```
   - If `!state.runtime.actions.is_empty()`, append:
     ```markdown

     ## Actions Taken So Far
     - Turn {turn}: `{tool}` `{target}` -> {summary}
     ```
     For each `action` in `state.runtime.actions`.

2. **Invariant II Preservation**:
   - `out` must continue to have `messages.len() <= 3` (one `Message::User`, plus at most one previous `Message::ToolCall` and `Message::ToolResult`, or just the single user message on initial turn).

---

## Testing Requirements (TDD)

In `crates/superstate-agent/tests/test_run.rs` (or a dedicated test):
1. `test_state_only_context_omits_actions_when_empty`:
   - Initialize `state` with no actions.
   - Call `StateOnly.build(&state, &history)`.
   - Verify `Message::User` string does NOT contain `"## Actions Taken So Far"`.
2. `test_state_only_context_formats_action_ledger`:
   - Initialize `state`, push `ActionEntry` elements to `state.runtime.actions`.
   - Call `StateOnly.build(&state, &history)`.
   - Verify `Message::User` string contains `"## Actions Taken So Far"`.
   - Verify each action is formatted as `- Turn {turn}: \`{tool}\` \`{target}\` -> {summary}`.
   - Verify `messages.len() <= 3`.
3. `test_state_only_runner_injects_action_ledger_in_prompts`:
   - Run a `Runner` with `StateOnly` context strategy and a multi-step `ScriptedProvider`.
   - Inspect `p.seen` after turn 1.
   - Verify that in turn 2's request, the initial `Message::User` contains the action taken in turn 1.

---

## Execution Steps
1. Write the tests in `crates/superstate-agent/tests/test_run.rs`.
2. Run `cargo test -p superstate-agent --test test_run` to observe failures (RED phase).
3. Implement action ledger formatting in `crates/superstate-agent/src/context.rs` (GREEN phase).
4. Run `cargo test -p superstate-agent --test test_run` and verify all tests pass.
5. Verify `cargo check -p superstate-agent` and `cargo clippy -p superstate-agent --all-targets -- -D warnings`.
6. Commit: `git add crates/superstate-agent/src/context.rs crates/superstate-agent/tests/test_run.rs && git commit --no-gpg-sign -m "feat(agent): format action ledger into StateOnly prompt context"`
7. Run `roborev status; roborev show HEAD`.
8. Write completion report to `docs/superpowers/plans/task-4-report.md`.
