# Provider Retry Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** A transient provider failure (rate limit, overload, server error, dropped connection) is waited out with backoff instead of ending the run.

**Why:** The first `superstate[conversational]` eval run (2026-09-23) died after 53 seconds and 28 calls with `provider returned 429: RESOURCE_EXHAUSTED`. The same key answered normally seconds later, so it was a burst rate limit, not an exhausted quota. The loop returns `Outcome::Error` on **any** provider error, so one 429 is fatal. Larger-context runs (`conversational`, `folded`) send more tokens per minute and will hit this more often. agy survives the same limits.

**Design:**
- `is_transient`: `Api` with status 429/500/502/503/504, or `Transport`.
- Retry up to 6 times with backoff 2, 4, 8, 16, 32 and 60 seconds.
- Never sleep past the run's deadline. If the next wait would reach it, fail now with the error.
- Every retry is logged to stderr, which the eval driver's event log captures, and counted in `RunReport.provider_retries`, which the `--report` JSON carries.
- Non-transient errors fail at once, as today.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/folded-context`. The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back. A parallel task changes `context.rs`, `cli.rs` and `task_runner.rs`; this task must not touch those files.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` is known to be timing-flaky: if it alone fails, re-run once and report both results.
- Existing tests must pass unmodified, except for adding the new `provider_retries` field to the one `RunReport { … }` literal named below.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Retry transient provider errors with backoff

**Files:**
- Modify: `crates/superstate-agent/src/run.rs`, `crates/superstate-cli/src/report.rs`, `crates/superstate-cli/tests/test_run_report.rs` (the `RunReport { … }` literal only)
- Test: `crates/superstate-agent/tests/test_provider_retry.rs` (create)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_provider_retry.rs`:

```rust
//! Transient provider failures are waited out; everything else fails at once; and no wait ever
//! outlasts the run's deadline.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use superstate_agent::context::Conversational;
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{is_transient, retry_delay, Budget, Outcome, Runner};
use superstate_agent::shell::LocalShell;
use superstate_agent::state::State;
use superstate_provider::{Provider, ProviderError, Request, Response};

/// Fails with each queued error in turn, then answers "done".
struct Flaky {
    failures: Mutex<Vec<ProviderError>>,
    calls: Mutex<u32>,
    always: Option<u16>,
}

impl Flaky {
    fn new(failures: Vec<ProviderError>) -> Self {
        Self {
            failures: Mutex::new(failures),
            calls: Mutex::new(0),
            always: None,
        }
    }
    fn always(status: u16) -> Self {
        Self {
            always: Some(status),
            ..Self::new(Vec::new())
        }
    }
}

impl Provider for Flaky {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        *self.calls.lock().unwrap() += 1;
        if let Some(status) = self.always {
            return Err(ProviderError::Api {
                status,
                body: "RESOURCE_EXHAUSTED".into(),
            });
        }
        let mut f = self.failures.lock().unwrap();
        if !f.is_empty() {
            return Err(f.remove(0));
        }
        Ok(Response {
            text: "done".into(),
            ..Default::default()
        })
    }
    fn model_id(&self) -> String {
        "flaky".into()
    }
}

fn runner<'a>(p: &'a Flaky, root: &std::path::Path, max: Option<Duration>) -> Runner<'a> {
    Runner {
        provider: p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: root.to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: max,
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    }
}

fn api(status: u16) -> ProviderError {
    ProviderError::Api {
        status,
        body: "x".into(),
    }
}

#[test]
fn transient_errors_are_recognised() {
    for s in [429, 500, 502, 503, 504] {
        assert!(is_transient(&api(s)), "{s}");
    }
    for s in [400, 401, 403, 404] {
        assert!(!is_transient(&api(s)), "{s}");
    }
    assert!(is_transient(&ProviderError::Transport("reset".into())));
    assert!(!is_transient(&ProviderError::Parse("bad json".into())));
    assert!(!is_transient(&ProviderError::MissingKey("GEMINI_API_KEY")));
}

#[test]
fn the_backoff_doubles_to_a_one_minute_ceiling() {
    let secs: Vec<u64> = (1..=7).map(|a| retry_delay(a).as_secs()).collect();
    assert_eq!(secs, vec![2, 4, 8, 16, 32, 60, 60]);
}

#[test]
fn a_rate_limit_is_waited_out() {
    let d = tempfile::tempdir().unwrap();
    let p = Flaky::new(vec![api(429)]);
    let mut st = State::new("g", "true");
    let (outcome, report) = runner(&p, d.path(), None).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.provider_retries, 1);
    assert_eq!(*p.calls.lock().unwrap(), 2);
}

#[test]
fn a_bad_request_fails_at_once() {
    let d = tempfile::tempdir().unwrap();
    let p = Flaky::new(vec![api(400)]);
    let mut st = State::new("g", "true");
    let (outcome, report) = runner(&p, d.path(), None).run(&mut st);
    match outcome {
        Outcome::Error(e) => assert!(e.contains("400"), "{e}"),
        other => panic!("expected an error, got {other:?}"),
    }
    assert_eq!(report.provider_retries, 0);
    assert_eq!(*p.calls.lock().unwrap(), 1);
}

