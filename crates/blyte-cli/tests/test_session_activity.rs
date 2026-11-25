use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use blyte_cli::session::{Entry, EntryBody, PromptRunner, Session};

type Flag = Arc<(Mutex<bool>, Condvar)>;

struct ScriptedRunner {
    outcomes: Mutex<Vec<Result<(), String>>>,
    gate: Option<Flag>,
    started: Option<Flag>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl ScriptedRunner {
    fn ok_all() -> (Self, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                outcomes: Mutex::new(Vec::new()),
                gate: None,
                started: None,
                seen: Arc::clone(&seen),
            },
            seen,
        )
    }

    fn with_outcomes(outcomes: Vec<Result<(), String>>) -> (Self, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                outcomes: Mutex::new(outcomes),
                gate: None,
                started: None,
                seen: Arc::clone(&seen),
            },
            seen,
        )
    }

    fn gated_first() -> (Self, Flag, Flag, Arc<Mutex<Vec<String>>>) {
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                outcomes: Mutex::new(Vec::new()),
                gate: Some(Arc::clone(&gate)),
                started: Some(Arc::clone(&started)),
                seen: Arc::clone(&seen),
            },
            started,
            gate,
            seen,
        )
    }
}

impl PromptRunner for ScriptedRunner {
    fn run(
        &mut self,
        prompt: &str,
        _model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        let first_call = {
            let mut seen = self.seen.lock().unwrap();
            let is_first = seen.is_empty();
            seen.push(prompt.to_string());
            is_first
        };
        if first_call {
            if let Some(ref started) = self.started {
                let (lock, cv) = &**started;
                *lock.lock().unwrap() = true;
                cv.notify_all();
            }
            if let Some(ref gate) = self.gate {
                let (lock, cv) = &**gate;
                let mut open = lock.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
            }
        }
        let outcome = {
            let mut outcomes = self.outcomes.lock().unwrap();
            if outcomes.is_empty() {
                Ok(())
            } else {
                outcomes.remove(0)
            }
        };
        if outcome.is_ok() {
            emit(EntryBody::Turn {
                n: 1,
                text: format!("ok: {prompt}"),
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: None,
            });
        }
        outcome
    }
}

fn wait_for_idle_count(session: &Session, expected: usize) -> Vec<Entry> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut after = 0;
    let mut all = Vec::new();
    while Instant::now() < deadline {
        let batch = session.entries_after(after, Duration::from_millis(50));
        for entry in batch {
            after = entry.seq;
            all.push(entry);
        }
        let idle_count = all
            .iter()
            .filter(|e| matches!(e.body, EntryBody::Idle { .. }))
            .count();
        if idle_count >= expected {
            return all;
        }
    }
    panic!("timed out waiting for {expected} idle entries; got {all:?}");
}

#[test]
fn sending_a_message_calls_hook_with_some_then_none() {
    let (runner, _seen) = ScriptedRunner::ok_all();
    let session = Session::start(Box::new(runner), "test-model".to_string());

    let calls: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let hook_calls = Arc::clone(&calls);
    let session_for_hook = session.clone();
    session.set_activity_hook(Box::new(move |prompt| {
        let _ = session_for_hook.info();
        hook_calls
            .lock()
            .unwrap()
            .push(prompt.map(ToString::to_string));
    }));

    session.submit("refactor the parser", Vec::new()).unwrap();
    wait_for_idle_count(&session, 1);

    let recorded = calls.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec![Some("refactor the parser".to_string()), None]
    );
}

#[test]
fn runner_error_still_calls_hook_with_none() {
    let (runner, _seen) = ScriptedRunner::with_outcomes(vec![Err("boom".to_string())]);
    let session = Session::start(Box::new(runner), "test-model".to_string());

    let calls: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let hook_calls = Arc::clone(&calls);
    session.set_activity_hook(Box::new(move |prompt| {
        hook_calls
            .lock()
            .unwrap()
            .push(prompt.map(ToString::to_string));
    }));

    session.submit("do failing work", Vec::new()).unwrap();
    let entries = wait_for_idle_count(&session, 1);
    assert!(entries
        .iter()
        .any(|e| matches!(&e.body, EntryBody::Error { message } if message == "boom")));

    let recorded = calls.lock().unwrap().clone();
    assert_eq!(recorded, vec![Some("do failing work".to_string()), None]);
}

#[test]
fn two_queued_messages_produce_interleaved_some_and_none_calls() {
    let (runner, started, gate, _seen) = ScriptedRunner::gated_first();
    let session = Session::start(Box::new(runner), "test-model".to_string());

    let calls: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let hook_calls = Arc::clone(&calls);
    session.set_activity_hook(Box::new(move |prompt| {
        hook_calls
            .lock()
            .unwrap()
            .push(prompt.map(ToString::to_string));
    }));

    session.submit("first message", Vec::new()).unwrap();
    {
        let (lock, cv) = &*started;
        let mut ready = lock.lock().unwrap();
        while !*ready {
            ready = cv.wait(ready).unwrap();
        }
    }
    session.submit("second message", Vec::new()).unwrap();
    {
        let (lock, cv) = &*gate;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }

    wait_for_idle_count(&session, 2);

    let recorded = calls.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec![
            Some("first message".to_string()),
            None,
            Some("second message".to_string()),
            None,
        ]
    );
}

#[test]
fn panicking_hook_on_first_call_does_not_stop_second_message() {
    let (runner, seen) = ScriptedRunner::ok_all();
    let session = Session::start(Box::new(runner), "test-model".to_string());

    let panicked_once = Arc::new(AtomicBool::new(false));
    let calls: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let hook_panicked = Arc::clone(&panicked_once);
    let hook_calls = Arc::clone(&calls);
    session.set_activity_hook(Box::new(move |prompt| {
        if !hook_panicked.swap(true, Ordering::SeqCst) {
            panic!("intentional hook panic");
        }
        hook_calls
            .lock()
            .unwrap()
            .push(prompt.map(ToString::to_string));
    }));

    session.submit("first", Vec::new()).unwrap();
    wait_for_idle_count(&session, 1);

    session.submit("second", Vec::new()).unwrap();
    wait_for_idle_count(&session, 2);

    assert_eq!(
        seen.lock().unwrap().clone(),
        vec!["first".to_string(), "second".to_string()]
    );
    assert_eq!(
        calls.lock().unwrap().clone(),
        vec![None, Some("second".to_string()), None]
    );
}
