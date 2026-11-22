# Results you can trust: a quieter fallback flag, and a conversation model that cannot write around the check

## Why

Five trial runs of one request were read in full on 2026-11-22. Every run passed the check and
still had real defects. Two findings follow from that:

- **The fallback flag is noise.** "Possible fallbacks added" fired on all five runs: 46 lines, of
  which 3 were fallbacks. It matches `unwrap_or` and similar by text, so moved code, path
  handling and plain defaults all count. A flag that is wrong 43 times in 46 teaches the user to
  accept without reading. The fresh review, a model reading the same change, was right on 15 of
  20 concerns; one of five reviews returned nothing because its reply was cut off.
- **The conversation model can write anywhere in the repository.** Its `write` and `edit` tools,
  and its sandboxed shell, can write the user's checkout, `.git` and `.wt/`, so it could create a
  request's branch or a "this passed" record by hand. The rule that nothing reaches a request's
  branch without the check holds only because the model follows its instructions.

## Decisions

1. A line matched by the fallback pattern is a candidate, not a flag. The fresh review judges
   each candidate; the run is flagged only for those it confirms, or when it could not judge.
2. Nothing is dropped silently: a candidate the review did not judge, or a review that failed,
   flags the run and says so.
3. The conversation model writes only in `.wt/tasks/` (its task and question files) and temporary
   directories. Any other write needs the user's approval, the same way a shell command that
   leaves the sandbox does. This applies wherever the conversation model runs (`--orchestrate` and
   the surface), not to implementer runs.

## Checked against real data (done by the planner, not a task)

After F1: run the new judging over the five trial changes and compare with the read-through
(3 real fallbacks of 46 candidates). After W1: on the surface, ask the model to write a file in
the checkout, in `.git` and in `.wt/`, and confirm each is refused or asks.

### Task F1

**Goal.** The fresh model review judges each fallback candidate, the run's fallback flag reports
only what it confirmed or could not judge, and a review whose reply is cut off no longer returns
nothing.

**Files.**
- `crates/blyte-cli/src/fallback_check.rs`, `crates/blyte-cli/src/model_review.rs`,
  `crates/blyte-cli/src/review_checks.rs`, `crates/blyte-cli/src/queue_actions.rs`.
- `crates/blyte-cli/src/review.rs` or wherever `blyte review` prints flags and concerns, only to
  print the new fields.
- Tests beside the existing tests for these modules in `crates/blyte-cli/tests/`.
- Do not change `assets/surface/index.html`, `crates/blyte-agent/`, `sandbox.rs` or `tools.rs`.

**What to build.**

1. **Moved lines are not candidates** (`fallback_check::added_fallbacks`). A matched added line
   whose text, trimmed, is equal to the trimmed text of a removed line anywhere in the same diff
   is dropped: the code was moved, not added. Each removed line cancels at most one added line.
   `Finding` gains `text: String` (the added line, trimmed, at most 200 characters).
2. **The review judges the candidates** (`model_review`). `build_request` takes the candidates
   and, when there are any, adds a section listing each as `file:line`, the pattern and the line's
   text, with this definition: a fallback is code that substitutes a default, a guess, an empty
   result or a silent skip when something is missing, unknown or unreadable, instead of failing
   with an error that names it; a documented, intended default is not a fallback; neither is
   string or path handling that cannot hide a missing input. The reply becomes one JSON object:
   `{"concerns": [...as today...], "fallbacks": [{"file", "line", "verdict": "fallback" |
   "not_fallback", "reason": "<one sentence>"}]}`, one entry for every candidate listed. `parse`
   accepts that object; a bare array (the old format) is a parse error. A verdict other than the
   two values is a parse error.
3. **The saved review** (`.wt/<id>.model-review.json`) gains `fallbacks` (the verdicts; absent in
   files written before this change, which must still load).
4. **The flag** (`review_checks` / `queue_actions`, wherever "possible fallbacks added" is built):
   - no candidates: no fallback flag and, as today, candidates alone are a reason to run the
     review (keep `should_run_model_review` true when there are candidates);
   - the review succeeded: flag `fallbacks added: N (file:line, file:line, …)` listing only the
     candidates judged `fallback`; candidates with no entry in the reply are listed in a second
     flag `fallback candidates not judged: N (file:line, …)`; when every candidate was judged
     `not_fallback`, there is no fallback flag;
   - the review failed or was not run: flag `possible fallbacks added: N (not judged: <why>)`,
     where `<why>` is the review's error or `no review`.
   Flags stay one line of at most 200 characters, as `review_checks::build` already requires:
   when the list does not fit, cut it and end with `, … and K more`.
   `.wt/<id>.checks.json` keeps every candidate in `fallbacks`, each now with its `text`, and with
   `verdict` and `reason` when judged, so the record shows what was dismissed and why.
