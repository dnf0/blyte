# Plan: the orchestrator dispatches early

In the 2026-10-15 Argon benchmark, `blyte --orchestrate` spent $0.70–3.00 of its own on each task,
mostly reading the repository before dispatching once. Its implementers read the same files again.
Decision (user, 2026-10-15): a prompt change plus a one-time reminder, with no hard limit on reading.

### Task OD1

**Goal:** the orchestrator gets oriented in a few calls, leaves deep reading to the implementers, and
splits work into focused dispatches. If it is still exploring after many calls without dispatching,
it gets one reminder.

**Where:**
- `crates/blyte-agent/src/orchestrator.rs`:
  - `PREAMBLE` (line 10), the orchestrator's "How you work" text;
  - `DISPATCH_TOOL_INSTRUCTION` (line 81), added under "## Delegating work" when the run has a
    `dispatch` tool (`--dispatch-cmd`);
  - `prompt_sections_with_dispatch` (line 92).
- `crates/blyte-agent/src/run.rs`, `Runner::run` (line 602):
  - The loop already adds one-time user messages before building the request (the "10 turns
    left" message, around lines 686-691, and the research reminder after it). Add the new
    reminder in the same place, so it follows the previous turn's tool results.
  - Tool calls are recorded around lines 806-880.
  - A run is an orchestrator run when `state.orchestrator.instructions.is_some()`.
- Tests: `crates/blyte-agent/tests/test_orchestrator_prompt.rs`. It has a `RecordingProvider` that
  records each `Request`; extend it, or add a scripted provider in the same style, that returns
  set tool calls turn by turn.

**Change:**
1. **Prompt.** Add a paragraph to `PREAMBLE`, after the paragraph that starts "Start on the work at
   once", with this meaning:
   - Get oriented with a few cheap calls (list the files, grep for the names in the request, read
     the one or two files the work centres on), then dispatch.
   - The implementer reads the code it changes and writes its own tests, so don't read files on
     its behalf, and don't read the whole repository to plan.
   - Split work with separable parts into several focused dispatches, each naming its files and how
     to check it, rather than one large one.
   - Further reading is for reviewing a dispatch's result, not for preparing it.

   Extend `DISPATCH_TOOL_INSTRUCTION` with: make the first dispatch early; each task names the
   files to change and the check that shows it worked; when the report shows the gate failed or
   work is missing, send a focused follow-up dispatch rather than doing it yourself.

   Keep the wording plain and short: about 6 sentences in total, no lists of numbers.
2. **Reminder.** In `Runner::run`, for orchestrator runs only:
   - Count the tool calls made before the first dispatch. A dispatch is a call to the `dispatch`
     tool, or a `bash` call whose `command` contains `blyte dispatch`. Count every other call:
     `read`, `grep`, `glob`, and any other `bash` command.
   - When the count reaches `ORCHESTRATOR_EXPLORE_NUDGE = 8` and there has been no dispatch, push
     one `Message::User` before the next request: "You have made 8 tool calls without dispatching.
     Dispatch the work now from what you know; the implementer reads the code it changes. Keep
     reading only if you cannot yet say which files the task is about."
   - It is sent at most once per run, and never once a dispatch has happened.
   - Define `ORCHESTRATOR_EXPLORE_NUDGE` as a `pub const` next to the other run constants, with a
     one-line comment giving the reason (the benchmark above).
   - Non-orchestrator runs and research runs are unchanged.

**Tests:**
- `prompt_sections_with_dispatch(..., true)` and `(..., false)` both contain the new orientation
  paragraph. Assert one or two distinctive phrases, not the whole text. Only the `true` form
  contains the extended dispatch instruction.
- In an orchestrator run (state with `instructions: Some(..)`) where the provider returns a `read`
  call on each of 9 turns and then a final answer, the request after the 8th tool call ends with
  the reminder, and it appears exactly once in the final history.
- Same run, but turn 3 is a `bash` call with `blyte dispatch X --plan p.md --plan-task "Task X"`
  (or a `dispatch` tool call): the reminder never appears.
- A non-orchestrator run with 9 `read` calls never gets the reminder.
- Use the existing test helpers in `crates/blyte-agent/tests/`. Tool calls on a scratch temporary
  directory are fine; no network.
