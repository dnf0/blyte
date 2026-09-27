# Quota Fails Fast, Login Errors Say So — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. Do not read `~/.codex/auth.json` or any other real credential file. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** (1) A 429 that means an exhausted **usage quota** (not a burst rate limit) ends the run at
once with a clear message, instead of six backoff retries. (2) ChatGPT login problems (unreadable
`auth.json`, missing or expired token) are reported as authentication errors, not as "could not parse
provider response".

**Why (2026-09-27):** the ChatGPT-provider live test on a free-plan login got
`429 {"error":{"type":"usage_limit_reached",...,"resets_in_seconds":805604}}` and blyte retried six
times over ~2 minutes for a quota that resets in 9 days. Earlier, an expired Codex login surfaced as
"could not parse provider response: ChatGPT login token expired…", which reads like a bug in blyte.

**Tech Stack:** Rust.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**, with one named exception: in
  `crates/blyte-provider/tests/test_chatgpt.rs`, `a_token_about_to_expire_is_refused_with_how_to_refresh_it`
  changes its match arm from `ProviderError::Parse(m)` to `ProviderError::Auth(m)` (nothing else in it).
- A plain 429 (for example Gemini's burst `RESOURCE_EXHAUSTED` with a `retryDelay`) stays transient
  and is still retried exactly as today (`test_provider_retry.rs` checks this).
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `quota_exhausted` and `ProviderError::Auth`

**What to build** (names are part of the contract; the tests below check them):

`crates/blyte-provider/src/lib.rs`:
- New variant `ProviderError::Auth(String)` with `#[error("authentication: {0}")]`.

`crates/blyte-provider/src/chatgpt.rs`, `from_codex_home`: every error about the login itself returns
`ProviderError::Auth` instead of `ProviderError::Parse`: unparseable `auth.json`, missing `tokens`,
missing or empty `access_token`, malformed JWT, undecodable payload, missing `exp`, expiring token,
missing account id. Keep each message's text (the expiry message keeps
"run any codex command to refresh your ChatGPT login"). A missing or unreadable file stays
`MissingKey("CODEX_HOME")`. Response parsing (`parse_sse` and friends) stays `Parse`. Update the doc
comment of `from_codex_home` accordingly.

`crates/blyte-agent/src/run.rs`:
- `pub fn quota_exhausted(e: &ProviderError) -> Option<String>`: `Some(message)` only for
  `ProviderError::Api { status: 429, body }` where `body` contains any of `usage_limit_reached`,
  `insufficient_quota` or `PerDay` (Gemini's daily-quota ids, e.g. `GenerateRequestsPerDayPerProjectPerModel`),
  or contains `"resets_in_seconds":` followed (after optional spaces) by an integer greater than 300.
  The message is `usage quota exhausted; not retrying`, plus ` (resets in <human>)` when
  `resets_in_seconds` is present, where `<human>` is `<d>d <h>h` for ≥ 1 day, `<h>h <m>m` for ≥ 1 hour,
  else `<m>m`. Parse the number by hand (no regex crate).
- `is_transient` returns false whenever `quota_exhausted(e).is_some()`; otherwise unchanged.
- In the provider-call loop, the non-transient error arm returns
  `Outcome::Error(format!("{msg}: {e}"))` when `quota_exhausted(&e)` gives `Some(msg)`, and
  `Outcome::Error(e.to_string())` otherwise (as today).

- [ ] **Step 1: Write the failing tests.** Create `crates/blyte-agent/tests/test_quota_exhausted.rs`:

```rust
//! An exhausted usage quota ends the run at once: a free-plan ChatGPT login returned
//! usage_limit_reached (resets in 9 days) and blyte retried six times over two minutes (2026-09-27).

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{is_transient, quota_exhausted, Budget, Outcome, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

fn e429(body: &str) -> ProviderError {
    ProviderError::Api { status: 429, body: body.into() }
}

const CHATGPT_FREE: &str = r#"{"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","plan_type":"free","resets_at":1791301338,"limit_window_minutes":43200,"resets_in_seconds":805604}}"#;

#[test]
fn quota_bodies_are_recognised_and_not_transient() {
    for body in [
        CHATGPT_FREE,
        r#"{"error":{"code":"insufficient_quota"}}"#,
        r#"{"error":{"status":"RESOURCE_EXHAUSTED","details":[{"quotaId":"GenerateRequestsPerDayPerProjectPerModel"}]}}"#,
        r#"{"error":{"type":"rate_limit","resets_in_seconds": 3600}}"#,
    ] {
        let e = e429(body);
        assert!(quota_exhausted(&e).is_some(), "{body}");
        assert!(!is_transient(&e), "{body}");
    }
}

#[test]
fn burst_rate_limits_stay_transient() {
    for body in [
        "RESOURCE_EXHAUSTED",
        r#"{"error":{"status":"RESOURCE_EXHAUSTED","details":[{"retryDelay":"37s"}]}}"#,
        r#"{"error":{"type":"rate_limit","resets_in_seconds":30}}"#,
    ] {
        let e = e429(body);
        assert!(quota_exhausted(&e).is_none(), "{body}");
        assert!(is_transient(&e), "{body}");
    }
    let not_429 = ProviderError::Api { status: 500, body: "usage_limit_reached".into() };
    assert!(quota_exhausted(&not_429).is_none());
}

#[test]
fn the_message_says_when_the_quota_resets() {
    let m = quota_exhausted(&e429(CHATGPT_FREE)).unwrap();
    assert!(m.starts_with("usage quota exhausted; not retrying"), "{m}");
    assert!(m.contains("(resets in 9d 7h)"), "{m}");
    let m = quota_exhausted(&e429(r#"{"resets_in_seconds":5400}"#)).unwrap();
    assert!(m.contains("(resets in 1h 30m)"), "{m}");
}

struct Quota {
    calls: Mutex<u32>,
}

impl Provider for Quota {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        *self.calls.lock().unwrap() += 1;
        Err(e429(CHATGPT_FREE))
    }
}

#[test]
fn a_run_stops_on_the_first_quota_error_without_retrying() {
    let d = tempfile::tempdir().unwrap();
    let p = Quota { calls: Mutex::new(0) };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
    };
    let mut st = State::new("g", "true");
    let (outcome, report) = runner.run(&mut st);
    match outcome {
        Outcome::Error(e) => {
            assert!(e.starts_with("usage quota exhausted; not retrying (resets in 9d 7h)"), "{e}");
            assert!(e.contains("429"), "{e}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
    assert_eq!(report.provider_retries, 0);
    assert_eq!(*p.calls.lock().unwrap(), 1);
}
```

Add to `crates/blyte-provider/tests/test_chatgpt.rs` a test that an `auth.json` with no `tokens`
object gives `Err(ProviderError::Auth(_))`, and make the one named change to the expiry test.

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_quota_exhausted`: compile error (`quota_exhausted` missing).

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "fix(agent): an exhausted usage quota stops the run at once; ChatGPT login problems are authentication errors"
```
