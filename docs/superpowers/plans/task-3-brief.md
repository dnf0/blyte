# Task 3 Brief: Context Assembly in `StateOnly::build`

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-harness-observed-working-set`

## Context
Tasks 1 & 2 defined and synchronized `WorkingSet` in `Runtime` (R).
Task 3 wires this working set into the prompt assembly in `crates/superstate-agent/src/context.rs` (`StateOnly::build`).
This makes active file snippets and recent test failures visible to the agent across turns in `StateOnly` mode while strictly preserving Invariant II (`messages.len() <= 3`).

## Global Constraints
- Target crate: `crates/superstate-agent`
- Rust 2021 edition, zero compiler warnings (`cargo check -p superstate-agent`, `cargo clippy -p superstate-agent --all-targets -- -D warnings`)
- Clean execution on Unix platforms (macOS / Linux)
- Invariant II (Strict Prompt Bounding): `messages.len() <= 3` strictly preserved under `StateOnly::build`.
- Sections are omitted completely when empty.
- Conventional commit message with `--no-gpg-sign`.

## Files to Modify / Create
- Modify: `crates/superstate-agent/src/context.rs`
- Modify: `crates/superstate-agent/tests/test_run.rs`

## Step-by-Step Instructions

### 1. Write the failing tests first (TDD RED phase)
In `crates/superstate-agent/tests/test_run.rs`:
- Add `test_state_only_context_renders_working_set_and_preserves_bound`:
  Populate `state.runtime.working_set` with a file snippet and a test failure. Call `StateOnly.build(&state, &history)`.
  Assert:
  - `msgs.len() <= 3`
  - `prompt` contains `## Active Working Set`
  - `prompt` contains `### \`lru.py\`` and the file content
  - `prompt` contains `## Last Test Failure` and the error message
- Add `test_state_only_context_omits_working_set_when_empty`:
  When `state.runtime.working_set` is default/empty, assert that neither `## Active Working Set` nor `## Last Test Failure` appears in the prompt.
- Add `test_state_only_runner_injects_working_set_into_llm_prompts`:
  Verify full runner prompt injection using `MockProvider`.

### 2. Run tests to verify they fail
Run: `cargo test -p superstate-agent --test test_run test_state_only_context_renders_working_set`
Observe failure.

### 3. Implement minimal code (GREEN phase)
In `crates/superstate-agent/src/context.rs` (`StateOnly::build`):
Before `Actions Taken So Far`:
```rust
        // Inject Active Working Set if non-empty
        if !state.runtime.working_set.files.is_empty() {
            user_prompt.push_str("\n## Active Working Set\n");
            for file in &state.runtime.working_set.files {
                let lines_info = match (file.start_line, file.end_line) {
                    (Some(s), Some(e)) => format!(" (lines {s}-{e})"),
                    _ => String::new(),
                };
                user_prompt.push_str(&format!(
                    "### `{}`{}\n```\n{}\n```\n",
                    file.path,
                    lines_info,
                    file.content.trim_end()
                ));
            }
        }

        // Inject Last Test Failure if present
        if let Some(failure) = &state.runtime.working_set.last_failure {
            user_prompt.push_str(&format!(
                "\n## Last Test Failure\n```text\n{}\n```\n",
                failure.trim_end()
            ));
        }
```

### 4. Run tests and linter
Run:
- `cargo test -p superstate-agent --test test_run`
- `cargo check -p superstate-agent --all-targets`
- `cargo clippy -p superstate-agent --all-targets -- -D warnings`

### 5. Commit
```bash
git add crates/superstate-agent/src/context.rs crates/superstate-agent/tests/test_run.rs
git commit --no-gpg-sign -m "feat(agent): inject Active Working Set and Last Test Failure into StateOnly prompt"
roborev status; roborev show HEAD
```

### 6. Write report
Save report to `/Users/danielfisher/repos/superstate/.wt/feat-harness-observed-working-set/docs/superpowers/plans/task-3-report.md`.
