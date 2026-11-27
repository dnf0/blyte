//! Transient provider failures are waited out; everything else fails at once; and no wait ever
//! outlasts the run's deadline.

use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{is_transient, retry_delay, Budget, Outcome, Runner};
use blyte_agent::shell::{Output, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

static RETRY_ENV_LOCK: Mutex<()> = Mutex::new(());

struct RetryDelayGuard {
    _lock: MutexGuard<'static, ()>,
    prev: Option<OsString>,
}

impl RetryDelayGuard {
    fn set(val: Option<&str>) -> Self {
        let lock = RETRY_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("BLYTE_RETRY_DELAY_MS");
        unsafe {
            match val {
                Some(v) => std::env::set_var("BLYTE_RETRY_DELAY_MS", v),
                None => std::env::remove_var("BLYTE_RETRY_DELAY_MS"),
            }
        }
        Self { _lock: lock, prev }
    }
}

impl Drop for RetryDelayGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var("BLYTE_RETRY_DELAY_MS", v),
                None => std::env::remove_var("BLYTE_RETRY_DELAY_MS"),
            }
        }
    }
}

struct FastShell;

impl Shell for FastShell {
    fn exec(&self, _command: &str, _cwd: &Path, _timeout: Duration) -> Result<Output, String> {
        Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    fn describe(&self) -> String {
        String::new()
    }
}

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
        shell: &FastShell,
        fs: &LocalFs,
        context: &Conversational,
        root: root.to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: max,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
        base_check: blyte_agent::run::BaseCheck::None,
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
    assert!(is_transient(&ProviderError::NoAnswer {
        block_reason: Some("SAFETY".into()),
        block_reason_message: None,
        usage: Default::default(),
    }));
    assert!(!is_transient(&ProviderError::Parse("bad json".into())));
    assert!(!is_transient(&ProviderError::MissingKey("GEMINI_API_KEY")));
}

#[test]
fn the_backoff_doubles_to_a_one_minute_ceiling() {
    let _g = RetryDelayGuard::set(None);
    let secs: Vec<u64> = (1..=7).map(|a| retry_delay(a).as_secs()).collect();
    assert_eq!(secs, vec![2, 4, 8, 16, 32, 60, 60]);
}

#[test]
fn a_rate_limit_is_waited_out() {
    let _g = RetryDelayGuard::set(Some("1"));
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
    let _g = RetryDelayGuard::set(Some("60000"));
    let d = tempfile::tempdir().unwrap();
    let p = Flaky::always(429);
    let mut st = State::new("g", "true");
    let started = Instant::now();
    let (outcome, report) = runner(&p, d.path(), Some(Duration::from_secs(10))).run(&mut st);
    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert_eq!(
        report.provider_retries, 0,
        "a 60s wait would pass a 10s deadline"
    );
    assert_eq!(*p.calls.lock().unwrap(), 1);
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the run must not sleep the 60s wait past the 10s deadline, took {:?}",
        started.elapsed()
    );
}

fn no_answer(input: u64) -> ProviderError {
    ProviderError::NoAnswer {
        block_reason: Some("SAFETY".into()),
        block_reason_message: Some("The prompt is blocked due to safety".into()),
        usage: blyte_provider::Usage {
            input,
            ..Default::default()
        },
    }
}

#[test]
fn no_answer_is_retried_once_and_recovers() {
    let _g = RetryDelayGuard::set(Some("1"));
    let d = tempfile::tempdir().unwrap();
    let p = Flaky::new(vec![no_answer(300)]);
    let mut st = State::new("g", "true");
    let (outcome, report) = runner(&p, d.path(), None).run(&mut st);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.provider_retries, 1);
    assert_eq!(*p.calls.lock().unwrap(), 2);
    assert_eq!(report.usage.input, 300);
}

#[test]
fn no_answer_fails_after_one_retry() {
    let _g = RetryDelayGuard::set(Some("1"));
    let d = tempfile::tempdir().unwrap();
    let p = Flaky::new(vec![no_answer(300), no_answer(400), no_answer(500)]);
    let mut st = State::new("g", "true");
    let (outcome, report) = runner(&p, d.path(), None).run(&mut st);
    match outcome {
        Outcome::Error(e) => {
            assert!(e.contains("no answer"), "{e}");
            assert!(e.contains("SAFETY"), "{e}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
    assert_eq!(report.provider_retries, 1);
    assert_eq!(*p.calls.lock().unwrap(), 2);
    assert_eq!(report.usage.input, 700);
}