5. **A cut-off reply.** Today `max_output_tokens` is 32,000 and one review ended with finish
   reason `MAX_TOKENS` and an empty reply, having spent its budget before writing the answer.
   - Look at `blyte_provider::Request` and the provider for the fields that bound the model's
     thinking separately from its answer. If there is one, set it so the thinking cannot use more
     than half of `max_output_tokens`.
   - When the reply's finish reason is `MAX_TOKENS` and the text is empty or does not parse, make
     exactly one more attempt with the thinking bound at its lowest setting (or, if the provider
     has no such field, with `max_output_tokens` doubled). The cost of both attempts is summed
     into the saved `cost_usd`. If the second attempt is also cut off or does not parse, the
     review fails with the existing error text plus `(after one retry)`. No further retries.
   - State in the report which fields exist and which of the two paths you built.
6. `blyte review <id>` prints the confirmed fallbacks with their reasons, and the count of
   candidates dismissed.

**Tests.**
- moved line: a diff that removes `let x = a.unwrap_or(b);` from one place and adds the same
  line elsewhere yields no finding; adding it twice while removing it once yields one finding;
  a genuinely new matched line yields a finding with its `text`.
- `build_request` lists each candidate with file, line and text, and has no such section when
  there are none.
- `parse`: the object with both arrays; missing `fallbacks` key when candidates were sent is an
  error; a bare array is an error; an unknown verdict is an error.
- the flag, with a fake provider (the existing tests for `run_with_provider` show how): all
  judged `not_fallback` → no fallback flag and the checks file records each verdict and reason;
  one judged `fallback` → `fallbacks added: 1 (file:line)`; one candidate missing from the
  reply → the not-judged flag names it; review fails → `possible fallbacks added: N (not judged:
  …)`; a long list is cut to 200 characters with `… and K more`.
- cut-off: a fake provider whose first reply is empty with finish reason `MAX_TOKENS` and whose
  second is valid → one retry, concerns returned, cost is the sum; both cut off → failed with
  `(after one retry)`; a first reply that is valid → no retry.
- an old `.model-review.json` without `fallbacks` still loads.

Run the test files you add to, then the repository's check.

**Constraints.**
- **No fallbacks**: an unreadable file or a reply that does not parse is an error that names it;
  never an empty verdict list treated as "nothing found".
- **Resumable**: the review's result is saved as today; nothing new is held only in memory.
- Keep existing comments; do not delete comments from code you do not change.
- Depends on: none. Siblings: W1 restricts the conversation model's writes in `crates/blyte-agent`
  and is **not** in this worktree; do not touch those files.

**Commit message.**
`fix(review): fallback candidates are judged by the fresh review before they flag a run; a cut-off review retries once`

### Task W1

**Goal.** In orchestrate mode (the conversation model, in the terminal and on the surface), the
`write` and `edit` tools and the sandboxed shell can write only in `<root>/.wt/tasks/` and
temporary directories. A write anywhere else needs the user's approval.

**Background to read first.** `crates/blyte-agent/src/tools.rs` (`write`, `edit`,
`resolve_in_workspace`, `set_approval`, how the shell asks for approval to leave the sandbox),
`crates/blyte-agent/src/approval.rs`, `crates/blyte-agent/src/sandbox.rs` (`default_allowed` and
how the macOS and Linux profiles are built from the allowed list),
`crates/blyte-cli/src/task_runner.rs` (`orchestrator_extra_writes`, and where the approver is
chosen: near the `set_approval` call it picks `TtyApprover` whenever there is a conversation),
`crates/blyte-cli/src/repl.rs`, and how the surface answers approvals (search for the surface's
approver in `crates/blyte-cli/src/`).

**Files.** Those above, `crates/blyte-cli/assets/orchestrator/process.md` and `reference.md`,
`crates/blyte-cli/src/orchestrator_text.rs` (the sandbox paragraph), and tests in
`crates/blyte-agent/tests/` and `crates/blyte-cli/tests/`. Do not change `model_review.rs`,
`fallback_check.rs`, `review_checks.rs` or `queue_actions.rs`.

**What to build.**

1. **A write policy for the tools.** A process-wide setting beside `set_approval`:
   `set_write_scope(Option<WriteScope>)`, where `WriteScope { allowed: Vec<PathBuf> }` holds
   absolute, canonical directories. `None` (every implementer run, and orchestrate mode with the
   sandbox off) behaves exactly as today.
