//! Tests for Gemini Vertex AI provider mode, authentication, URL generation,
//! effort suffixes, token caching, and pricing.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blyte_provider::gemini::{vertex_url, Gemini};
use blyte_provider::registry::{detect_provider_kind, ProviderKind};
use blyte_provider::{pricing, resolve_provider, Message, Provider, ProviderError, Request, Usage};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn clear_vertex_env() {
    std::env::remove_var("GOOGLE_CLOUD_PROJECT");
    std::env::remove_var("CLOUDSDK_CORE_PROJECT");
    std::env::remove_var("GOOGLE_CLOUD_LOCATION");
    std::env::remove_var("GEMINI_API_KEY");
    std::env::remove_var("GOOGLE_API_KEY");
    std::env::remove_var("BLYTE_MODEL");
}

#[test]
fn test_detect_provider_kind_vertex() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_vertex_env();

    assert_eq!(
        detect_provider_kind(Some("vertex/gemini-4-argon")),
        ProviderKind::Gemini
    );
    assert_eq!(
        detect_provider_kind(Some("vertex/gemini-4-argon-high")),
        ProviderKind::Gemini
    );
    assert_eq!(detect_provider_kind(Some("vertex")), ProviderKind::Gemini);
}

#[test]
fn test_resolve_provider_vertex_missing_key() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_vertex_env();

    match resolve_provider(Some("vertex/gemini-4-argon")) {
        Err(ProviderError::MissingKey(key)) => assert_eq!(key, "GOOGLE_CLOUD_PROJECT"),
        Ok(_) => panic!("expected MissingKey(GOOGLE_CLOUD_PROJECT), got Ok"),
        Err(e) => panic!("expected MissingKey(GOOGLE_CLOUD_PROJECT), got: {e}"),
    }

    match resolve_provider(Some("vertex")) {
        Err(ProviderError::MissingKey(key)) => assert_eq!(key, "GOOGLE_CLOUD_PROJECT"),
        Ok(_) => panic!("expected MissingKey(GOOGLE_CLOUD_PROJECT), got Ok"),
        Err(e) => panic!("expected MissingKey(GOOGLE_CLOUD_PROJECT), got: {e}"),
    }
}

#[test]
fn test_resolve_provider_cloudsdk_core_project_fallback() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_vertex_env();
    std::env::set_var("CLOUDSDK_CORE_PROJECT", "fallback-proj");

    let p = resolve_provider(Some("vertex/gemini-4-argon")).unwrap();
    assert_eq!(p.model_id(), "vertex/gemini-4-argon");

    clear_vertex_env();
}

#[test]
fn test_vertex_url_pure_function() {
    assert_eq!(
        vertex_url("p", "global", "gemini-4-argon"),
        "https://aiplatform.googleapis.com/v1/projects/p/locations/global/publishers/google/models/gemini-4-argon:generateContent"
    );
    assert_eq!(
        vertex_url("p", "us-central1", "gemini-4-argon"),
        "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/us-central1/publishers/google/models/gemini-4-argon:generateContent"
    );
}

