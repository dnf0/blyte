use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::{Output, Shell};
use blyte_agent::state::State;
use blyte_provider::openai::OpenAi;

struct NoopShell;

impl Shell for NoopShell {
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

#[test]
fn transport_timeout_is_retried_and_fails_after_retry_limit() {
    unsafe {
        std::env::set_var("BLYTE_MODEL_TIMEOUT_SECS", "1");
        std::env::set_var("BLYTE_RETRY_DELAY_MS", "5");
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);

    let server_handle = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let mut held = Vec::new();
        while !stop_thread.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    held.push(stream);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
        drop(held);
    });

    unsafe {
        std::env::set_var("OPENAI_BASE_URL", format!("http://{addr}/v1"));
        std::env::set_var("OPENAI_API_KEY", "test-key");
    }

    let provider = OpenAi::openai_from_env().unwrap();
    let d = tempfile::tempdir().unwrap();

    let runner = Runner {
        provider: &provider,
        shell: &NoopShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: Some(Duration::from_millis(60)),
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
        base_check: blyte_agent::run::BaseCheck::None,
    };

    let mut st = State::new("goal", "true");
    let frozen = Instant::now();
    let (outcome, report) = runner.run_with_clock(&mut st, &|| frozen);

    stop.store(true, Ordering::Relaxed);
    let _ = server_handle.join();

    assert!(
        matches!(outcome, Outcome::Error(_)),
        "Expected Outcome::Error on transport timeout: {outcome:?}"
    );
    assert_eq!(
        report.provider_retries, 6,
        "Expected 6 provider retries before failing"
    );
}
