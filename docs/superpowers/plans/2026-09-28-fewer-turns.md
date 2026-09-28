# Fewer Turns on Gemini — Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Why (2026-09-28):** every turn re-sends the whole context (about 58K tokens by the end of an
offload dispatch), cached or not, so turns are the cost unit. In offload v2 run B2 (superstate-eval
`results/offload-v2-sonnet-20260928-1028/ab-2`, the first run with dispatch event logs), blyte on
Gemini 3.8 Flash took 243 turns over 5 dispatches:

- **218 turns made exactly one tool call**; 6 made more (across 5,571 earlier Gemini turns: 5,565
  of them one call). The system prompt already asks for independent calls in one turn; Gemini
  mostly ignores it.
- **20 streaks of consecutive read/grep/glob turns covered 112 turns**: batched, up to 92 fewer
  turns (38%; an upper bound, since some reads depend on the previous result).
- **38 of 88 reads re-read a file already read in the same dispatch.**
- **5 dispatches** where run B1 used 3. Each dispatch starts a fresh conversation and re-reads the
  code it touches. The eval's offload prompt did not say what blyte's skill says ("do not split work
  artificially"); that prompt is fixed separately in superstate-eval.

Tool-level changes were first left out; after review (2026-09-28) Task 2 adds a multi-path `read`, the only
model-independent way to cut these turns. An "unchanged since turn N" reply stays out.

## Global Constraints

- Work only in your own worktree. Existing tests pass **unmodified**.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: show the model how to batch

**Files:** `crates/blyte-agent/src/run.rs` (`system_prompt`), `crates/blyte-agent/src/tools.rs`
(`specs`, the `read` description), `crates/blyte-agent/tests/test_batching_prompt.rs` (new).

1. In `system_prompt`, replace the `## Working efficiently` paragraph with exactly:

   ```text
   ## Working efficiently
   Every turn re-sends this whole conversation, so turns are what a run costs. When you need
   several things that do not depend on each other, ask for all of them at once, as several tool
   calls in one turn: for example, to look at three files, call `read` three times in the same
   response, not once per response. The calls run in order and you see every result together. Only
   wait for a result when the next call depends on it.
   ```

   (as one Rust string continuing the existing `format!`, with a `\n` after the heading and at the
   end, and no other line breaks, matching how the surrounding sections are written; no other text
   in the prompt changes). The existing `tests/test_parallel_calls.rs` asserts the prompt contains
   `## Working efficiently` and `several tool calls in one turn`; the new text keeps both.
2. Append to the `read` tool description: ` To read several files, call read once per file in the
   same response.` (one leading space).
3. **Test** (`crates/blyte-agent/tests/test_batching_prompt.rs`): a `Runner` with a stub provider
   that captures the first `Request` (copy the `Stopper` pattern from
   `tests/test_gate_slots_nesting.rs`); assert its `system` contains `call \`read\` three times in
   the same response` and `turns are what a run costs`, and that the `read` spec's description ends
   with `call read once per file in the same response.`

- [ ] Steps: test first (fails), implement, full checks, commit:

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(agent): the prompt shows the model how to batch independent calls in one turn"
```

## Measure (after it lands; needs the user's go-ahead: about $8)

Two more B runs of offload v2 (same task, same `plan.md`, `--plan-from`, Sonnet orchestrator), with Tasks 1 and 2,
the eval's offload prompt fix and this change, against B1 and B2:

- turns per dispatch and the share of turns with more than one call (target: well above B2's 2.5%);
- dispatches per run (target: 1–3, not 5);
- Gemini cost and wall time; target tests passed (must stay 72/72).

Keep the prompt change only if multi-call turns rise clearly and Gemini cost falls beyond the B1–B2
range with no quality loss; otherwise revert it and record the result.

---

### Task 2: `read` takes several paths (added 2026-09-28)

**Why:** the prompt can only ask; a model that makes one call per turn still makes one call per
turn. A `read` that takes a list gets several files into one turn whatever the model does, which
is the only model-independent way to cut these turns. Reads were 88 of B2's 243 calls and most of
its read streaks. (`bash` and `grep` already cover several things per call.) **Starts from Task 1's
merged result** (both touch the `read` spec).

**Files:** `crates/blyte-agent/src/tools.rs` (`specs`, the `read` branch of `execute_unbounded`),
`crates/blyte-agent/src/run.rs` (`extract_action_target` and `extract_action_summary` for `read`,
and the event input for `read`), `crates/blyte-agent/src/run.rs` system prompt example,
`crates/blyte-agent/tests/test_read_paths.rs` (new).

**Behaviour:**
- `read` accepts `path` (as today, unchanged, with `start_line`/`end_line`) **or** `paths`, an array
  of 1–10 workspace-relative strings. Both or neither: an error saying to pass one of them. More than
  10: an error. `start_line`/`end_line` with `paths`: an error (they apply to one file).
- With `paths`, each file is read whole, numbered exactly as a single-file read, and preceded by a
  line `==> <path> <==`; files are separated by a blank line. A path that cannot be read (outside
  the workspace, missing, a directory) gives `==> <path> <==` followed by its error, and the other
  files are still returned; the call is an error only if every path failed.
- **Budget:** one call returns at most `READ_LINE_BUDGET` lines and `MAX_OUTPUT_CHARS` characters in
  total. Files are added whole, in the order given, while they fit; a file that would not fit is not
  cut but listed at the end: `not read (over this call's budget; read separately): <path>, <path>`.
  A single file larger than the whole budget behaves as a single-file read of it would (its first
  `READ_LINE_BUDGET` lines, with the existing note), and only when it is the first file.
- Schema: `"path"` and `"paths"` (`{"type":"array","items":{"type":"string"},"maxItems":10}`) both
  optional (`"required": []`); the check above happens at run time.
- The `read` description becomes exactly: `Read one file (path) or several at once (paths, up to 10).
  Up to 2,000 lines (60,000 characters) come back in one call, so read whole files; pass
  start_line/end_line only for one larger file. To read several files, pass them together as paths,
  or call read once per file in the same response.` (It keeps Task 1's ending, which its test checks.)
- In the system prompt's `## Working efficiently` paragraph, the example becomes: `for example, to
  look at three files, pass all three to one read call as paths (or call \`read\` three times in the
  same response), not one per response.` (keeping `call \`read\` three times in the same response`,
  which Task 1's test checks).
- Event log and digests: a `read` with `paths` shows as `a.rs, b.rs, c.rs` (at most three names,
  then `(+N)`), so dispatch event logs and folded digests stay readable.

- [ ] **Step 1: tests** (`crates/blyte-agent/tests/test_read_paths.rs`, on a temp directory with
  `LocalFs`/`LocalShell` and `tools::execute`): two files come back with headers, numbering and a
  blank line between; a missing file among good ones is reported inline and the call is not an
  error; all missing is an error; `path` and `paths` together, neither, 11 paths, and `paths` with
  `start_line` are errors; with three files whose total exceeds `MAX_OUTPUT_CHARS`, the third is
  listed as not read and the first two are whole; a single-path `read` is byte-identical to before
  (compare with `path`); `extract_action_target` for a four-path read is `a, b, c (+1)`.
- [ ] Steps 2–4: fail, implement, full checks. **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(agent): read takes several paths, so one call can bring several files into one turn"
```

## Result (2026-09-28): both tasks reverted

In the Vertex A/B above (four real blyte dispatches on Gemini 3.8 Flash with tasks 1 and 2 merged),
Gemini made exactly one tool call on every turn that made any (0 multi-call turns) and never used
`read` with `paths` (0 of 68 reads). Neither change affected Gemini, so by this plan's rule both
were reverted (#134, #136). The planned offload B runs were cancelled after this. Multi-path `read`
might help models that do batch; it is in #136 if a measurement on another model is wanted. The
eval's offload prompt change (one dispatch per coupled change, superstate-eval #75) stays: it
matches blyte's own skill.