2. **`write` and `edit` with a scope set.** After `resolve_in_workspace`, resolve the real target:
   canonicalize the deepest existing ancestor of the path and append the rest (the file may not
   exist yet). If the result is inside one of `allowed`, write as today. Otherwise ask the
   configured approver (`Approver::ask`) with the command text `write <path>` or `edit <path>`
   and the reason `outside the directories this conversation may write (<allowed list>)`:
   approved → write; refused, or no approver configured → an error result saying the write was
   not made, where it may write, and that the user can approve it. A symbolic link inside an
   allowed directory that points outside it is outside (the canonical path decides). An ancestor
   that cannot be canonicalized is an error naming it, not an allowed write.
3. **The sandboxed shell in orchestrate mode.** Its writable directories become: `.wt/tasks`
   under the main checkout, the temporary directories and the developer caches that
   `default_allowed` already lists. Not the repository root, not the git directory or git common
   directory, not the rest of `.wt/`. Build this as its own function beside `default_allowed`
   (for example `orchestrator_allowed(root)`), leave `default_allowed` and implementer runs
   unchanged, and replace `orchestrator_extra_writes`. Reads are unchanged. `.wt/tasks` is
   created (with its parents) when the conversation starts, before the shell is spawned, because
   the Linux profile can only allow paths that exist.
4. **Set the scope** to the same list where orchestrate mode sets up approval, only when the
   sandbox is on. With the sandbox off, no scope is set (and the start line already says the
   sandbox is off).
5. **The approver on the surface.** Find why `task_runner.rs` picks `TtyApprover` whenever a
   conversation exists, and whether that replaces the surface's own approver when the
   conversation runs on the surface. If it does, the surface's approver must be the one used
   there, so an approval request appears on the surface and not on a terminal nobody is watching.
   Add a test that fails without the fix. If it does not, say so in the report with the lines
   that show it, and change nothing.
6. **The `blyte` tool is unchanged**: `dispatch` and the other allowed commands still run as
   today and write their own files in `.wt/`.
7. **Instructions.** `process.md` and `reference.md`: task and question files go in
   `.wt/tasks/<name>.md`; one sentence that writes elsewhere need the user's approval. The
   sandbox paragraph in `orchestrator_text.rs` says the shell and the write tools can write only
   in `.wt/tasks` and temporary directories. Update the tests that pin these texts.

**Tests.**
- tools, with a scope of one temporary directory `A` inside a temporary root and a recording
  approver: `write` and `edit` inside `A` succeed without asking; `write` to `<root>/src/x.rs`,
  to `<root>/.git/refs/heads/x`, to `<root>/.wt/passed/x.json` and to
  `<root>/.wt/combined/r.json` each ask once and, refused, write nothing and return the error;
  approved, the write happens; with no approver, refused with the error; a symbolic link
  `A/link -> <root>/.git` makes `write A/link/config` ask; `..` segments cannot escape
  (`A/../src/x.rs` asks); a new file in a new subdirectory of `A` is allowed; with no scope set,
  every one of these writes as today without asking.
- `orchestrator_allowed(root)` contains `.wt/tasks` under the main checkout and the temporary
  directories, and contains neither `root`, the git common directory nor `.wt` itself; for a
  linked worktree it still names the **main** checkout's `.wt/tasks`.
- on macOS and Linux (follow the existing sandbox tests for how they are gated per platform): a
  shell spawned with the orchestrate profile can create a file in `.wt/tasks`, and fails to
  create one in the root, in `.git` and in `.wt/passed`.
- orchestrate mode with the sandbox on sets the scope; with it off, sets none; an implementer
  run sets none.
- the approver test from item 5.

Run the test files you add to, then the repository's check.

**Constraints.**
- **No fallbacks**: a path that cannot be resolved is an error that names it, never treated as
  allowed and never silently skipped. No environment variable or setting that turns the scope
  off other than the existing sandbox setting.
- **Resumable**: not a long job; nothing to resume.
- Implementer runs, research runs and their sandbox are unchanged; their tests pass unmodified.
- Keep existing comments; do not delete comments from code you do not change.
- Depends on: none. Siblings: F1 changes the review and fallback flag in `crates/blyte-cli` and
  is **not** in this worktree; do not touch those files.

**Commit message.**
`fix(sandbox): the conversation model writes only in .wt/tasks; any other write needs the user's approval`

## After the check against the trial changes (2026-11-22)

Decision 1 of Task F1 ("moved lines are not candidates") was withdrawn. The three fallbacks the
read-through called real were all one existing line, re-indented inside changed code, so the rule
dropped exactly the candidates that mattered. Moved lines are candidates again and the fresh
review judges them.
