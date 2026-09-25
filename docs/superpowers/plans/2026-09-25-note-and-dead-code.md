# Remove `note`, Then Sweep Dead Code Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:**
1. **Remove the `note` tool.** The system prompt promises *"Your notes persist across turns and are the only thing guaranteed to."* That was true under the deleted `StateOnly`, which rendered notes into every prompt. Under `folded` nothing shows them: a note is just a tool call that folds into a one-line digest. The model used `note` 4 times in 4 runs. The tool, the promise, and the `Agent` region of state it wrote to all go (user decision, 2026-09-25).
2. **Sweep dead code and unused dependencies** left behind by removing about 37K lines, with no change in behaviour.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/note-and-dead-code` (from `main`). Each task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back. Task 2 builds on Task 1.
- Do not run Docker, and do not run `harness_bench` with `--image`. `Cargo.lock` is gitignored. An evaluation run is using this machine, so build with `CARGO_BUILD_JOBS=4`.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass at the end of each task.
- These are deletions. Tests whose subject is deleted are deleted with it; incidental references are edited minimally. **List every deleted item and test in your report.** Do not weaken assertions about anything that stays.
- Commit with `--no-gpg-sign` and the exact message in each task.

---

### Task 1: Remove the `note` tool and the `Agent` state region

**Files:** `crates/blyte-agent/src/{tools.rs,run.rs,state.rs}`, `crates/blyte-agent/examples/solve.rs`, `crates/blyte-cli/src/{repl.rs,report.rs}` (only if they reference the removed items), and tests.

- [x] **Step 1: Agent.**
  - In `tools.rs`, delete the `note` `ToolSpec` from `specs()`.
  - In `run.rs`:
    - delete the `## Working notes` section of the system prompt (the paragraph that starts `Use \`note\` to record what you learn`), leaving the other sections as they are;
    - delete the `call.name == "note"` branch in `run_until`, so every tool call goes to `tools::execute`;
    - delete the `"note" =>` arms of `extract_action_target` and `extract_action_summary`. A `note` call now falls through to their generic arms, which must still compile and behave for unknown tools.
  - In `state.rs`, delete `pub struct Agent`, the `State.agent` field, its initialisation in `State::new`, `apply_agent_patch`, and docs that describe the Agent region or worker-owned patches. Keep `Orchestrator` and `Runtime`, and update the `State` doc comment to describe two regions.
  - In `examples/solve.rs`, remove the line that prints `state.agent.fields`.
- [x] **Step 2: CLI.** If `repl.rs`'s `/state` output, `report.rs`, or anything else references `state.agent`, `apply_agent_patch` or note keys (e.g. `agent_note_keys`), remove those references and any report field that only carried them.
- [x] **Step 3: Tests.**
  - Delete `crates/blyte-agent/tests/test_state_authority.rs`: its subject is the Agent region's patch authority.
  - In `test_state.rs`, delete the test that applies an agent patch.
  - In `test_run.rs`, delete the test that drives the `note` tool.
  - In `test_action_extraction.rs`, delete the `note` cases.
  - Fix any other compile errors minimally, and list them.
- [x] **Step 4: Full check.** Run the full check (with `CARGO_BUILD_JOBS=4`). Expected: clean, 0 failed. Then:

```bash
git grep -n -E '"note"|apply_agent_patch|state\.agent|struct Agent\b|Working notes|agent_note_keys' -- crates/
```

  Expected: no output.
- [x] **Step 5: Commit.**

```bash
git add -A crates/
git commit --no-gpg-sign -m "refactor!: remove the note tool and the Agent state region (the prompt promised persistence it no longer had)"
```

---

### Task 2: Sweep dead code and unused dependencies

**Files:** any under `crates/`, the root `Cargo.toml` and the crate `Cargo.toml`s.

**Rules (apply mechanically, and report every deletion):**
1. **Items.** A `pub` or private function, method, type, constant, field or module in `crates/blyte-agent`, `crates/blyte-provider` or `crates/blyte-cli` is **dead** if nothing outside its own definition references it, except tests. For each dead item:
   - delete it;
   - delete the tests that exist only to test it (a test file whose only subject is dead goes entirely);
   - if a test uses a dead item only incidentally, edit that test minimally.
   
   Use `git grep -n -w <name> -- crates` for each candidate. **Keep:** entry points (`main`, the examples `harness_bench.rs` and `solve.rs`, `Provider` trait impls and methods used through `dyn Provider`), trait methods with in-crate implementors, and serde fields that are serialised into the run report (the report is read by the eval).
2. **Fields never read.** A struct field that is written but never read outside tests is dead: delete it and its writes (for example `Runtime` fields no longer consumed). Keep report fields (rule 1).
3. **Dependencies.** For each `[dependencies]` and `[dev-dependencies]` entry of each crate, and each `[workspace.dependencies]` entry, check with `git grep -n -w "<crate_name_with_underscores>" -- crates/<crate>` (and for macros, e.g. `serde::`, `thiserror::`, `#[derive(Error`). Remove any the crate does not use. Remove workspace dependencies no crate uses.
4. **Stale docs in code.** Delete module or item docs that describe removed features (StateOnly, working set, loop-breaker, daemon, MCP, fold notes, cost cap, journal, notes/Agent region). Do not reword docs of things that stay.

- [x] **Step 1: Find candidates.** List every `pub fn`, `pub struct`, `pub enum`, `pub const` and `pub mod` in the three crates, and every field of `State`, `Orchestrator`, `Runtime`, `RunReport` and `TaskConfig`. Check each against rules 1–2. Record the dead ones.
- [x] **Step 2: Delete** the dead items, their writes, their tests and the stale docs (rules 1, 2 and 4).
- [x] **Step 3: Dependencies** (rule 3).
- [x] **Step 4: Full check.** Run the full check (with `CARGO_BUILD_JOBS=4`). Expected: clean, 0 failed. Run `cargo run --release -p blyte-agent --example harness_bench -- --turns 200` (local only), and report its output and `find crates -name '*.rs' | xargs wc -l | tail -1`.
- [x] **Step 5: Commit.**

```bash
git add -A Cargo.toml crates/
git commit --no-gpg-sign -m "refactor: sweep dead code and unused dependencies"
```
