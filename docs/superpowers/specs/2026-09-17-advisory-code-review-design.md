# Design Document: Advisory Code Review & Ledger Integration

**Date**: 2026-09-17  
**Status**: Approved  
**Author**: Antigravity & Pair Developer  
**Scope**: `crates/superstate-review`, `crates/superstate-cli`, `crates/superstate-core`

---

## 1. Context & Motivation

In `FINDINGS.md` §9 and the rebuttal sequence, empirical evaluation established:
1. **The 17%-precision gate failure mode**: When milestone gates assert only necessary conditions (e.g. exit 0, file exists), they pass 100% of runs where the real verifier (the sealed grader) fails.
2. **Review effectiveness**: Adversarial static LLM code review over the git candidate diff catches **88.9% (16 of 18)** of these false passes on repository tasks (DataCurve).
3. **The false-alarm risk**: When review is treated as an uncalibrated hard stop (`has_blocking_issues == true`), it incurs a **60.5% false alarm rate** on viable solutions because LLM reviewers enforce strict production engineering cleanliness rather than benchmark-minimal assertions.

Therefore, adversarial code review in `superstate` must be **advisory and loop-integrated**:
- It inspects the candidate diff against requirements and architectural contracts.
- Findings are emitted cleanly (human-readable or JSON) and can be recorded into the epistemic ledger (`.agent-state/state.json`) as `[review-advisory]` notes.
- In `superstate daemon`, advisory review surfaces actionable feedback directly to the worker agent during retries, holding vacuous/weak gates open rather than terminating prematurely.

---

## 2. Architecture & Components

```
                                      ┌──────────────────────┐
                                      │   Task Instruction   │
                                      │  (or prompt context) │
                                      └──────────┬───────────┘
                                                 │
┌───────────────────────────┐                    │
│ Working Tree / Git Diff   ├────────────────────┼──────────────┐
└─────────────┬─────────────┘                    │              │
              │                                  ▼              │
              ▼                       ┌──────────────────────┐  │
 ┌─────────────────────────┐          │  superstate-review   │  │
 │     get_git_diff()      ├─────────►│     runner.rs        │  │
 └─────────────────────────┘          └──────────┬───────────┘  │
                                                 │              │
                                                 ▼              │
                                      ┌──────────────────────┐  │
                                      │   CLI Driver (agy/   │  │
                                      │    claude preset)    │  │
                                      └──────────┬───────────┘  │
                                                 │              │
                                                 ▼              │
                                      ┌──────────────────────┐  │
                                      │     ReviewReport     │  │
                                      │  (findings, severities) │
                                      └──────────┬───────────┘  │
                                                 │              │
               ┌─────────────────────────────────┴──────────────┼────────────────┐
               ▼                                                ▼                ▼
     ┌───────────────────┐                           ┌────────────────────┐ ┌─────────────┐
     │  CLI / JSON Output│                           │ Epistemic Ledger   │ │ Daemon Loop │
     │  (--json / stdout)│                           │ (state.json notes) │ │ (hold open  │
     └───────────────────┘                           └─────────┬──────────┘ │ if vacuous) │
                                                               │            └─────────────┘
                                                               ▼
                                                     ┌────────────────────┐
                                                     │    state prompt    │
                                                     │ (subagent context) │
                                                     └────────────────────┘
```

---

## 3. Detailed Specifications

### 3.1 `crates/superstate-review`

#### Schema Updates (`crates/superstate-review/src/schema.rs`)
Preserve existing `Severity` (`Critical`, `High`, `Medium`, `Low`) and `ReviewFinding`. Add serialization/deserialization helpers and display formatting:
```rust
impl fmt::Display for ReviewReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Summary: {}", self.summary)?;
        writeln!(f, "Blocking Issues: {}", self.has_blocking_issues)?;
        for finding in &self.findings {
            writeln!(f, "- [{:?}] {}: {}-{}", finding.severity, finding.file.display(), finding.line_range.0, finding.line_range.1)?;
            writeln!(f, "  Problem: {}", finding.problem)?;
            writeln!(f, "  Suggested Fix: {}", finding.suggested_fix)?;
        }
        Ok(())
    }
}
```

