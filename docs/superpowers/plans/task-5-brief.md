# Task 5 Brief: End-to-End Verification & Live Eval Re-Run

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-harness-observed-working-set`

## Context
Tasks 1 through 4 implemented:
1. `WorkingSet` and `FileSnippet` data structures with Invariant I state partitioning.
2. Synchronous working set updates in `run.rs` on `read`, `edit`, `write`, and `bash`.
3. Bounded context assembly in `StateOnly::build` injecting `## Active Working Set` and `## Last Test Failure` while preserving `messages.len() <= 3`.
4. The loop-breaker guard clause intercepting redundant reads.

Task 5 provides:
1. End-to-end integration tests in `crates/superstate-cli/tests/test_working_set_e2e.rs` verifying REPL `/state` working set display and CLI execution.
2. Clean workspace test, clippy, and fmt verification.
3. Live empirical evaluation with `gemini-3.8-flash` on the LRU cache bug in `--context state-only` to measure turns to resolution, token efficiency, and the complete elimination of Stateless Amnesia / ping-pong oscillations.

## Global Constraints
- Target crates: `crates/superstate-cli`, `crates/superstate-agent`
- Rust 2021 edition, zero compiler warnings (`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`)
- Clean execution on Unix platforms (macOS / Linux)
- Invariants I, II, and III fully preserved.
- Conventional commit message with `--no-gpg-sign`.

## Files to Create / Modify
- Create: `crates/superstate-cli/tests/test_working_set_e2e.rs`
- Modify: `crates/superstate-cli/src/repl.rs` (if `/state` needs formatting for active working set files)

## Step-by-Step Instructions

### 1. Write the tests first (TDD RED phase)
In `crates/superstate-cli/tests/test_working_set_e2e.rs`:
- Test CLI / REPL display of working set files and last failure under `/state`.
- Test multi-file coordination where `read`, `edit`, and `bash` update state observable by the runner.

### 2. Run tests and verify
Run: `cargo test -p superstate --test test_working_set_e2e`

### 3. Check Workspace Quality Gates
Run:
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`

### 4. Commit
```bash
git add crates/superstate-cli/tests/test_working_set_e2e.rs crates/superstate-cli/src/repl.rs
git commit --no-gpg-sign -m "test(cli): add end-to-end verification for harness working set"
roborev status; roborev show HEAD
```

### 5. Live Eval Re-Run with `gemini-3.8-flash`
Set up isolated eval in `/tmp/eval-lru-gemini-v2`:
- Python LRU cache bug with unit tests.
- Run `superstate run --context state-only "Fix the LRU bug so test_lru.py passes" --verify-cmd "python3 -m unittest test_lru.py"` using `gemini-3.8-flash`.
- Record:
  - Turns to resolution
  - Working set contents across turns
  - Input & output token counts
  - Verification pass confirmation
- Compare directly with baseline (13 turns, ping-pong loops).

### 6. Write report
Save report to `/Users/danielfisher/repos/superstate/.wt/feat-harness-observed-working-set/docs/superpowers/plans/task-5-report.md`.
