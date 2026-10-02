use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::openai::OpenAi;

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
        while !stop_thread.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((_stream, _)) => {
                    // Hold connection open without replying so client times out
                    std::thread::sleep(Duration::from_millis(1200));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    unsafe {
        std::env::set_var("OPENAI_BASE_URL", format!("http://{addr}/v1"));
        std::env::set_var("OPENAI_API_KEY", "test-key");
    }

    let provider = OpenAi::openai_from_env().unwrap();
    let d = tempfile::tempdir().unwrap();

    let runner = Runner {
        provider: &provider,
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
    };

    let mut st = State::new("goal", "true");
    let (outcome, report) = runner.run(&mut st);

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
