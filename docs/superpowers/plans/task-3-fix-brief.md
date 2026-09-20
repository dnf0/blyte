# Task 3 Fix Brief: Address Reviewer Findings

## Context & Commits
- **Working Directory:** `/Users/danielfisher/repos/superstate/.worktrees/feat-pre-tool-use-file-guard`
- **Base Commit:** `97e5839`

## Reviewer Findings to Address

### 1. Fingerprint Directory Exclusion
In `crates/superstate-core/src/daemon.rs`:
Add `".agents"` to `FINGERPRINT_SKIP_DIRS`:
```rust
const FINGERPRINT_SKIP_DIRS: [&str; 5] = [
    ".git",
    ".agent-state",
    ".agents",
    ".pytest_cache",
    "__pycache__",
];
```
This ensures that creating `.agents/hooks.json` does not cause false progress detection on turn 1.

### 2. Git Exclude for Ephemeral Hooks
When provisioning `.agents/hooks.json`, if `.git/info` exists, append `.agents/` to `.git/info/exclude` if not already present. This ensures ephemeral daemon hooks are not inadvertently committed into the repository during checkpoint commits.

### 3. Enum Alias Standardization
In `crates/superstate-cli/tests/test_cli_daemon_guard.rs`:
Standardize the enum match on `Commands::Daemon` throughout the file.

### 4. Regression Test
Add a test in `crates/superstate-core/tests/test_daemon_guard.rs` asserting that adding `.agents/hooks.json` does NOT change `workspace_fingerprint` of the directory.

## Steps
1. Apply fixes in `daemon.rs` and `test_cli_daemon_guard.rs`.
2. Add regression test in `test_daemon_guard.rs`.
3. Run `cargo test --all` and `cargo clippy --all-targets -- -D warnings`.
4. Commit using `git commit --no-gpg-sign -m "fix(daemon): exclude .agents from tree fingerprint and git checkpointing"`.
5. Write fix report to `docs/superpowers/plans/task-3-fix-report.md`.
