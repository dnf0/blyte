# Design Spec: Robust Diff Extraction and Blocking Code Review

**Date**: 2026-09-17  
**Status**: Draft  
**Target Repository**: `superstate` (`crates/superstate-core`, `crates/superstate-review`, `crates/superstate-cli`)  

---

## 1. Context & Motivation

In benchmark EXP-056, `superstate_daemon_review` was evaluated against `superstate_mcp` across the 14 discriminating core tasks. The run was halted at 18 runs (9 tasks) in a dead heat (5/9 vs 5/9, 1-1 discordant). Root cause analysis of runtime traces and workspace logs revealed that the code review engine was dormant on all 9 tasks due to two distinct defects:

1. **Empty Diff on Committed Work**: `get_git_diff(&working_dir, Some("HEAD"))` runs `git diff HEAD`. When a worker agent commits its changes (e.g., `git commit -m "fix: ..."`), the working tree is clean. `git diff HEAD` outputs zero bytes. `execute_review` receives an empty diff, detects no changes, and immediately returns `ReviewReport::clean("No changes detected in diff.")`. The reviewer LLM was never invoked on the committed work.
2. **Review Restricted to Vacuous Gates**: In `crates/superstate-core/src/daemon.rs`, line 685 only rejected milestone completion `if is_vacuous_gate(verify_cmd)`. When a non-vacuous shell gate was present (all 14 core tasks), review findings were strictly advisory (logged to `state.notes`) and could never reject completion or trigger an iterative fix turn.

---

## 2. Goals & Non-Goals

### Goals
- **Full Change Coverage**: The review engine must evaluate 100% of the code written during a milestone, regardless of whether changes are uncommitted in the working tree, staged, or already committed across one or multiple git commits.
- **Enforceable Quality Gate**: The daemon must support a `--review-blocking` mode where Critical and High severity review findings prevent milestone completion and trigger a retry turn, even if a simplistic shell gate passed.
- **Deterministic & Zero Prompt Engineering**: The feedback passed to the worker must be the exact, structured findings from the review report (`file`, `problem`, `suggested_fix`), formatted like a standard compiler or test failure without behavioral prompt manipulation.
- **Zero API/Network Keys in Binary**: Local CLI drivers (`agy` or `claude`) continue to be used via subprocess execution.

### Non-Goals
- Altering the benchmark's held-out grader or scoring mechanism.
- Injecting prompt contracts (`SUPERSTATE_EVAL_PROMPT_CONTRACTS=0` remains strictly enforced).

---

## 3. Architecture & Design

### Component 1: `DaemonRunner` Milestone Base Commit Tracking

In `crates/superstate-core/src/daemon.rs`:
1. Add `milestone_start_commits: HashMap<String, String>` to `DaemonRunner`.
2. When a milestone first starts execution (or on turn 1), query and record the current git commit:
   ```rust
   let current_head = get_git_head_commit(&working_dir)?;
   self.milestone_start_commits.entry(task_id.clone()).or_insert(current_head);
   ```
3. When computing the diff for code review at milestone completion:
   - Let `start_commit = self.milestone_start_commits.get(&task_id)`.
   - Query `current_head`.
   - If `current_head != start_commit`:
     - Run `git diff <start_commit>..HEAD` to capture all commits authored during this milestone.
     - Append any uncommitted working tree changes (`git diff HEAD` + untracked files).
   - If `current_head == start_commit` (or no commits were made):
     - Run `git diff HEAD` + untracked files (working tree diff).
4. If a milestone is marked complete, remove its entry from `milestone_start_commits`.

### Component 2: Robust Diff Extraction in `superstate-review` and CLI

In `crates/superstate-review/src/diff.rs`:
- Enhance `get_git_diff`:
  - When `commit_range` is provided (e.g. `Some("abc..def")` or `Some("HEAD~1")`), execute `git diff <range>` and, if diffing against working tree, append untracked files.
  - Add helper `get_milestone_diff(repo_root: &Path, start_commit: Option<&str>) -> Result<String, DiffError>`:
    - If `start_commit` is provided and different from HEAD, diff `start_commit..HEAD` plus uncommitted changes.
    - If working tree is clean and `start_commit` is None, check `HEAD~1..HEAD` or `state.base_branch...HEAD` if available before returning empty.

In `crates/superstate-cli/src/commands/review.rs`:
- Update `cmd_review` to use `get_milestone_diff` or inspect `HEAD~1..HEAD` if working tree is clean and no range was explicitly passed.

### Component 3: `--review-blocking` Mode in `superstate-core` Daemon

In `crates/superstate-core/src/daemon.rs`:
1. Add `pub review_blocking: bool` to `DaemonConfig` (default `false`).
2. Add CLI flag `--review-blocking` to `superstate state daemon`.
3. In `DaemonRunner::step`:
   - When `gate_res.passed` is true and `(self.config.advisory_review || self.config.review_blocking)` is true:
     - Run code review on `get_milestone_diff`.
     - Record findings to `state.notes`.
     - **Enforcement Logic**:
       ```rust
       let should_block = report.has_blocking_issues && (
           self.config.review_blocking || is_vacuous_gate(verify_cmd)
       );
       
       if should_block && self.config.driver_cmd.is_some() && attempts_so_far < attempt_ceiling {
           *self.total_attempts.entry(active_task.id.clone()).or_insert(0) += 1;
           return Ok(DaemonStepOutcome::Failed {
               task_id: active_task.id,
               reason: format!("code-review: {} blocking issue(s) detected", report.findings.len()),
               retries_remaining: self.config.max_retries.saturating_sub(attempts_so_far),
           });
       }
       ```
   - When `should_block` triggers, the daemon marks the step failed. In the subsequent driver turn, the daemon's retry prompt automatically includes the failure reason and recent ledger note containing the exact review findings (`file`, `problem`, `suggested_fix`).

---

## 4. Test Strategy

1. **Unit Tests in `superstate-review`**:
   - Test `get_milestone_diff` with committed changes only (clean working tree).
   - Test `get_milestone_diff` with uncommitted changes only.
   - Test `get_milestone_diff` with mixed committed + uncommitted changes.
   - Test `get_milestone_diff` with untracked files.
2. **Integration Tests in `superstate-core`**:
   - Test `DaemonRunner` with `--advisory-review` (non-blocking on non-vacuous gate): verify note is recorded and milestone completes.
   - Test `DaemonRunner` with `--review-blocking` on non-vacuous gate: verify blocking findings cause `DaemonStepOutcome::Failed` and trigger a retry turn.
   - Test `DaemonRunner` with `--review-blocking` where review passes clean: verify milestone completes immediately.
3. **CLI Tests in `superstate-cli`**:
   - Test `superstate state daemon --review-blocking`.
   - Test `superstate review` against committed changes without `--range`.
