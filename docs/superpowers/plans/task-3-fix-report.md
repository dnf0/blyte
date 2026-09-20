# Task 3 Fix Report: Exclude `.agents` from Fingerprint & Git Checkpointing

## 1. Overview & Status
- **Status:** DONE
- **Task:** Task 3 Review Findings: Exclude `.agents` from tree fingerprint and git checkpointing, standardize CLI enum alias
- **Commit:** `d5a5939` (`fix(daemon): exclude .agents from tree fingerprint and git checkpointing`)
- **Working Directory:** `/Users/danielfisher/repos/superstate/.worktrees/feat-pre-tool-use-file-guard`
- **Branch:** `feat/pre-tool-use-file-guard`
- **Base Commit:** `97e5839`

## 2. Reviewer Findings Addressed

1. **Fingerprint Directory Exclusion**:
   - In `crates/superstate-core/src/daemon.rs`, added `".agents"` to `FINGERPRINT_SKIP_DIRS`:
     ```rust
     const FINGERPRINT_SKIP_DIRS: [&str; 5] = [
         ".git",
         ".agent-state",
         ".agents",
         ".pytest_cache",
         "__pycache__",
     ];
     ```
   - Exported `pub fn workspace_fingerprint(dir: &Path) -> String` to make content fingerprinting accessible for integration regression tests.
   - Prevents automated provisioning of `.agents/hooks.json` on turn 1 from registering false progress when no project files have changed.

2. **Git Exclude for Ephemeral Hooks**:
   - In `crates/superstate-core/src/daemon.rs`, when provisioning `.agents/hooks.json` under `guard_file_reads`, if `.git/info` exists, `.agents/` is appended to `.git/info/exclude` if not already present.
   - Ensures ephemeral daemon hooks are not inadvertently tracked or committed into the repository during checkpoint commits (`git add -A`).

3. **Enum Alias Standardization**:
   - In `crates/superstate-cli/tests/test_cli_daemon_guard.rs`, standardized the enum pattern match to `Commands::Daemon` throughout the file and removed redundant `Subcommands` import.

4. **Regression & Integration Tests**:
   - Added `test_workspace_fingerprint_excludes_agents_dir` in `crates/superstate-core/tests/test_daemon_guard.rs` asserting that adding `.agents/hooks.json` does not alter the directory's `workspace_fingerprint`.
   - Added `test_daemon_runner_provisions_hooks_json_appends_to_git_exclude` in `crates/superstate-core/tests/test_daemon_guard.rs` verifying `.agents/` is automatically appended to `.git/info/exclude`.

## 3. Verification & TDD Evidence

1. **RED Phase**:
   - Exported `workspace_fingerprint` as `pub fn` and added the regression tests before updating `FINGERPRINT_SKIP_DIRS` or git exclude logic.
   - Ran `cargo test --test test_daemon_guard`:
     - `test_workspace_fingerprint_excludes_agents_dir` failed as expected: `assertion left == right failed: left: "|1|dac61bd6b3259db8", right: "|2|3615183cbdf21b5b"`
     - `test_daemon_runner_provisions_hooks_json_appends_to_git_exclude` failed as expected: `.git/info/exclude must contain .agents/ after provisioning`.
2. **GREEN Phase**:
   - Added `".agents"` to `FINGERPRINT_SKIP_DIRS` in `daemon.rs`.
   - Added git info exclude check and append logic in `DaemonRunner::step()` when provisioning `hooks.json`.
   - Standardized `Commands::Daemon` in `test_cli_daemon_guard.rs`.
   - Ran `cargo test --test test_daemon_guard`: all 7 tests passed.
   - Ran `cargo test --test test_cli_daemon_guard`: all 2 tests passed.
3. **Static Analysis & Full Workspace Verification**:
   - `cargo clippy --all-targets -- -D warnings`: Completed with exit code 0 and 0 warnings.
   - `cargo test --all`: All unit, integration, and doc tests across all crates in the workspace passed cleanly.

## 4. Summary of Changes

| File | Status | Description |
|---|---|---|
| `crates/superstate-core/src/daemon.rs` | Modified | Added `.agents` to `FINGERPRINT_SKIP_DIRS`, made `workspace_fingerprint` pub, added `.git/info/exclude` update |
| `crates/superstate-core/tests/test_daemon_guard.rs` | Modified | Added regression test for fingerprint exclusion and integration test for git exclude |
| `crates/superstate-cli/tests/test_cli_daemon_guard.rs` | Modified | Standardized enum match on `Commands::Daemon` |
