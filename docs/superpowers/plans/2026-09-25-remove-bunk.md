# Remove What Did Not Earn Its Place Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** Delete everything that was measured not to pay, or that the user chose to drop on 2026-09-25. **blyte** becomes `blyte-agent` + `blyte-provider` + a thin `blyte` CLI (task mode and REPL).

**What goes:**
1. **Context strategies that lost** (superstate-eval 13.33–13.45): `StateOnly`, `StateWindow`, `Compact`, `Adaptive`. `Folded` (the default) and `Conversational` (the control) stay.
2. **Fold notes:** `Orchestrator.fold_notes`, the fold-note request, `render_fold_note`, `FOLD_NOTE_MARKER`, `Runtime.fold_notes`, the digest's note parameter, `--fold-notes`, and the `RunReport`/`RunRecord` fold-note counts.
3. **Cost cap:** `Budget.max_cost_usd` and `--max-cost-usd`. (`blyte_provider::pricing` stays: the run report's `cost_usd` uses it.)
4. **Journal and resume:** `Event::Checkpoint`, `Runner::resume`, the `resumed` parameter of `run_until`, `--journal`, `--resume`, `crates/blyte-cli/src/journal.rs`.
5. **The bolt-on era:**
   - the crates `blyte-core`, `blyte-mcp`, `blyte-worktree`, `blyte-review`, `blyte-rules`, `blyte-syntax`;
   - every CLI subcommand (`doctor`, `mcp`, `review`, `init`, `state`, `worktree`, `daemon`, `cancel`, `logs`, `handoff`, `shunt`, `inspect`, `guard`);
   - `crates/blyte-cli/src/commands/` and `state_cli.rs`.
   
   The one thing the harness uses from `blyte-core`, the autonomous-TDD phase wording (`tdd.rs`), moves into the CLI.

**What stays, unchanged in behaviour:**
- the agent loop, `folded`/`conversational`, `--fold-budget`;
- the loop-breaker, output bounding and provider retry with server hints;
- the verification gate, `--tdd`, `--review` and checkpoint commits;
- persistent sessions, native grep and glob, `pricing`, per-turn usage and the run report;
- the REPL and `harness_bench`.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/remove-bunk` (from `main`). Each task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back. Task 2 builds on Task 1.
- Do not run Docker. `Cargo.lock` is gitignored.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass at the end of each task.
- This is a deletion. **Tests of deleted features are deleted with them.** A test that only used a deleted strategy or field incidentally (for example, a runner test that happened to pass `&StateOnly`, or a `Budget` literal with `max_cost_usd`) is edited minimally to keep testing what it tested: use `&Conversational` for runner tests (or `Folded` where the test is about folding), and drop removed fields from literals. **List every test file you delete and every test you edit in your report.** Do not weaken an assertion about a feature that stays.
- Commit with `--no-gpg-sign` and the exact message in each task.

---

### Task 1: Remove the losing strategies, fold notes, cost cap, journal and resume

**Files:** `crates/blyte-agent/src/{context.rs,run.rs,state.rs}`, `crates/blyte-cli/src/{cli.rs,task_runner.rs,report.rs,repl.rs,main.rs,lib.rs}`, `crates/blyte-agent/examples/harness_bench.rs` (only if it references a removed item), and tests as described.

- [ ] **Step 1: `context.rs`.**
  - Delete `StateOnly`, `StateWindow`, `Compact` and `Adaptive`, with their impls, docs and helpers used only by them (`call_for_result`, `superseded`, `fetch_key`, `is_refetch`, and anything else that becomes unused).
  - Make `greedy_plan` take no growth parameter: `fn greedy_plan(rest: &[Message], budget: usize) -> FoldPlan`, with the growth branch removed.
  - Remove the `note: Option<&str>` parameter from `digest` and the note-rendering block; `render_folded` no longer reads `state.runtime.fold_notes`.
  - Keep the `folds` and `fold_budget` trait methods (used by `Folded` and the runner).
  - Remove the module docs' references to deleted strategies. Keep `Conversational`, `Folded`, `FoldPlan`, `digest`, `render_folded`, `approx_tokens`.
- [ ] **Step 2: `state.rs` and `run.rs`.**
  - Remove `Orchestrator.fold_notes` and `Runtime.fold_notes`.
  - Remove `Budget.max_cost_usd` (and from `impl Default for Budget`), the cost-cap check and the no-price error in `run_until`, and the `price` local.
  - Remove `FOLD_NOTE_MARKER`, `render_fold_note`, `Runner::fold_note`, the fold-note block in `run_until`, `last_sent` if it becomes unused except for `TurnUsage` (**keep** what `TurnUsage.appended`/`new_chars` need), and `RunReport.fold_notes`/`fold_note_failures`.
  - Remove `Event::Checkpoint`, both `emit(Event::Checkpoint { .. })` calls, `Runner::resume`, and the `resumed` parameter of `run_until` (update its two callers). The fold-notes `clear()` goes with the field.
  - Remove the `Checkpoint { .. } => {}` arm in `examples/solve.rs` and the `Checkpoint` arm in `crates/blyte-cli/src/repl.rs`'s `format_event`.
- [ ] **Step 3: CLI.**
  - Remove the flags `--fold-notes`, `--max-cost-usd`, `--journal` and `--resume`, and the matching `TaskConfig` fields and `From<&Cli>` lines.
  - In `task_runner.rs`: remove the resume block, the `Checkpoint` branch of the `on_event` closure, `state.orchestrator.fold_notes = ...`, and the `max_cost_usd` field of the `Budget` literal. `runner.run(state)` is the only run call again.
  - Restrict `--context` to `value_parser = ["conversational", "folded"]` (default `folded`), with its doc comment updated. Remove the construction of `state_only`, `state_window`, `compact` and `adaptive`, and their match arms (unknown values cannot reach the match; keep `_ => &conversational`).
  - In `report.rs`, remove `fold_notes` and `fold_note_failures` (keep `cost_usd`, `fold_budget` and `turn_usage`).
  - In `main.rs`, remove the resume-without-prompt condition: back to `if let Some(ref prompt) = cli.prompt { if prompt.trim().is_empty() {`.
  - Delete `crates/blyte-cli/src/journal.rs` and its `pub mod journal;`.
  - Remove the removed fields from the REPL's `TaskConfig` literal.
- [ ] **Step 4: Tests.**
  - **Delete:** `crates/blyte-agent/tests/{test_adaptive.rs,test_compact.rs,test_fold_notes.rs,test_state_window.rs}` and `crates/blyte-cli/tests/test_journal.rs`.
  - **Edit `test_resilience.rs`:** delete `a_run_stops_itself_at_its_cost_cap`, `a_cost_cap_without_a_known_price_is_an_error_not_ignored` and `a_run_resumes_from_its_last_checkpoint_without_rerunning_tools`. Keep the retry-hint and pricing tests, and remove the imports they no longer need.
  - **Other files:** in every other test, remove `max_cost_usd` from `Budget` literals and `fold_notes` from `Orchestrator`/`TaskConfig`/`RunReport` literals, and apply the constraint above to tests that use `StateOnly`/`StateWindow`. In `test_parallel_calls.rs`, delete `state_only_carries_the_whole_last_block` and `state_window_carries_whole_blocks`, whose subjects were deleted.
- [ ] **Step 5: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Then confirm nothing references the removed items:

```bash
git grep -n -E "StateOnly|StateWindow|Compact\b|Adaptive|fold_notes|FOLD_NOTE|render_fold_note|max_cost_usd|Event::Checkpoint|Runner::resume|--journal|--resume|--fold-notes|--max-cost-usd" -- crates/
```

  Expected: no output. Report the full list of deleted test files and edited tests.
- [ ] **Step 6: Commit.**

```bash
git add -A crates/
git commit --no-gpg-sign -m "refactor!: remove losing context strategies, fold notes, cost cap, journal and resume"
```

---

### Task 2: Remove the bolt-on crates and subcommands; move the TDD wording into the CLI

**Files:** `Cargo.toml` (workspace members), `crates/blyte-cli/{Cargo.toml,src/**}`, `crates/blyte-core`, `crates/blyte-mcp`, `crates/blyte-worktree`, `crates/blyte-review`, `crates/blyte-rules`, `crates/blyte-syntax` (deleted), CLI tests, `.github/workflows/cd.yml`, `CLAUDE.md`, `.claude/agy-rules.md`.

- [ ] **Step 1: Move TDD.**
  - Copy `crates/blyte-core/src/tdd.rs` to `crates/blyte-cli/src/tdd.rs`. In the copy, replace `use crate::models::MilestoneSpec;` with a local definition, keeping the file's own docs and unit tests:

```rust
/// One autonomous-TDD phase: what to do, and the command that decides it is done.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MilestoneSpec {
    pub id: Option<String>,
    pub title: String,
    pub verify_cmd: String,
    pub files: Vec<std::path::PathBuf>,
}
```

    If the moved file's code or tests set `MilestoneSpec` fields that do not exist here (`depends_on`), remove those field initialisers only.
  - Add `pub mod tdd;` to `crates/blyte-cli/src/lib.rs`.
  - In `task_runner.rs`, `blyte_core::tdd::tdd_milestones` becomes `crate::tdd::tdd_milestones`. Update `tdd_phases`'s doc comment: the wording lives in `crate::tdd` (moved from the removed daemon crate).
  - In `crates/blyte-cli/tests/test_tdd_phases.rs`, `blyte_core::tdd::` becomes `blyte_cli::tdd::`. This is a path change only.
- [ ] **Step 2: Remove the subcommands.**
  - In `cli.rs`, delete the `Commands` enum, the `command` field of `Cli`, and every type used only by them.
  - In `main.rs`, delete every `Some(Commands::…)` arm and the `match`. What remains: a non-empty prompt runs `task_runner::run_task`; otherwise the REPL starts, exactly as the current `None =>` arm does.
  - Delete `crates/blyte-cli/src/commands/` and `crates/blyte-cli/src/state_cli.rs`, and their `pub mod` lines.
  - Remove anything in `repl.rs`, `task_runner.rs` or `report.rs` that referenced them. If the REPL has slash-commands that call into deleted code, remove those commands and list them in your report.
- [ ] **Step 3: Remove the crates.**
  - Delete `crates/blyte-core`, `crates/blyte-mcp`, `crates/blyte-worktree`, `crates/blyte-review`, `crates/blyte-rules` and `crates/blyte-syntax`.
  - Remove them from the workspace `members` in the root `Cargo.toml` (if members is a glob such as `crates/*`, there is nothing to change) and from `crates/blyte-cli/Cargo.toml`.
  - Remove workspace dependencies that no remaining crate uses (for example `fd-lock`, `minijinja`, `chrono`), checking each with `git grep` before removing it.
- [ ] **Step 4: CLI tests.** Delete every file in `crates/blyte-cli/tests/` that exercises a removed subcommand or crate: `test_cli_blocking_review.rs`, `test_cli_branch_scoped_isolation.rs`, `test_cli_command_guard.rs`, `test_cli_daemon_guard.rs`, `test_cli_daemon_lifecycle.rs`, `test_cli_dag_daemon.rs`, `test_cli_handoff.rs`, `test_cli_impact_slicing.rs`, `test_cli_init_milestones.rs`, `test_cli_inspect.rs`, `test_cli_multi_daemon_fleet.rs`, `test_cli_review.rs`, `test_cli_rollback.rs`, `test_cli_shunt.rs`, `test_detach_argv.rs`, `test_state_init_default.rs`, `test_state_init_direct.rs`, `test_state_init.rs`, `test_state_prompt_worktree.rs`, `test_state_prompt.rs`, `test_state_worktree_lifecycle.rs`, `test_worktree_cli.rs`, and any other test that fails only because it invokes a removed subcommand.
  - For `test_cli_dispatch.rs`, `test_cli_smoke.rs` and `test_e2e_cli.rs`: delete the tests that invoke removed subcommands. Keep and pass the tests that exercise task mode, `--help`, flags or the REPL. Delete a file only if nothing in it remains.
  - Keep: `test_fold_budget.rs`, `test_repl_commands.rs`, `test_repl_model_cmd.rs`, `test_run_report.rs`, `test_task_runner.rs`, `test_tdd_phases.rs`, `test_working_set_e2e.rs`, `test_action_ledger_e2e.rs`.
- [ ] **Step 5: CI, docs, rules.**
  - In `.github/workflows/cd.yml`, remove the deleted crates from any crate list.
  - In `CLAUDE.md` and `.claude/agy-rules.md`, remove references to the deleted crates and subcommands, and remove the flaky-test mentions of `blyte-mcp`'s and `blyte-core`'s tests and `test_cli_daemon_detached_forwarding` if those tests no longer exist.
  - Do not edit `docs/superpowers/` (historical records) or `docs/adr/`.
- [ ] **Step 6: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Then run:

```bash
git grep -n -E "blyte_(core|mcp|worktree|review|rules|syntax)|blyte-(core|mcp|worktree|review|rules|syntax)" -- . ':!docs/superpowers' ':!docs/adr' ':!CHANGELOG.md'
ls crates/
./target/debug/blyte --help
```

  Expected: no `git grep` output; `ls` shows only `blyte-agent`, `blyte-cli` and `blyte-provider` (plus any untracked leftover directories from before, which you must not touch); `--help` lists no subcommands. Report line counts: `find crates/blyte-* -name '*.rs' | xargs wc -l | tail -1`.
- [ ] **Step 7: Commit.**

```bash
git add -A .
git commit --no-gpg-sign -m "refactor!: remove the bolt-on era (daemon, MCP, worktree, review, rules, syntax crates and subcommands)"
```
