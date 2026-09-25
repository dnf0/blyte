//! Resilience: server retry hints and model pricing.

use std::time::Duration;

use blyte_agent::run::{retry_delay, retry_wait, server_retry_hint};
use blyte_provider::{pricing, ProviderError, Usage};

fn api(status: u16, body: &str) -> ProviderError {
    ProviderError::Api {
        status,
        body: body.into(),
    }
}

#[test]
fn the_server_retry_hint_is_honoured_and_bounded() {
    let body = r#"{"error": {"code": 429, "details": [{"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "37s"}]}}"#;
    assert_eq!(
        server_retry_hint(&api(429, body)),
        Some(Duration::from_secs(37))
    );
    assert_eq!(retry_wait(&api(429, body), 1), Duration::from_secs(37));
    // A hint shorter than the backoff does not shorten it.
    let short = r#"{"retryDelay": "1s"}"#;
    assert_eq!(retry_wait(&api(429, short), 3), retry_delay(3));
    // No hint: the backoff.
    assert_eq!(retry_wait(&api(503, "overloaded"), 2), retry_delay(2));
    assert_eq!(
        server_retry_hint(&ProviderError::Transport("reset".into())),
        None
    );
    // An absurd hint is capped.
    let long = r#"{"retryDelay": "900s"}"#;
    assert_eq!(
        server_retry_hint(&api(429, long)),
        Some(Duration::from_secs(120))
    );
}

#[test]
fn gemini_flash_is_priced_with_thinking_billed_as_output() {
    let p = pricing("gemini/gemini-3.8-flash").expect("a known price");
    let u = Usage {
        input: 1_000_000,
        cached: 1_000_000,
        output: 100_000,
        thinking: 100_000,
    };
    assert!((p.cost(&u) - 1.575).abs() < 1e-9, "{}", p.cost(&u));
    assert_eq!(pricing("scripted"), None);
}