#### Review Runner (`crates/superstate-review/src/runner.rs`)
Implement the execution engine invoking the driver subprocess:
```rust
pub struct ReviewConfig {
    pub driver: Option<String>,
    pub timeout_secs: u64,
}

pub fn execute_review(
    repo_root: &Path,
    diff: &str,
    task_instruction: Option<&str>,
    config: &ReviewConfig,
) -> Result<ReviewReport, ReviewError>;
```
- **Driver Resolution**: Auto-detects in order:
  1. Explicit `config.driver` argument.
  2. `SUPERSTATE_REVIEW_DRIVER` or `SUPERSTATE_DRIVER` environment variable.
  3. Auto-detected `agy` in PATH (`agy --model gemini-3.8-flash-low --print --dangerously-skip-permissions`).
  4. Auto-detected `claude` in PATH (`claude -p --dangerously-skip-permissions`).
- **Prompt Construction**: Uses `build_review_system_prompt()` with the diff enclosed in markdown fences, capped at a safe boundary (120,000 characters).
- **Safe JSON Extraction**: Strips markdown code fences (` ```json ... ``` `), parses dictionary or list responses, promotes severity to `has_blocking_issues = true` if `Critical` or `High` findings are present.

### 3.2 CLI Interface: `superstate review` (`crates/superstate-cli/src/commands/review.rs`)

The `superstate review` command accepts:
- `commit_range: Option<String>`: Git commit range (e.g. `HEAD~1..HEAD`). If `None`, computes the diff of unstaged and staged changes in the working directory against `HEAD`.
- `--repo <PathBuf>`: Path to repository root (defaults to `.`).
- `--driver <String>`: Override driver command.
- `--state-path <PathBuf>`: Path to `.agent-state/state.json`.
- `--record`: When set, automatically appends any `Critical` or `High` findings into the state ledger as:
  `[review-advisory] <problem> (Fix: <suggested_fix>)`
- `--json`: Output `ReviewReport` JSON to stdout.
- `--fail-on-blocking`: Exit with non-zero exit code if `has_blocking_issues` is true (for CI integration).

### 3.3 Daemon Advisory Integration (`crates/superstate-core/src/daemon.rs`)

In `DaemonRunner::step`:
- Add `advisory_review: bool` to `DaemonConfig`.
- Expose `--advisory-review` CLI flag in `superstate daemon`.
- **Milestone Evaluation Hook**:
  - When a milestone's `run_verification_cmd` passes:
    - If `advisory_review` is enabled:
      - Extract the git diff of changes made for the active milestone.
      - Invoke `execute_review`.
      - If `has_blocking_issues` is true:
        - Record note to state ledger:
          `[review-advisory] Milestone {task_id}: {finding.problem}`
        - If the verification gate was vacuous (`is_vacuous_gate(verify_cmd)`):
          - Hold the task open for another driver turn (up to `UNGATED_MIN_DRIVER_TURNS` or max retries). The next prompt generated via `render_turnkey_prompt` will include the advisory finding from the ledger, giving the subagent actionable corrective guidance.
        - If the verification gate was non-vacuous (strict test):
          - Note is recorded for future milestones and diagnostics, but milestone is allowed to complete (avoiding false-alarm deadlock).

---

## 4. Error Handling & Edge Cases

1. **Empty Diff**: If no changes exist in the working tree/range, returns `ReviewReport::clean("No changes detected in diff.")` immediately without calling the driver.
2. **Missing Driver**: If neither `agy`, `claude`, nor an explicit `--driver` is present, returns an actionable `ReviewError::DriverNotFound` instructing the user how to configure a driver.
3. **Malformed LLM Output**: Strips code blocks, parses raw lists as findings, and handles non-JSON error returns gracefully without crashing.
4. **Large Diffs**: Diffs exceeding 120,000 characters are safely truncated with an explanatory warning in the prompt.

---

## 5. Verification Plan

1. **Unit Tests**:
   - `crates/superstate-review/tests/test_runner.rs`: Driver resolution, prompt formatting, JSON extraction, error paths.
2. **CLI Tests**:
   - `crates/superstate-cli/tests/test_cli_review.rs`: `superstate review --json`, `superstate review --record`, and `--fail-on-blocking`.
3. **Daemon Integration Tests**:
   - `crates/superstate-core/tests/test_daemon_advisory_review.rs`: Verifies that advisory review appends notes to ledger and holds vacuous gates open.
4. **Regression**:
   - `cargo test --all` across the entire workspace passes with zero warnings.
