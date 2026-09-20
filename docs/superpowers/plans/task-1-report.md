# Task 1 Report: Command Specialization in `impact.rs`

**Status:** Completed (DONE)  
**Commit SHA:** `1e6547e3e0d4b83da692cab152353e5dbf0a7660` (`1e6547e`)  
**Branch:** `feat/daemon-test-impact-slicing`  
**Repository:** `/Users/danielfisher/repos/superstate/.worktrees/feat-daemon-test-impact-slicing`  
**Date:** 2026-09-19  

---

## Executive Summary

Task 1 implemented command specialization in `crates/superstate-core/src/impact.rs` via `pub fn build_sliced_test_cmd(base_cmd: &str, impacted_files: &[PathBuf], repo_root: &Path) -> Option<String>`.

In the two-tier milestone-scoped test impact slicing architecture, `infer_impacted_tests` identifies which test files are affected by changes in a milestone. `build_sliced_test_cmd` inspects the base milestone verification command (`verify_cmd`), detects the underlying test runner harness (Pytest, Cargo, Go, Vitest/Jest/Node), and specializes the command invocation to target only the impacted test files. If the verification command is generic or unrecognized (e.g. `sh verify.sh`, `make test`, or ungated commands like `exit 0`), or if no files were impacted, it returns `None`, safely falling back to the standard verification workflow.

---

## TDD Execution

### 1. Red Phase
Created unit tests in `crates/superstate-core/tests/test_impact_slicing.rs` validating:
- `test_build_sliced_test_cmd_pytest`: Verifies `pytest tests/` with `tests/test_auth.py` and `tests/test_token.py` specializes to `Some("pytest tests/test_auth.py tests/test_token.py")`.
- `test_build_sliced_test_cmd_pytest_container_prefix`: Verifies container-prefixed `./run-in-container pytest tests/` specializes to `Some("./run-in-container pytest tests/test_auth.py")`.
- `test_build_sliced_test_cmd_cargo`: Verifies `cargo test` with `tests/test_lexer.rs` maps to `Some("cargo test --test test_lexer")`.
- `test_build_sliced_test_cmd_unrecognized_returns_none`: Verifies generic scripts like `sh verify.sh` yield `None`.
- `test_build_sliced_test_cmd_empty_impacted_files_returns_none`: Verifies empty `impacted_files` yields `None`.
- `test_build_sliced_test_cmd_cargo_multiple_targets`: Verifies multiple integration test targets are formatted as `--test <target>`, sorted, and deduplicated.
- `test_build_sliced_test_cmd_node_vitest_and_jest`: Verifies `npx vitest run`, `npx jest`, and `npm test` invocations.
- `test_build_sliced_test_cmd_go_package`: Verifies Go test package directory slicing (`go test ./...` with `pkg/server/server_test.go` $\rightarrow$ `go test ./pkg/server`).

Ran `cargo test -p superstate-core --test test_impact_slicing` to verify RED failure:
```text
error[E0432]: unresolved import `superstate_core::impact::build_sliced_test_cmd`
 --> crates/superstate-core/tests/test_impact_slicing.rs:2:5
  |
2 | use superstate_core::impact::build_sliced_test_cmd;
  |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ no `build_sliced_test_cmd` in `impact`
error: could not compile `superstate-core` (test "test_impact_slicing") due to 1 previous error
```

### 2. Green Phase
Implemented `pub fn build_sliced_test_cmd` and `is_word_boundary_match` in `crates/superstate-core/src/impact.rs`:
- Added word boundary validation to prevent substring false-positives (e.g. `run_pytest_suite.sh`).
- Detected Pytest, supporting direct, container-prefixed (`./run-in-container`), `python -m pytest`, and `uv run pytest` prefixes.
- Detected Cargo test, transforming integration tests under `tests/` directories into `--test <target>` flags, and unit test files into name filters.
- Detected Go test, translating file paths into package directories (`./<dir>`).
- Detected Node/Vitest/Jest, supporting `npm test --`, `pnpm test --`, `yarn test`, `npx vitest run`, `npx vitest`, and `npx jest`.
- Relativized all file paths against `repo_root` with deterministic sorting and deduplication.

Ran `cargo test -p superstate-core --test test_impact_slicing`:
```text
running 8 tests
test test_build_sliced_test_cmd_empty_impacted_files_returns_none ... ok
test test_build_sliced_test_cmd_unrecognized_returns_none ... ok
test test_build_sliced_test_cmd_cargo ... ok
test test_build_sliced_test_cmd_cargo_multiple_targets ... ok
test test_build_sliced_test_cmd_pytest ... ok
test test_build_sliced_test_cmd_pytest_container_prefix ... ok
test test_build_sliced_test_cmd_node_vitest_and_jest ... ok
test test_build_sliced_test_cmd_go_package ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Ran `cargo test -p superstate-core`: All 24 test suites passed.

### 3. Static Analysis & Verification
- `cargo clippy -p superstate-core --all-targets -- -D warnings`: Passed with 0 warnings.
- `cargo clippy --all-targets -- -D warnings`: Passed across entire workspace with 0 warnings.
- `cargo check --all`: Clean compilation across all crates.
- `cargo fmt --all -- --check`: Checked and confirmed clean.

---

## File Changes Summary

| File | Status | Description |
|---|---|---|
| `crates/superstate-core/tests/test_impact_slicing.rs` | Created | Added 8 unit tests for test runner command specialization |
| `crates/superstate-core/src/impact.rs` | Modified | Implemented `build_sliced_test_cmd` and `is_word_boundary_match` helper |

---

## Commit Details

- **Commit SHA:** `1e6547e3e0d4b83da692cab152353e5dbf0a7660` (`1e6547e`)
- **Message:** `feat(core): implement test runner command specialization for impact slicing`
- **GPG Verification:** Skipped (`--no-gpg-sign`) per repository guidance.
