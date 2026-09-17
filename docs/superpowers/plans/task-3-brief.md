# Task 3 Brief: Daemon Advisory Review Loop in `crates/superstate-core`

- **Repository Directory**: `/Users/danielfisher/repos/superstate`
- **Current Branch**: `feat/advisory-code-review`
- **Depends on:** Task 1, Task 2

## Goal
Wire advisory code review into the background daemon loop in `crates/superstate-core/src/daemon.rs` and expose `--advisory-review` in `superstate daemon`.
When a milestone gate passes:
- If `--advisory-review` is enabled, extract the git diff for the milestone and run `superstate_review::execute_review`.
- If blocking issues (`Critical` or `High`) are detected:
  - Record an advisory ledger note:
    `[review-advisory] Task <id>: <summary>. Finding: <problem> (<suggested_fix>)`
  - If the verification gate was vacuous (`is_vacuous_gate(verify_cmd)`):
    Hold the task open for another driver turn (up to `UNGATED_MIN_DRIVER_TURNS` or attempt ceiling) by returning `DaemonStepOutcome::Failed { task_id, reason: "advisory-review: blocking issues detected on vacuous gate", retries_remaining }` so the driver receives the critique in `state prompt` and can fix the issue rather than accepting an uncertified completion.
  - If the verification gate was non-vacuous (a strict verifier):
    Record the advisory note to the ledger for downstream diagnostic context, but allow the milestone to complete (avoiding the 60.5% benchmark false-alarm trap).

## Files to Touch
- Modify: `crates/superstate-core/Cargo.toml` (add `superstate-review = { path = "../superstate-review" }`)
- Modify: `crates/superstate-core/src/daemon.rs`
- Modify: `crates/superstate-cli/src/state_cli.rs`
- Create: `crates/superstate-core/tests/test_daemon_advisory_review.rs`

## Detailed Requirements & Interfaces
1. In `crates/superstate-core/Cargo.toml`:
   Add dependency:
   ```toml
   superstate-review = { path = "../superstate-review" }
   ```
2. In `crates/superstate-core/src/daemon.rs`:
   - In `DaemonConfig`:
     Add field: `pub advisory_review: bool`.
     In `Default for DaemonConfig`, default `advisory_review: false`.
   - In `DaemonRunner::step`:
     After:
     ```rust
     let gate_res = run_verification_cmd(verify_cmd, &working_dir, self.config.timeout_secs)?;
     ```
     Around lines 606 (where `if gate_res.passed` is handled):
     If `gate_res.passed && self.config.advisory_review && working_dir.join(".git").exists()`:
     - Compute the git diff of changes made for this milestone (e.g. `superstate_review::get_git_diff(&working_dir, None)`).
     - Run `superstate_review::execute_review(&working_dir, &diff, None, &ReviewConfig { driver: self.config.driver_cmd.clone(), timeout_secs: self.config.timeout_secs })`.
     - If the review succeeds and `report.has_blocking_issues`:
       - Construct advisory note:
         ```rust
         let first_finding = report.findings.first().map(|f| format!("{}: {}", f.file.display(), f.problem)).unwrap_or_else(|| report.summary.clone());
         let note = format!("[review-advisory] Task {}: {}. Problem: {}", active_task.id, report.summary, first_finding);
         state.notes.push(LedgerEntry::new("superstate-daemon", note));
         save_state_atomic(&self.config.state_path, &state)?;
         ```
       - If `is_vacuous_gate(verify_cmd)` and `self.config.driver_cmd.is_some()`:
         - Check attempt ceiling and turn floor:
           Hold task open:
           ```rust
           return Ok(DaemonStepOutcome::Failed {
               task_id: active_task.id,
               reason: "advisory-review: blocking issues detected on vacuous gate".to_string(),
               retries_remaining: self.config.max_retries.saturating_sub(attempts_so_far),
           });
           ```
3. In `crates/superstate-cli/src/state_cli.rs`:
   - In `DaemonArgs`:
     Add flag:
     ```rust
     /// Run advisory adversarial code review on milestone completion
     #[arg(long)]
     pub advisory_review: bool,
     ```
   - In daemon runner construction (`dispatch_daemon` / `DaemonConfig` initialization):
     Pass `advisory_review: args.advisory_review`.

4. Tests (`crates/superstate-core/tests/test_daemon_advisory_review.rs`):
   - Test daemon step with `advisory_review: true` on clean diff (proceeds without failure).
   - Test daemon step with `advisory_review: true` with a mock driver reporting blocking issues on a vacuous gate (`verify_cmd: "true"`), confirming the advisory note is appended to `state.notes` and the step returns `DaemonStepOutcome::Failed`.
   - Test daemon step with `advisory_review: true` with a mock driver reporting blocking issues on a non-vacuous gate (`verify_cmd: "exit 0"` or custom command), confirming the note is recorded and milestone completes.

## TDD Instructions
- Step 1: Write failing tests in `crates/superstate-core/tests/test_daemon_advisory_review.rs`.
- Step 2: Run `cargo test -p superstate-core --test test_daemon_advisory_review` (verify FAIL).
- Step 3: Implement `advisory_review` in `Cargo.toml`, `daemon.rs`, and `state_cli.rs`.
- Step 4: Run `cargo test --all` (verify PASS).
- Step 5: Commit changes with message `feat(daemon): wire advisory review into milestone gate evaluation`.
