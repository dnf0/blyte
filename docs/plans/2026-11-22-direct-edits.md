# Direct edits for small changes: an experiment

## Why

On the surface the conversation model hands every request to one implementer run. For a
three-task request that takes 22 to 25 minutes (`docs/evidence.md`, "One run by default"). A
one-line change pays most of that too: writing a task, a worktree, the run's reading turns, the
check, the fresh review, combining. This experiment lets the conversation model make a small
change itself and measures whether that is faster and cheaper without losing the check.

## Decisions

1. **The check still decides.** A direct edit is made in its own worktree `.wt/<id>` on `wip/<id>`,
   never in the user's checkout. It becomes a finished run only when the repository's gate has
   passed on it, through a `blyte` subcommand that runs the gate itself. From there it goes down
   the same path as an implementer run: flags, review queue, fresh review, combine, and the two
   routes that push and open a pull request. The model still cannot push.
2. **A direct edit is recorded as a run.** It gets the same files as a dispatched run
   (`.wt/<id>.start.json`, `.wt/<id>.json`, `.wt/<id>.task.md`, `.wt/<id>.checks.json`), with
   `"kind": "edit"` in the start file, so the surface, the review queue and combine need no change.
3. **Off unless the process says so.** The default process (`assets/orchestrator/process.md`) is
   not changed here. The subcommands exist and the `blyte` tool allows them; the trial supplies a
   process that tells the model when to use them. The default changes only after the measurement.
4. **Not in this experiment:** stopping the model's `write`/`edit` tools and sandboxed shell from
   writing to the main checkout, `.git` or `.wt/` directly. That gap exists today, with or without
   direct edits (the sandbox allows writes anywhere in the repository). It is the next piece of
   work if direct edits are kept.

## Measurement (done by the planner, not a task)

Two small requests against the same base, three trials each, with the process that allows direct
edits and with the default process: minutes from message to a combined branch, total USD,
conversation-model USD, and whether the result is right (read, not only checked).

### Task E1

**Goal.** Add `blyte edit-start` and `blyte edit-finish`, and let the conversation model's `blyte`
tool call them, so a change the conversation model makes itself is gated and recorded exactly as
a dispatched run is.

**Files.**
- New `crates/blyte-cli/src/direct_edit.rs`, registered in `lib.rs`.
- `crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/main.rs` (the two subcommands).
- `crates/blyte-agent/src/tools.rs` (the `blyte` tool: allow them, inject the request, time limit).
- `crates/blyte-cli/assets/orchestrator/reference.md` (document both; one short section).
- New `crates/blyte-cli/tests/test_direct_edit.rs`; a test beside the existing `blyte` tool tests
  in `crates/blyte-agent/tests/` for the tool changes.
- Do **not** change `assets/orchestrator/process.md`, `combine.rs`, `requests.rs`, `serve.rs` or
  `assets/surface/index.html`.

**What to build.**

1. `blyte edit-start <id> --task <text> [--base <ref>]`
   - `<id>` is validated exactly as `blyte dispatch` validates a run id (reuse that function).
     Refuse, naming what exists, if `.wt/<id>`, `.wt/<id>.start.json`, `.wt/<id>.edit.json` or
     the branch `wip/<id>` already exists.
   - `--task` is required and must not be empty after trimming: the user's request in a sentence
     or two. Written to `.wt/<id>.task.md`.
   - The base is resolved exactly as `blyte dispatch` resolves it (same default when `--base` is
     absent, same error when it does not resolve). Reuse dispatch's code; do not write a second
     rule.
   - Creates the worktree `.wt/<id>` on new branch `wip/<id>` at the base commit, and runs
     `[dispatch].setup` in it the way dispatch does (`setup::run_setup`). A setup failure removes
     the worktree and branch and is an error.
   - Writes `.wt/<id>.edit.json`: `id`, `base`, `base_sha`, `started_at`.
   - Prints one JSON object: `id`, `path` (absolute path of the worktree), `base_sha`. Exit 0.
