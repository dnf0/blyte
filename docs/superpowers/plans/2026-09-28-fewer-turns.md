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

Tool-level changes (a multi-path `read`, "unchanged since turn N" replies) are **not** in this
plan: they are the kind of tool tweak rejected before, and come back only if this does not cut turns.

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

Two more B runs of offload v2 (same task, same `plan.md`, `--plan-from`, Sonnet orchestrator), with
the eval's offload prompt fix and this change, against B1 and B2:

- turns per dispatch and the share of turns with more than one call (target: well above B2's 2.5%);
- dispatches per run (target: 1–3, not 5);
- Gemini cost and wall time; target tests passed (must stay 72/72).

Keep the prompt change only if multi-call turns rise clearly and Gemini cost falls beyond the B1–B2
range with no quality loss; otherwise revert it and record the result.
