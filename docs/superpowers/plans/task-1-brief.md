# Task 1 Brief: WorkingSet Data Structures and Invariant I State Partitioning

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-harness-observed-working-set`

## Context
We are implementing the Harness-Observed Working Set to eliminate Stateless Amnesia and ping-pong file-reading oscillations in `StateOnly` mode.
In this first task, we establish the core data structures and ensure they strictly live in `Runtime` (R) to uphold Invariant I.

## Global Constraints
- Target crate: `crates/superstate-agent`
- Rust 2021 edition, zero compiler warnings (`cargo check -p superstate-agent`, `cargo clippy -p superstate-agent --all-targets -- -D warnings`)
- Clean execution on Unix platforms (macOS / Linux)
- Invariant I (Partition Integrity): `working_set` lives strictly in `Runtime` (R). Agent patches cannot modify or clear it.
- Conventional commit message with `--no-gpg-sign`.

## Files to Create / Modify
- Create: `crates/superstate-agent/src/working_set.rs`
- Modify: `crates/superstate-agent/src/lib.rs` (add `pub mod working_set;`)
- Modify: `crates/superstate-agent/src/state.rs` (add `pub working_set: WorkingSet` to `Runtime`)
- Create: `crates/superstate-agent/tests/test_working_set.rs`

## Step-by-Step Instructions

### 1. Write the tests first (TDD)
Create `crates/superstate-agent/tests/test_working_set.rs` containing:
- `test_working_set_add_and_update`: Verify adding and updating file snippets.
- `test_working_set_apply_edit`: Verify exact string replacement in cached snippets.
- `test_working_set_lru_eviction`: Verify LRU eviction when file count (3) or line budget (250) is exceeded.
- `test_working_set_failure_lifecycle`: Verify recording distilled test failures and clearing on pass.
- `test_invariant_i_agent_patch_cannot_mutate_working_set`: Verify `State.apply_agent_patch` cannot touch or nullify `working_set`.

### 2. Run test to verify it fails
Run: `cargo test -p superstate-agent --test test_working_set`
Observe failure.

### 3. Implement minimal code
1. Create `crates/superstate-agent/src/working_set.rs`:
   - `pub const MAX_ACTIVE_FILES: usize = 3;`
   - `pub const MAX_FILE_LINES: usize = 100;`
   - `pub const MAX_TOTAL_LINES: usize = 250;`
   - `FileSnippet` struct (`path`, `content`, `start_line`, `end_line`, `last_accessed_turn`).
   - `WorkingSet` struct (`files: Vec<FileSnippet>`, `last_failure: Option<String>`).
   - Implement `add_or_update`, `apply_edit`, `record_failure`, `clear_failure`, and `prune_lru`.
2. Add `pub mod working_set;` in `crates/superstate-agent/src/lib.rs`.
3. Add `pub working_set: WorkingSet` (with `#[serde(default)]`) to `Runtime` in `crates/superstate-agent/src/state.rs`.

### 4. Run tests and linter
Run:
- `cargo test -p superstate-agent --test test_working_set`
- `cargo check -p superstate-agent --all-targets`
- `cargo clippy -p superstate-agent --all-targets -- -D warnings`

### 5. Commit
```bash
git add crates/superstate-agent/src/working_set.rs crates/superstate-agent/src/lib.rs crates/superstate-agent/src/state.rs crates/superstate-agent/tests/test_working_set.rs
git commit --no-gpg-sign -m "feat(agent): implement WorkingSet data structures and Invariant I state partitioning"
roborev status; roborev show HEAD
```

### 6. Write report
Save report to `/Users/danielfisher/repos/superstate/.wt/feat-harness-observed-working-set/docs/superpowers/plans/task-1-report.md`.