2. `blyte edit-finish <id> [--request <request>]`
   - Requires `.wt/<id>.edit.json` (missing: error naming it). `--request`, when given, is
     validated with `dispatch::validate_request_id`.
   - Errors, each naming the cause, and writes nothing: the worktree has uncommitted or untracked
     changes; `wip/<id>` has no commit beyond `base_sha`.
   - Runs the same gate a dispatched run gets: the comment step against `base_sha` followed by
     `[dispatch].gate`, with `[dispatch].env`, in the worktree (`dispatch::gate_with_comment_step`,
     `run_gate_steps`). If the tip's tree has already passed this gate
     (`passed_trees::has_passed` with the same key dispatch uses), the gate is not run again.
   - **Gate fails:** print one JSON object `{ "id", "outcome": "gate_failed", "failing_step",
     "output_tail" }`, exit 1, and write no start file and no summary. The model can fix, commit
     and call `edit-finish` again: that is the resume path.
   - **Gate passes:** record the tree as passed (`passed_trees::record_passed`); write
     `.wt/<id>.start.json` with `dispatch::write_start_record` (same fields a dispatch writes,
     with `request` when given, plus `"kind": "edit"`; add that field to the record as an optional
     one that dispatch leaves out); write the summary `.wt/<id>.json` with the fields a dispatched
     run's summary has: `outcome` `verified`, `exit` 0, `branch`, `commits`, `changed_files`,
     `turns` 0, `cost_usd` 0.0, `seconds` (the time the gate took), `final_message` (the task
     text); then call `queue_actions::evaluate_verified_run` for it, as a finished dispatch does.
     Print the summary JSON, exit 0.
   - Calling `edit-finish` again after it has passed is an error naming `.wt/<id>.json`.
   - The summary is written last: a start file without a summary must never be left behind by a
     pass (write the summary and start file so that a crash between them leaves a state the next
     `edit-finish` call completes, and test it).
3. The `blyte` tool in `crates/blyte-agent/src/tools.rs`:
   - `BLYTE_TOOL_COMMANDS` gains `edit-start` and `edit-finish`.
   - `--request <BLYTE_REQUEST>` is injected into `edit-finish` the same way it is injected into
     `dispatch` (same code path, same behaviour when no request is set).
   - `edit-finish` runs the gate, which takes longer than the tool's 300-second limit on a
     foreground `blyte` call. Give `edit-finish` alone a limit of 3600 seconds (a named constant
     with a comment saying why). Every other command keeps 300.
4. `reference.md`: a short section, in the file's existing style, that documents both commands,
   what each prints, and that edits go in the worktree path `edit-start` prints, are committed
   there, and that `edit-finish` runs the check. It does not say when to edit directly: that
   belongs to the process.

**Tests** (`tests/test_direct_edit.rs`, real temporary git repositories with a `blyte.toml`
whose gate is a shell one-liner, as the existing dispatch and combine tests do).
- `edit-start` creates the worktree, branch, task file and edit file, and prints the JSON;
  refuses an existing id (each of the four cases); refuses an empty `--task`; errors on a base
  that does not resolve; a failing setup leaves no worktree and no branch.
- `edit-finish`: missing edit file; dirty worktree; no commits; each an error that names the
  cause and leaves no start file or summary.
- gate fails (`gate = "false"`-style): exit 1, JSON with `gate_failed` and the failing step, no
  start file, no summary; then, after a fixing commit and a passing gate, a second `edit-finish`
  succeeds.
- gate passes: start file has `kind` `edit`, the request when given, `base_sha`; summary has
  `outcome` `verified`, the commit and the changed file; a checks file exists; the tree is
  recorded as passed; a second `edit-finish` is an error.
- the gate is not run when the tree has already passed (use a gate that appends to a counter
  file outside the worktree).
- end to end with the existing code: after a passing `edit-finish --request R`,
  `requests::request_readiness(root, R)` is `Ready` (or waiting only for a flag) and
  `combine::combine_in(root, R, ..)` produces a `combined` record whose branch contains the
  commit and whose `check` is `already_passed`.
- tool tests: `edit-start` and `edit-finish` are accepted; another command is still refused;
  `edit-finish` gets `--request` injected when a request is set and not otherwise; the time
  limit chosen for `edit-finish` is 3600 and for `dispatch` 300 (test the function that picks it).

Run: `cargo test -p blyte --test test_direct_edit` and the `blyte-agent` test file you add to.

**Constraints.**
- **No fallbacks**: a missing or unreadable input is an error that names it; never a default, an
  empty result or a skipped step.
- **Resumable**: everything is in `.wt/` files; a failed or interrupted `edit-finish` can be run
  again.
- Neither command pushes, opens a pull request, merges, or touches the user's checked-out branch.
- Existing tests pass unmodified, except where the start record's new optional field needs a
  struct literal updated.
- Keep existing comments; do not delete comments from code you do not change.
- Depends on: none. Siblings: none.

**Commit message.**
`feat(surface): edit-start and edit-finish, a gated path for a change the conversation model makes itself`
