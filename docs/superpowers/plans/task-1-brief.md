# Task 1 Brief: Command Specialization in `impact.rs`

## Global Constraints
- Compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No compiler or clippy warnings (`cargo clippy --all-targets -- -D warnings`).
- Skip GPG verification on all commits (`--no-gpg-sign`).
- Zero prompt engineering: all behavior governed by git diffs, process exit codes (`$? == 0`), and state invariants.

## Task Details
- **Task:** Task 1: Command Specialization in `impact.rs`
- **Depends on:** none
- **Working Directory:** `/Users/danielfisher/repos/superstate/.worktrees/feat-daemon-test-impact-slicing`

**Files:**
- Create: `crates/superstate-core/tests/test_impact_slicing.rs`
- Modify: `crates/superstate-core/src/impact.rs`

**Interfaces:**
- Produces: `pub fn build_sliced_test_cmd(base_cmd: &str, impacted_files: &[PathBuf], repo_root: &Path) -> Option<String>`

## Implementation Steps
Follow TDD strictly:
1. Write failing unit tests in `crates/superstate-core/tests/test_impact_slicing.rs`:
   - `test_build_sliced_test_cmd_pytest`: `pytest tests/` with `tests/test_auth.py` and `tests/test_token.py` $\rightarrow$ `Some("pytest tests/test_auth.py tests/test_token.py")`.
   - `test_build_sliced_test_cmd_pytest_container_prefix`: `./run-in-container pytest tests/` with `tests/test_auth.py` $\rightarrow$ `Some("./run-in-container pytest tests/test_auth.py")`.
   - `test_build_sliced_test_cmd_cargo`: `cargo test` with `tests/test_lexer.rs` $\rightarrow$ `Some("cargo test --test test_lexer")`.
   - `test_build_sliced_test_cmd_unrecognized_returns_none`: `sh verify.sh` with `tests/test_auth.py` $\rightarrow$ `None`.
2. Run `cargo test -p superstate-core --test test_impact_slicing` to confirm failure (unresolved function).
3. Implement `pub fn build_sliced_test_cmd` in `crates/superstate-core/src/impact.rs`:
   - Inspects `base_cmd` to detect Pytest, Cargo, Node/Vitest/Jest, etc.
   - If `impacted_files` is empty, returns `None`.
   - Translates impacted test files into relative paths from `repo_root`.
   - Returns `Some(specialized_cmd)` or `None`.
4. Run `cargo test -p superstate-core --test test_impact_slicing` and verify PASS.
5. Run `cargo clippy -p superstate-core --all-targets -- -D warnings`.
6. Commit changes:
   ```bash
   git add crates/superstate-core/src/impact.rs crates/superstate-core/tests/test_impact_slicing.rs
   git commit --no-gpg-sign -m "feat(core): implement test runner command specialization for impact slicing"
   ```
7. Write implementer report to `docs/superpowers/plans/task-1-report.md`.
8. Report status: DONE / DONE_WITH_CONCERNS / BLOCKED.