#[test]
fn test_vertex_effort_suffix() {
    let g = Gemini::new_vertex("p", "global").with_model("gemini-4-argon-high");

    // puts gemini-4-argon in the URL
    assert_eq!(
        g.endpoint_url(),
        "https://aiplatform.googleapis.com/v1/projects/p/locations/global/publishers/google/models/gemini-4-argon:generateContent"
    );

    // sets the thinking level as today
    assert_eq!(g.thinking_level(), Some("high"));
    assert_eq!(g.effort(), Some("high".to_string()));

    let req = Request {
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    };
    let b = g.body(&req);
    assert_eq!(
        b["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "high"
    );
}

#[test]
fn test_vertex_model_id() {
    let g = Gemini::new_vertex("p", "global").with_model("gemini-4-argon");
    assert_eq!(g.model_id(), "vertex/gemini-4-argon");

    let g_high = Gemini::new_vertex("p", "global").with_model("gemini-4-argon-high");
    assert_eq!(g_high.model_id(), "vertex/gemini-4-argon");
}

#[test]
fn test_vertex_pricing() {
    let p = pricing("vertex/gemini-4-argon").expect("known price");
    assert_eq!(p.input, 2.00);
    assert_eq!(p.cached, 0.10);
    assert_eq!(p.output, 10.00);

    let cost = p.cost(&Usage {
        input: 1_000_000,
        cached: 1_000_000,
        output: 1_000_000,
        thinking: 500_000,
    });
    // (1M * 2.00 + 1M * 0.10 + (1M + 0.5M) * 10.00) / 1e6 = (2 + 0.10 + 15) = 17.10
    assert!((cost - 17.10).abs() < 1e-6);
}

#[test]
fn test_vertex_token_cache() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = call_count.clone();

    let fake_token_source = move || {
        let n = count_clone.fetch_add(1, Ordering::SeqCst);
        Ok(format!("mock-token-{n}"))
    };

    let start = Instant::now();
    let current_time = Arc::new(Mutex::new(start));
    let time_clone = current_time.clone();
    let clock = move || *time_clone.lock().unwrap();

    let g = Gemini::new_vertex("p", "global")
        .with_model("gemini-4-argon")
        .with_token_source(fake_token_source)
        .with_clock(clock);

    // Initial request: fetches once
    let t1 = g.get_token().unwrap();
    assert_eq!(t1, "mock-token-0");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Second request within 45 minutes (e.g. 10 minutes later): reuses cached token
    *current_time.lock().unwrap() = start + Duration::from_secs(10 * 60);
    let t2 = g.get_token().unwrap();
    assert_eq!(t2, "mock-token-0");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Third request at 44 minutes: still within 45 minutes
    *current_time.lock().unwrap() = start + Duration::from_secs(44 * 60);
    let t3 = g.get_token().unwrap();
    assert_eq!(t3, "mock-token-0");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Request after 45 minutes (46 minutes later): cache has expired, fetches again
    *current_time.lock().unwrap() = start + Duration::from_secs(46 * 60);
    let t4 = g.get_token().unwrap();
    assert_eq!(t4, "mock-token-1");
    assert_eq!(call_count.load(Ordering::SeqCst), 2);

    // Subsequent request within new 45 minute window: reuses new token
    *current_time.lock().unwrap() = start + Duration::from_secs(50 * 60);
    let t5 = g.get_token().unwrap();
    assert_eq!(t5, "mock-token-1");
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[test]
fn test_vertex_token_cache_invalidate_on_401() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = call_count.clone();

    let g = Gemini::new_vertex("p", "global").with_token_source(move || {
        let n = count_clone.fetch_add(1, Ordering::SeqCst);
        Ok(format!("token-{n}"))
    });

    let t1 = g.get_token().unwrap();
    assert_eq!(t1, "token-0");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Simulate 401: invalidation causes next fetch to get a new token
    g.invalidate_token();

    let t2 = g.get_token().unwrap();
    assert_eq!(t2, "token-1");
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[test]
fn test_vertex_token_failure_error_message() {
    let g = Gemini::new_vertex("p", "global").with_token_source(|| {
        Err(ProviderError::Auth(
            "failed to obtain access token; run `gcloud auth application-default login`"
                .to_string(),
        ))
    });

    let err = g.get_token().unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("gcloud auth application-default login"),
        "error message should instruct running login: {msg}"
    );
}

#[test]
fn test_vertex_from_env_custom_location() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_vertex_env();
    std::env::set_var("GOOGLE_CLOUD_PROJECT", "custom-proj");
    std::env::set_var("GOOGLE_CLOUD_LOCATION", "europe-west4");

    let g = Gemini::vertex_from_env().unwrap();
    assert_eq!(g.location(), Some("europe-west4"));
    assert_eq!(g.project(), Some("custom-proj"));
    assert_eq!(
        g.endpoint_url(),
        "https://europe-west4-aiplatform.googleapis.com/v1/projects/custom-proj/locations/europe-west4/publishers/google/models/gemini-3.8-flash:generateContent"
    );

    clear_vertex_env();
}
