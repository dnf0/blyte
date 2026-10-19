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
    ProviderError::Api {
        status: 429,
        body: body.into(),
    }
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
    let not_429 = ProviderError::Api {
        status: 500,
        body: "usage_limit_reached".into(),
    };
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
    fn model_id(&self) -> String {
        "quota".into()
    }
}

#[test]
fn a_run_stops_on_the_first_quota_error_without_retrying() {
    let d = tempfile::tempdir().unwrap();
    let p = Quota {
        calls: Mutex::new(0),
    };
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
        research: false,
        claim_check: false,
    };
    let mut st = State::new("g", "true");
    let (outcome, report) = runner.run(&mut st);
    match outcome {
        Outcome::Error(e) => {
            assert!(
                e.starts_with("usage quota exhausted; not retrying (resets in 9d 7h)"),
                "{e}"
            );
            assert!(e.contains("429"), "{e}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
    assert_eq!(report.provider_retries, 0);
    assert_eq!(*p.calls.lock().unwrap(), 1);
}
