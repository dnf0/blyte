# Task 3 Brief: CLI Integration & Verification

## Working Directory
`/Users/danielfisher/repos/superstate/.worktrees/feat-daemon-dag-execution`
Branch: `feat/daemon-dag-execution`

## Goal
Verify and ensure seamless CLI integration for arbitrary DAG milestone execution via `superstate state init --milestones <JSON>` and `superstate state daemon`.

## Global Constraints
- Compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No new external crate dependencies.
- Zero network I/O or direct API keys in Rust binary.
- Use `--no-gpg-sign` for git commits.
- Follow strict Test-Driven Development (TDD).

## Requirements

### 1. `crates/superstate-cli/src/commands/state.rs`
- Verify that `cmd_init` correctly parses JSON milestones with `id` and `depends_on` (now supported by `MilestoneSpec` in `superstate-core`).
- Ensure any CLI error reporting when `init_state_milestones` returns `StateError::InvalidTransition` (e.g. initial deadlock) is clean and informative.

### 2. Integration Tests: `crates/superstate-cli/tests/test_cli_dag_daemon.rs`
Write tests covering:
1. `test_cli_diamond_dag_daemon_execution`:
   - Initialize git repo in a temp dir.
   - Run `superstate state init --milestones` with a diamond DAG:
     - `task-a`: creates `a.txt`, verify `test -f a.txt`, depends: `[]`
     - `task-b`: creates `b.txt`, verify `test -f b.txt`, depends: `["task-a"]`
     - `task-c`: creates `c.txt`, verify `test -f c.txt`, depends: `["task-a"]`
     - `task-d`: creates `d.txt`, verify `test -f d.txt`, depends: `["task-b", "task-c"]`
   - Use a mock driver script (checking `$SUPERSTATE_ACTIVE_TASK` to touch the corresponding file).
   - Run `DaemonRunner` (or invoke `superstate state daemon`).
   - Verify state file reaches `status: "completed"` and all 4 tasks are in `completed_tasks`.
   - Verify git history has milestone checkpoint commits for all 4 tasks.
2. `test_cli_deadlocked_dag_daemon_halts_blocked`:
   - Initialize state with a task that depends on a non-existent milestone.
   - Run daemon.
   - Verify state file has `status: "blocked"` and contains a `[dag-blocked]` note in `notes`.

### 3. Build & Install
- Run `cargo install --path crates/superstate-cli --locked --force` so the local binary is updated.
- Verify `superstate state --help` and `superstate state daemon --help` work cleanly.

## Verification
- `cargo test -p superstate-cli --test test_cli_dag_daemon`
- `cargo test --all`
- `cargo check --all`

## Completion
- Commit with: `git commit --no-gpg-sign -m "feat(cli): verify and test diamond DAG daemon execution from CLI"`
- Write detailed report to `docs/superpowers/plans/task-3-report.md`.