#[test]
fn no_wait_outlasts_the_deadline() {
    let d = tempfile::tempdir().unwrap();
    let p = Flaky::always(429);
    let mut st = State::new("g", "true");
    let started = Instant::now();
    let (outcome, report) = runner(&p, d.path(), Some(Duration::from_secs(1))).run(&mut st);
    match outcome {
        Outcome::Error(e) => assert!(e.contains("429"), "{e}"),
        other => panic!("expected an error, got {other:?}"),
    }
    assert_eq!(report.provider_retries, 0, "the first 2s wait would pass a 1s deadline");
    assert!(started.elapsed() < Duration::from_secs(5));
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_provider_retry`. Expected: compile errors, because `is_transient`, `retry_delay` and `provider_retries` do not exist.

- [ ] **Step 3: Classification, schedule, counter.** In `crates/superstate-agent/src/run.rs`:
  1. Change `use superstate_provider::{Message, Provider, Request, ToolCall, Usage};` to `use superstate_provider::{Message, Provider, ProviderError, Request, ToolCall, Usage};`.
  2. In `pub struct RunReport`, add as its last field:

```rust
    /// Transient provider failures (rate limits, overload) waited out during the run.
    pub provider_retries: u32,
```

  3. Directly above `pub struct Runner<'a> {` (and above any doc comment or preceding item you insert before it, such as `PHASE_MARKER_TOOL` if present, placing this block before that one), add:

```rust
/// Retries allowed for one model call before a transient failure ends the run.
pub const MAX_PROVIDER_RETRIES: u32 = 6;

/// Transient provider failures worth waiting out: rate limits, overload, server errors and
/// dropped connections. Anything else (a malformed request, a bad key) fails the run at once.
pub fn is_transient(e: &ProviderError) -> bool {
    match e {
        ProviderError::Api { status, .. } => matches!(status, 429 | 500 | 502 | 503 | 504),
        ProviderError::Transport(_) => true,
        _ => false,
    }
}

/// Wait before retry `attempt` (1-based): 2, 4, 8, 16, 32, then 60 seconds.
pub fn retry_delay(attempt: u32) -> Duration {
    let exp = attempt.saturating_sub(1).min(5);
    Duration::from_secs((2u64 << exp).min(60))
}
```

- [ ] **Step 4: Retry in the loop.** In `Runner::run_until`, replace

```rust
            let resp = match self.provider.complete(&req) {
                Ok(r) => r,
                Err(e) => return (Outcome::Error(e.to_string()), report),
            };
```

     with

```rust
            // A burst rate limit ended a run after 28 calls (2026-09-23). Transient failures are
            // waited out, but never past the deadline: a run that cannot finish in time reports.
            let mut attempt = 0;
            let resp = loop {
                match self.provider.complete(&req) {
                    Ok(r) => break r,
                    Err(e) if is_transient(&e) && attempt < MAX_PROVIDER_RETRIES => {
                        attempt += 1;
                        let wait = retry_delay(attempt);
                        if deadline.is_some_and(|d| std::time::Instant::now() + wait >= d) {
                            return (Outcome::Error(e.to_string()), report);
                        }
                        report.provider_retries += 1;
                        eprintln!(
                            "provider: {e}; retrying in {}s ({attempt}/{MAX_PROVIDER_RETRIES})",
                            wait.as_secs()
                        );
                        std::thread::sleep(wait);
                    }
                    Err(e) => return (Outcome::Error(e.to_string()), report),
                }
            };
```

- [ ] **Step 4b: Sum it across phases.** In `Runner::run_phases`, directly after the line `total.gate_attempts += report.gate_attempts;`, add `total.provider_retries += report.provider_retries;`.

- [ ] **Step 5: Carry it in the report.** In `crates/superstate-cli/src/report.rs`:
  1. In `struct RunRecord`, add `pub provider_retries: u32,` directly after `pub rollbacks: u32,`.
  2. In `RunRecord::base`, add `provider_retries: 0,` directly after `rollbacks: 0,`.
  3. In `RunRecord::from_run`, add `provider_retries: report.provider_retries,` directly after `rollbacks: state.runtime.rollbacks,`.

  In `crates/superstate-cli/tests/test_run_report.rs`, in the `RunReport { … }` literal inside `a_completed_run_maps_every_field`, add `provider_retries: 0,` as the last field.

- [ ] **Step 6: Run the new tests.** Run: `cargo test -p superstate-agent --test test_provider_retry`. Expected: 5 passed. `a_rate_limit_is_waited_out` takes about 2 seconds, because it sleeps through one real backoff.

- [ ] **Step 7: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [ ] **Step 8: Commit.**

```bash
git add crates/superstate-agent/ crates/superstate-cli/
git commit --no-gpg-sign -m "fix(agent): wait out transient provider failures instead of ending the run"
```
