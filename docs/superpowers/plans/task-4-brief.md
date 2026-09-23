# Task 4 Brief: Loop-Breaker Interceptor (Guard Clause)

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-harness-observed-working-set`

## Context
In live evaluations under `--context state-only`, we observed that models can get trapped in repetitive reading loops (e.g. oscillating between reading `lru.py` and `test_lru.py` 10+ times without taking action).
Task 4 implements a deterministic loop-breaker guard clause in `crates/superstate-agent/src/run.rs`.
If the agent requests a redundant read for a file already resident in `state.runtime.working_set`, the harness intercepts the execution and immediately returns an advisory guidance notice reminding the agent that the file is already in its Active Working Set.

## Global Constraints
- Target crate: `crates/superstate-agent`
- Rust 2021 edition, zero compiler warnings (`cargo check -p superstate-agent`, `cargo clippy -p superstate-agent --all-targets -- -D warnings`)
- Clean execution on Unix platforms (macOS / Linux)
- Invariant III: In-memory string operations, zero latency. Zero shell or filesystem execution on intercepted reads.
- Conventional commit message with `--no-gpg-sign`.

## Files to Modify / Create
- Modify: `crates/superstate-agent/src/run.rs`
- Modify: `crates/superstate-agent/tests/test_run.rs`

## Step-by-Step Instructions

### 1. Write the failing tests first (TDD RED phase)
In `crates/superstate-agent/tests/test_run.rs`:
- Add `test_loop_breaker_intercepts_redundant_reads`:
  Simulate a run where the model calls `read lru.py`, and then on the very next turn calls `read lru.py` again.
  Assert that on the second turn, the tool outcome contains `[Harness Working Memory Guard]: \`lru.py\` is already loaded in your Active Working Set above`.
  Assert that the action ledger records this guard interception.
- Add `test_loop_breaker_allows_read_after_edit`:
  Simulate a run where the model calls `read lru.py`, then `edit lru.py`, and then reads `lru.py` again. Assert that because an edit intervened, the read is not blocked.

### 2. Run tests to verify they fail
Run: `cargo test -p superstate-agent --test test_run test_loop_breaker`
Observe failure.

### 3. Implement minimal code (GREEN phase)
In `crates/superstate-agent/src/run.rs`:
Implement a helper function:
```rust
fn should_intercept_redundant_read(
    call: &superstate_provider::ToolCall,
    working_set: &crate::working_set::WorkingSet,
    actions: &[ActionEntry],
) -> Option<String> {
    if call.name != "read" {
        return None;
    }
    let raw_path = call.arguments.get("path").and_then(|v| v.as_str())?;
    let path = normalize_path(raw_path);
    let is_cached = working_set.files.iter().any(|f| f.path == path);
    if !is_cached {
        return None;
    }

    // Check if the immediately preceding tool call was also read on this path
    let last_was_same_read = actions
        .last()
        .map(|a| a.tool == "read" && a.target.contains(path))
        .unwrap_or(false);

    // Or read this path 2+ times in the last 4 actions without an intervening edit
    let recent_same_reads = actions
        .iter()
        .rev()
        .take(4)
        .take_while(|a| a.tool != "edit")
        .filter(|a| a.tool == "read" && a.target.contains(path))
        .count();

    if last_was_same_read || recent_same_reads >= 2 {
        Some(format!(
            "[Harness Working Memory Guard]: `{path}` is already loaded in your Active Working Set above. \
             You do not need to re-read it. Propose your exact edit using `edit` or update your plan in `note`."
        ))
    } else {
        None
    }
}
```

In `Runner::run`, check `should_intercept_redundant_read` before delegating to `tools::execute`:
```rust
let outcome = if let Some(guard_msg) = should_intercept_redundant_read(call, &state.runtime.working_set, &state.runtime.actions) {
    crate::tools::ToolOutcome {
        content: guard_msg,
        is_error: false,
    }
} else if call.name == "note" {
    // note handling
    ...
} else {
    tools::execute(...)
};
```

### 4. Run tests and linter
Run:
- `cargo test -p superstate-agent --test test_run`
- `cargo check -p superstate-agent --all-targets`
- `cargo clippy -p superstate-agent --all-targets -- -D warnings`

### 5. Commit
```bash
git add crates/superstate-agent/src/run.rs crates/superstate-agent/tests/test_run.rs
git commit --no-gpg-sign -m "feat(agent): intercept redundant reads with harness working memory guard"
roborev status; roborev show HEAD
```

### 6. Write report
Save report to `/Users/danielfisher/repos/superstate/.wt/feat-harness-observed-working-set/docs/superpowers/plans/task-4-report.md`.
