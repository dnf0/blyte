# Task 2: Deterministic Target and Outcome Extraction Helpers in run.rs

Working Directory: `/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness`

## What to Implement

In `crates/superstate-agent/src/run.rs`:
Implement two extraction helper functions:
```rust
pub(crate) fn extract_action_target(tool: &str, args: &serde_json::Value) -> String;
pub(crate) fn extract_action_summary(tool: &str, content: &str, is_error: bool) -> String;
```

### Extraction Specifications:

#### `extract_action_target`:
- `"read"`:
  - Extract `"path"`. If `"start_line"` and `"end_line"` are present, return `format!("{path} (L{start_line}-L{end_line})")`.
  - If only `"path"` is present, return `path.to_string()`.
- `"write"`:
  - Extract `"path"` and `"content"`. Return `format!("{path} ({} bytes)", content.len())`. If content missing, return `path.to_string()`.
- `"edit"`:
  - Extract `"path"`. Return `path.to_string()`.
- `"bash"`:
  - Extract `"command"`. If length > 60 characters, return first 57 characters + `"..."`.
- `"glob"`:
  - Extract `"pattern"`. Return `pattern.to_string()`.
- `"grep"`:
  - Extract `"pattern"`. If `"path"` is provided, return `format!("pattern '{pattern}' in {path}")`. Otherwise return `format!("pattern '{pattern}'")`.
- `"note"`:
  - Extract `"patch"`. If it's a JSON object, return `format!("patch ({} keys)", patch.as_object().map_or(0, |o| o.len()))`.
- Fallback / Unknown tool:
  - Return first string argument value found or tool name.

#### `extract_action_summary`:
- `"read"`:
  - Return `format!("{} lines read", content.lines().count())`.
- `"write"`:
  - Return `"file written".to_string()`.
- `"edit"`:
  - If `is_error`: return `format!("failed: {}", content.lines().next().unwrap_or("edit failed"))`.
  - Else: return `"string replaced".to_string()`.
- `"bash"`:
  - If `!is_error`: return `"exit 0".to_string()`.
  - Else: find the last non-empty line of `content`, clamped to 80 chars, and return `format!("failed: {}", last_line)`.
- `"glob"`:
  - Count non-empty lines in `content`: `format!("{} files matched", count)`.
- `"grep"`:
  - Count non-empty lines in `content`: `format!("{} matches found", count)`.
- `"note"`:
  - Return `content.lines().next().unwrap_or("recorded").to_string()`.
- Fallback:
  - First line of `content` clamped to 80 chars.

---

## Testing Requirements (TDD)

Create `crates/superstate-agent/tests/test_action_extraction.rs`:
Test extraction for:
1. `read`: with and without line numbers.
2. `write`: target includes bytes, summary is "file written".
3. `edit`: success ("string replaced") and failure ("failed: string not found").
4. `bash`: short command and long command truncation (>60 chars); success ("exit 0") and failure (last non-empty line extracted).
5. `glob`: file count matching.
6. `grep`: pattern and path, match count.
7. `note`: key count, status string.

## Execution Steps
1. Write tests in `crates/superstate-agent/tests/test_action_extraction.rs`.
2. Run `cargo test -p superstate-agent --test test_action_extraction` to observe failure.
3. Implement `extract_action_target` and `extract_action_summary` in `crates/superstate-agent/src/run.rs`.
4. Run `cargo test -p superstate-agent --test test_action_extraction` to verify all tests pass.
5. Verify `cargo check -p superstate-agent` and `cargo clippy -p superstate-agent`.
6. Commit: `git add crates/superstate-agent/src/run.rs crates/superstate-agent/tests/test_action_extraction.rs && git commit --no-gpg-sign -m "feat(agent): implement deterministic action target and outcome summarizers"`
7. Run `roborev status; roborev show HEAD`.
8. Write your completion report to `docs/superpowers/plans/task-2-report.md`.
