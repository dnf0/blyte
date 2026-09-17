# Task 1 Brief: Core Review Engine & Runner in `crates/superstate-review`

- **Repository Directory**: `/Users/danielfisher/repos/superstate`
- **Current Branch**: `feat/advisory-code-review`
- **Depends on:** none

## Goal
Implement the core review runner and report parser in `crates/superstate-review`, including driver resolution (`agy` / `claude` / custom driver), safe markdown code-fence stripping for JSON, and parsing into `ReviewReport` (`ReviewFinding`, `Severity`, `has_blocking_issues`).

## Files to Touch
- Modify: `crates/superstate-review/src/schema.rs`
- Modify: `crates/superstate-review/src/lib.rs`
- Create: `crates/superstate-review/src/runner.rs`
- Create: `crates/superstate-review/tests/test_runner.rs`

## Detailed Requirements & Interfaces
1. In `crates/superstate-review/src/schema.rs`:
   - Implement `std::fmt::Display` for `ReviewReport` so it prints user-friendly summaries and lists findings with file, line range, problem, and suggested fix.
2. In `crates/superstate-review/src/runner.rs`:
   - Define:
     ```rust
     pub struct ReviewConfig {
         pub driver: Option<String>,
         pub timeout_secs: u64,
     }

     #[derive(thiserror::Error, Debug)]
     pub enum ReviewError {
         #[error("I/O error: {0}")]
         Io(#[from] std::io::Error),
         #[error("Diff error: {0}")]
         Diff(#[from] crate::diff::DiffError),
         #[error("Driver execution error: {0}")]
         Driver(String),
         #[error("Driver not found in PATH or environment")]
         DriverNotFound,
         #[error("JSON parse error: {0}")]
         Json(String),
     }

     pub fn resolve_review_driver(configured_driver: Option<&str>) -> Result<String, ReviewError>;
     pub fn clean_json_response(raw: &str) -> String;
     pub fn parse_review_report(raw: &str) -> Result<ReviewReport, ReviewError>;
     pub fn execute_review(
         repo_root: &std::path::Path,
         diff: &str,
         task_instruction: Option<&str>,
         config: &ReviewConfig,
     ) -> Result<ReviewReport, ReviewError>;
     ```
   - In `resolve_review_driver`:
     - If `configured_driver` is provided and non-empty, use `superstate_core::daemon::resolve_driver_command` or tokenize and append `-p` / `--dangerously-skip-permissions` as needed. (Note: check dependencies in Cargo.toml; if `superstate-review` doesn't depend on `superstate-core`, keep driver resolution self-contained in `superstate-review` or check `Cargo.toml`).
     - Check env vars `SUPERSTATE_REVIEW_DRIVER` and `SUPERSTATE_DRIVER`.
     - Check `shutil` / `which` in PATH for `agy` (`agy --model gemini-3.8-flash-low --print --dangerously-skip-permissions`) or `claude` (`claude -p --dangerously-skip-permissions`).
     - If neither is found and no driver is configured, return `ReviewError::DriverNotFound`.
   - In `clean_json_response`:
     - Strip markdown code blocks (` ```json ... ``` ` or ` ``` ... ``` `).
     - Trim whitespace.
   - In `parse_review_report`:
     - Parse JSON Value.
     - If Value is an Object: deserialize as `ReviewReport`. If `has_blocking_issues` is not set or false, but any finding has `Critical` or `High` severity, set `has_blocking_issues = true`.
     - If Value is an Array: deserialize as `Vec<ReviewFinding>`, set summary to "Review generated N findings", and compute `has_blocking_issues`.
   - In `execute_review`:
     - If `diff.trim().is_empty()`, return `Ok(ReviewReport::clean("No changes detected in diff."))`.
     - Build prompt combining system prompt (`build_review_system_prompt()`), optional task instructions, and the diff.
     - Run driver command using `std::process::Command`, with stdin or arg passing (e.g. `cmd -p <prompt>` or passing via stdin if supported).
     - Parse stdout with `parse_review_report`.

## TDD Instructions
- Step 1: Write failing tests in `crates/superstate-review/tests/test_runner.rs`.
- Step 2: Run `cargo test -p superstate-review --test test_runner` (verify FAIL).
- Step 3: Implement minimal code in `schema.rs`, `runner.rs`, and re-export in `lib.rs`.
- Step 4: Run `cargo test -p superstate-review` (verify PASS).
- Step 5: Commit changes with message `feat(review): implement review runner and report parser`.
