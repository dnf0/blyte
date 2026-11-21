use std::collections::{BTreeSet, VecDeque};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use blyte_agent::approval::{Approval, Approver};
use blyte_cli::session::{
    AnswerError, ApprovalAnswer, Attachment, Entry, EntryBody, PromptRunner, Session,
};

enum ScriptAction {
    Ok(Vec<EntryBody>),
    Err {
        emit: Vec<EntryBody>,
        message: String,
    },
    Panic {
        emit: Vec<EntryBody>,
        message: String,
    },
    Block {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
        emit: Vec<EntryBody>,
    },
}

struct RecordedCall {
    prompt: String,
    model: String,
}

struct ScriptedRunner {
    actions: VecDeque<ScriptAction>,
    calls: Arc<Mutex<Vec<RecordedCall>>>,
    dropped: Option<mpsc::Sender<()>>,
}

impl ScriptedRunner {
    fn new(actions: Vec<ScriptAction>) -> (Self, Arc<Mutex<Vec<RecordedCall>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runner = Self {
            actions: actions.into(),
            calls: Arc::clone(&calls),
            dropped: None,
        };
        (runner, calls)
    }

    fn with_drop_signal(
        actions: Vec<ScriptAction>,
        dropped: mpsc::Sender<()>,
    ) -> (Self, Arc<Mutex<Vec<RecordedCall>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runner = Self {
            actions: actions.into(),
            calls: Arc::clone(&calls),
            dropped: Some(dropped),
        };
        (runner, calls)
    }
}

impl Drop for ScriptedRunner {
    fn drop(&mut self) {
        if let Some(tx) = self.dropped.take() {
            let _ = tx.send(());
        }
    }
}

impl PromptRunner for ScriptedRunner {
    fn run(
        &mut self,
        prompt: &str,
        model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        self.calls.lock().unwrap().push(RecordedCall {
            prompt: prompt.to_string(),
            model: model.to_string(),
        });
        match self
            .actions
            .pop_front()
            .unwrap_or(ScriptAction::Ok(Vec::new()))
        {
            ScriptAction::Ok(entries) => {
                for body in entries {
                    emit(body);
                }
                Ok(())
            }
            ScriptAction::Err {
                emit: entries,
                message,
            } => {
                for body in entries {
                    emit(body);
                }
                Err(message)
            }
            ScriptAction::Panic {
                emit: entries,
                message,
            } => {
                for body in entries {
                    emit(body);
                }
                panic!("{}", message);
            }
            ScriptAction::Block {
                entered,
                release,
                emit: entries,
            } => {
                let _ = entered.send(());
                let _ = release.recv();
                for body in entries {
                    emit(body);
                }
                Ok(())
            }
        }
    }
}

fn wait_for_entries(session: &Session, expected_count: usize) -> Vec<Entry> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut all = Vec::new();
    while all.len() < expected_count {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for {expected_count} entries, got {}: {:?}",
            all.len(),
            all
        );
        let after = all.last().map(|e: &Entry| e.seq).unwrap_or(0);
        let more = session.entries_after(after, remaining);
        all.extend(more);
    }
    all
}

#[test]
fn entries_are_numbered_without_gaps() {
    fn assert_handle_traits<T: Clone + Send + Sync>() {}
    assert_handle_traits::<Session>();

    let (runner, _) = ScriptedRunner::new(vec![ScriptAction::Ok(vec![
        EntryBody::Turn {
            n: 1,
            text: "Checking the repository".into(),
            input_tokens: 120,
            output_tokens: 45,
            cost_usd: Some(0.01),
        },
        EntryBody::ToolStarted {
            name: "read".into(),
            input: "Cargo.toml".into(),
        },
        EntryBody::ToolFinished {
            name: "read".into(),
            summary: "ok (20 lines)".into(),
            is_error: false,
        },
    ])]);

    let session = Session::start(Box::new(runner), "gemini-4-neon".into());
    let seq = session.submit("hello", Vec::new()).unwrap();
    assert_eq!(seq, 1);

    let entries = wait_for_entries(&session, 5);
    assert_eq!(entries.len(), 5);
    let seqs: Vec<u64> = entries.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4, 5]);
    assert!(entries.iter().all(|e| e.ts > 0));

    assert!(matches!(
        entries[0].body,
        EntryBody::UserMessage {
            ref text,
            ref reports,
            queued: false
        } if text == "hello" && reports.is_empty()
    ));
    assert!(matches!(entries[1].body, EntryBody::Turn { n: 1, .. }));
    assert!(matches!(entries[2].body, EntryBody::ToolStarted { .. }));
    assert!(matches!(entries[3].body, EntryBody::ToolFinished { .. }));
    assert!(matches!(
        entries[4].body,
        EntryBody::Idle {
            turns: 1,
            cost_usd: Some(c)
        } if (c - 0.01).abs() < 1e-12
    ));
}

#[test]
fn entry_json_shape() {
    let entry = Entry {
        seq: 7,
        ts: 1_700_000_000,
        body: EntryBody::Turn {
            n: 2,
            text: "done".into(),
            input_tokens: 300,
            output_tokens: 50,
            cost_usd: Some(0.0025),
        },
    };
    let val = serde_json::to_value(&entry).unwrap();
    let obj = val.as_object().unwrap();
    let keys: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = [
        "seq",
        "ts",
        "kind",
        "n",
        "text",
        "input_tokens",
        "output_tokens",
        "cost_usd",
    ]
    .into_iter()
    .collect();
    assert_eq!(keys, expected);
    assert_eq!(obj.get("kind").unwrap(), "turn");
    assert_eq!(obj.get("seq").unwrap(), 7);
    assert_eq!(obj.get("ts").unwrap(), 1_700_000_000);
    assert_eq!(obj.get("n").unwrap(), 2);
    assert_eq!(obj.get("text").unwrap(), "done");
    assert_eq!(obj.get("input_tokens").unwrap(), 300);
    assert_eq!(obj.get("output_tokens").unwrap(), 50);
    assert_eq!(obj.get("cost_usd").unwrap(), 0.0025);
}

#[test]
fn messages_sent_while_busy_run_in_order() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (runner, calls) = ScriptedRunner::new(vec![
        ScriptAction::Block {
            entered: entered_tx,
            release: release_rx,
            emit: Vec::new(),
        },
        ScriptAction::Ok(Vec::new()),
        ScriptAction::Ok(Vec::new()),
    ]);

    let session = Session::start(Box::new(runner), "gemini-4-neon".into());
    let s1 = session.submit("first", Vec::new()).unwrap();
    assert_eq!(s1, 1);

    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let s2 = session.submit("second", Vec::new()).unwrap();
    let s3 = session.submit("third", Vec::new()).unwrap();
    assert_eq!(s2, 2);
    assert_eq!(s3, 3);

    let mid_info = session.info();
    assert!(mid_info.busy);
    assert_eq!(mid_info.queued, 2);

    let early = session.entries_after(0, Duration::ZERO);
    assert_eq!(early.len(), 3);
    assert!(matches!(
        early[0].body,
        EntryBody::UserMessage { queued: false, .. }
    ));
    assert!(matches!(
        early[1].body,
        EntryBody::UserMessage { queued: true, .. }
    ));
    assert!(matches!(
        early[2].body,
        EntryBody::UserMessage { queued: true, .. }
    ));

    release_tx.send(()).unwrap();

    let all = wait_for_entries(&session, 6);
    assert_eq!(all.len(), 6);
    let idle_count = all
        .iter()
        .filter(|e| matches!(e.body, EntryBody::Idle { .. }))
        .count();
    assert_eq!(idle_count, 3);

    let final_info = session.info();
    assert!(!final_info.busy);
    assert_eq!(final_info.queued, 0);
    assert_eq!(final_info.last_seq, 6);

    let recorded: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.prompt.clone())
        .collect();
    assert_eq!(recorded, vec!["first", "second", "third"]);
}

#[test]
fn blank_message_is_refused() {
    let (runner, calls) = ScriptedRunner::new(Vec::new());
    let session = Session::start(Box::new(runner), "gemini-4-neon".into());

    assert_eq!(
        session.submit("", Vec::new()),
        Err("a message needs text".to_string())
    );
    assert_eq!(
        session.submit("   \n\t  ", Vec::new()),
        Err("a message needs text".to_string())
    );
    assert_eq!(session.info().last_seq, 0);
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn attachments_are_appended_to_the_prompt() {
    let (runner, calls) = ScriptedRunner::new(vec![ScriptAction::Ok(Vec::new())]);
    let session = Session::start(Box::new(runner), "gemini-4-neon".into());

    session
        .submit(
            "Compare these",
            vec![
                Attachment {
                    run: "run-01".into(),
                    text: "Findings A".into(),
                },
                Attachment {
                    run: "run-02".into(),
                    text: "Findings B".into(),
                },
            ],
        )
        .unwrap();

    let entries = wait_for_entries(&session, 2);
    assert!(matches!(
        &entries[0].body,
        EntryBody::UserMessage { text, reports, queued: false }
            if text == "Compare these" && reports == &vec!["run-01".to_string(), "run-02".to_string()]
    ));

    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].prompt,
        "Compare these\n\nResearch report run-01:\nFindings A\n\nResearch report run-02:\nFindings B"
    );
}

#[test]
fn runner_error_becomes_error_then_idle() {
    let (runner, _) = ScriptedRunner::new(vec![ScriptAction::Err {
        emit: vec![EntryBody::Turn {
            n: 1,
            text: "attempt".into(),
            input_tokens: 10,
            output_tokens: 5,
            cost_usd: Some(0.005),
        }],
        message: "provider returned 503".into(),
    }]);

    let session = Session::start(Box::new(runner), "gemini-4-neon".into());
    session.submit("try it", Vec::new()).unwrap();

    let entries = wait_for_entries(&session, 4);
    assert!(matches!(entries[0].body, EntryBody::UserMessage { .. }));
    assert!(matches!(entries[1].body, EntryBody::Turn { n: 1, .. }));
    assert_eq!(
        entries[2].body,
        EntryBody::Error {
            message: "provider returned 503".into()
        }
    );
    assert!(matches!(
        entries[3].body,
        EntryBody::Idle {
            turns: 1,
            cost_usd: Some(c)
        } if (c - 0.005).abs() < 1e-12
    ));
}

#[test]
fn runner_panic_becomes_error_then_idle_and_the_next_prompt_runs() {
    let (runner, calls) = ScriptedRunner::new(vec![
        ScriptAction::Panic {
            emit: Vec::new(),
            message: "unexpected boom".into(),
        },
        ScriptAction::Ok(vec![EntryBody::Turn {
            n: 1,
            text: "recovered".into(),
            input_tokens: 20,
            output_tokens: 10,
            cost_usd: Some(0.002),
        }]),
    ]);

    let session = Session::start(Box::new(runner), "gemini-4-neon".into());
    session.submit("crash", Vec::new()).unwrap();
    let after_first = wait_for_entries(&session, 3);
    assert_eq!(
        after_first[1].body,
        EntryBody::Error {
            message: "unexpected boom".into()
        }
    );
    assert!(matches!(
        after_first[2].body,
        EntryBody::Idle { turns: 0, .. }
    ));

    session.submit("next", Vec::new()).unwrap();
    let all = wait_for_entries(&session, 6);
    assert!(matches!(
        all[3].body,
        EntryBody::UserMessage { ref text, .. } if text == "next"
    ));
    assert!(matches!(
        all[4].body,
        EntryBody::Turn { ref text, .. } if text == "recovered"
    ));
    assert!(matches!(
        all[5].body,
        EntryBody::Idle {
            turns: 1,
            cost_usd: Some(c)
        } if (c - 0.002).abs() < 1e-12
    ));

    let recorded: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.prompt.clone())
        .collect();
    assert_eq!(recorded, vec!["crash", "next"]);
}

#[test]
fn entries_after_waits_for_a_new_entry() {
    let (runner, _) = ScriptedRunner::new(vec![ScriptAction::Ok(Vec::new())]);
    let session = Session::start(Box::new(runner), "gemini-4-neon".into());

    let empty_start = Instant::now();
    let empty = session.entries_after(0, Duration::from_millis(300));
    let empty_elapsed = empty_start.elapsed();
    assert!(empty.is_empty());
    assert!(
        empty_elapsed >= Duration::from_millis(300),
        "expected >= 300ms wait, got {:?}",
        empty_elapsed
    );

    let last_seq = session.info().last_seq;
    let submitter = session.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        submitter.submit("wake up", Vec::new()).unwrap();
    });

    let wait_start = Instant::now();
    let received = session.entries_after(last_seq, Duration::from_secs(5));
    let wait_elapsed = wait_start.elapsed();
    handle.join().unwrap();

    assert!(!received.is_empty());
    assert!(
        wait_elapsed < Duration::from_secs(1),
        "expected wakeup within 1s, got {:?}",
        wait_elapsed
    );
    assert!(matches!(
        received[0].body,
        EntryBody::UserMessage { ref text, .. } if text == "wake up"
    ));
}

#[test]
fn idle_sums_turn_costs() {
    let (runner, _) = ScriptedRunner::new(vec![ScriptAction::Ok(vec![
        EntryBody::Turn {
            n: 1,
            text: "one".into(),
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: Some(0.0125),
        },
        EntryBody::Turn {
            n: 2,
            text: "two".into(),
            input_tokens: 200,
            output_tokens: 75,
            cost_usd: Some(0.0275),
        },
    ])]);

    let session = Session::start(Box::new(runner), "gemini-4-neon".into());
    session.submit("sum costs", Vec::new()).unwrap();

    let entries = wait_for_entries(&session, 4);
    match entries[3].body {
        EntryBody::Idle {
            turns,
            cost_usd: Some(total),
        } => {
            assert_eq!(turns, 2);
            assert!((total - 0.04).abs() < 1e-12, "got {total}");
        }
        ref other => panic!("expected Idle with summed cost, got {other:?}"),
    }
}

#[test]
fn idle_cost_is_none_when_a_turn_has_no_price() {
    let (runner, _) = ScriptedRunner::new(vec![ScriptAction::Ok(vec![
        EntryBody::Turn {
            n: 1,
            text: "priced".into(),
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: Some(0.01),
        },
        EntryBody::Turn {
            n: 2,
            text: "unpriced".into(),
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: None,
        },
    ])]);

    let session = Session::start(Box::new(runner), "gemini-4-neon".into());
    session.submit("missing price", Vec::new()).unwrap();

    let entries = wait_for_entries(&session, 4);
    assert_eq!(
        entries[3].body,
        EntryBody::Idle {
            turns: 2,
            cost_usd: None
        }
    );
}

#[test]
fn set_effort_rules() {
    let (runner, calls) = ScriptedRunner::new(vec![
        ScriptAction::Ok(Vec::new()),
        ScriptAction::Ok(Vec::new()),
    ]);
    let session = Session::start(Box::new(runner), "gemini-4-neon".into());

    let info0 = session.info();
    assert_eq!(info0.model, "gemini-4-neon");
    assert_eq!(info0.effort, None);
    assert!(info0.effort_supported);

    session.set_effort(Some("high")).unwrap();
    let info1 = session.info();
    assert_eq!(info1.model, "gemini-4-neon");
    assert_eq!(info1.effort.as_deref(), Some("high"));
    assert!(info1.effort_supported);

    let after_high = session.entries_after(0, Duration::ZERO);
    assert_eq!(after_high.len(), 1);
    assert_eq!(
        after_high[0].body,
        EntryBody::EffortChanged {
            effort: Some("high".into())
        }
    );

    session.submit("with high", Vec::new()).unwrap();
    wait_for_entries(&session, 3);

    session.set_effort(None).unwrap();
    let info2 = session.info();
    assert_eq!(info2.model, "gemini-4-neon");
    assert_eq!(info2.effort, None);
    assert!(info2.effort_supported);

    let after_none = session.entries_after(3, Duration::ZERO);
    assert_eq!(after_none.len(), 1);
    assert_eq!(
        after_none[0].body,
        EntryBody::EffortChanged { effort: None }
    );

    session.submit("without effort", Vec::new()).unwrap();
    wait_for_entries(&session, 6);

    let models_seen: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.model.clone())
        .collect();
    assert_eq!(models_seen, vec!["gemini-4-neon-high", "gemini-4-neon"]);

    assert_eq!(
        session.set_effort(Some("turbo")),
        Err("effort must be low, medium or high".to_string())
    );

    let (anth_runner, _) = ScriptedRunner::new(Vec::new());
    let anth_session = Session::start(Box::new(anth_runner), "claude-sonnet-4-6".into());
    assert!(!anth_session.info().effort_supported);
    assert_eq!(
        anth_session.set_effort(Some("high")),
        Err("claude-sonnet-4-6 does not take an effort setting".to_string())
    );
    assert_eq!(
        anth_session.set_effort(Some("xhigh")),
        Err("effort must be low, medium or high".to_string())
    );
}

#[test]
fn set_model_keeps_or_drops_effort() {
    let (runner, calls) = ScriptedRunner::new(vec![
        ScriptAction::Ok(Vec::new()),
        ScriptAction::Ok(Vec::new()),
    ]);
    let session = Session::start(Box::new(runner), "gemini-4-neon".into());

    assert!(session.set_model("   ").is_err());

    session.set_effort(Some("medium")).unwrap();
    session.set_model("gemini-4-argon").unwrap();

    let kept_info = session.info();
    assert_eq!(kept_info.model, "gemini-4-argon");
    assert_eq!(kept_info.effort.as_deref(), Some("medium"));
    assert!(kept_info.effort_supported);

    let entries_after_keep = session.entries_after(0, Duration::ZERO);
    assert_eq!(entries_after_keep.len(), 2);
    assert_eq!(
        entries_after_keep[1].body,
        EntryBody::ModelChanged {
            model: "gemini-4-argon".into()
        }
    );

    session.submit("prompt 1", Vec::new()).unwrap();
    wait_for_entries(&session, 4);

    session.set_model("claude-sonnet-4-6").unwrap();
    let dropped_info = session.info();
    assert_eq!(dropped_info.model, "claude-sonnet-4-6");
    assert_eq!(dropped_info.effort, None);
    assert!(!dropped_info.effort_supported);

    let entries_after_drop = session.entries_after(4, Duration::ZERO);
    assert_eq!(entries_after_drop.len(), 2);
    assert_eq!(
        entries_after_drop[0].body,
        EntryBody::ModelChanged {
            model: "claude-sonnet-4-6".into()
        }
    );
    assert_eq!(
        entries_after_drop[1].body,
        EntryBody::EffortChanged { effort: None }
    );

    session.submit("prompt 2", Vec::new()).unwrap();
    wait_for_entries(&session, 8);

    let models_seen: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.model.clone())
        .collect();
    assert_eq!(
        models_seen,
        vec!["gemini-4-argon-medium", "claude-sonnet-4-6"]
    );
}

#[test]
fn decision_with_a_message_submits_it() {
    let (runner, calls) = ScriptedRunner::new(vec![ScriptAction::Ok(Vec::new())]);
    let session = Session::start(Box::new(runner), "gemini-4-neon".into());

    let seq_without = session
        .record_decision("run-1", "accept", Some("looks good"), None)
        .unwrap();
    assert_eq!(seq_without, 1);

    let only_decision = session.entries_after(0, Duration::ZERO);
    assert_eq!(only_decision.len(), 1);
    assert_eq!(
        only_decision[0].body,
        EntryBody::Decision {
            run: "run-1".into(),
            action: "accept".into(),
            note: Some("looks good".into()),
        }
    );
    assert!(calls.lock().unwrap().is_empty());

    let seq_with = session
        .record_decision(
            "run-2",
            "send_back",
            Some("missing edge case"),
            Some("Please address missing edge case in run-2"),
        )
        .unwrap();
    assert_eq!(seq_with, 2);

    let all = wait_for_entries(&session, 4);
    assert_eq!(
        all[1].body,
        EntryBody::Decision {
            run: "run-2".into(),
            action: "send_back".into(),
            note: Some("missing edge case".into()),
        }
    );
    assert!(matches!(
        &all[2].body,
        EntryBody::UserMessage { text, reports, queued: false }
            if text == "Please address missing edge case in run-2" && reports.is_empty()
    ));
    assert!(matches!(all[3].body, EntryBody::Idle { .. }));

    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].prompt,
        "Please address missing edge case in run-2"
    );
}

#[test]
fn dropping_last_handle_stops_worker_after_prompt_in_progress() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (dropped_tx, dropped_rx) = mpsc::channel();
    let (runner, calls) = ScriptedRunner::with_drop_signal(
        vec![
            ScriptAction::Block {
                entered: entered_tx,
                release: release_rx,
                emit: Vec::new(),
            },
            ScriptAction::Ok(Vec::new()),
        ],
        dropped_tx,
    );

    let session = Session::start(Box::new(runner), "gemini-4-neon".into());
    let clone = session.clone();
    session.submit("in progress", Vec::new()).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    session.submit("queued after drop", Vec::new()).unwrap();

    drop(session);
    assert!(dropped_rx.recv_timeout(Duration::from_millis(100)).is_err());

    drop(clone);
    assert!(dropped_rx.recv_timeout(Duration::from_millis(100)).is_err());

    release_tx.send(()).unwrap();
    dropped_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let recorded: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.prompt.clone())
        .collect();
    assert_eq!(recorded, vec!["in progress"]);
}

#[test]
fn approval_ask_blocks_until_answered_yes_and_no() {
    let (runner, _) = ScriptedRunner::new(Vec::new());
    let session = Session::start(Box::new(runner), "gemini-4-neon".into());

    let approver = session.approver();
    let (done_tx, done_rx) = mpsc::channel();
    let cmd1 = "git push origin main && echo '<script>alert(1)</script>'";
    let reason1 = "publish verified commits";
    let t1 = std::thread::spawn(move || {
        let decision = approver.ask(cmd1, reason1);
        let _ = done_tx.send(decision);
    });

    let req1_entries = wait_for_entries(&session, 1);
    let id1 = match &req1_entries[0].body {
        EntryBody::ApprovalRequested {
            id,
            command,
            reason,
        } => {
            assert_eq!(command, cmd1);
            assert_eq!(reason, reason1);
            id.clone()
        }
        other => panic!("expected ApprovalRequested, got {other:?}"),
    };

    assert!(done_rx.recv_timeout(Duration::from_millis(120)).is_err());

    let ans1_seq = session.answer_approval(&id1, ApprovalAnswer::Yes).unwrap();
    assert_eq!(ans1_seq, 2);
    let outcome1 = done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(outcome1, Approval::Approved);
    t1.join().unwrap();

    let after_ans1 = wait_for_entries(&session, 2);
    assert_eq!(
        after_ans1[1].body,
        EntryBody::ApprovalAnswered {
            id: id1.clone(),
            command: cmd1.to_string(),
            answer: "yes".to_string(),
        }
    );

    let approver2 = session.approver();
    let (done2_tx, done2_rx) = mpsc::channel();
    let t2 = std::thread::spawn(move || {
        let decision = approver2.ask(cmd1, "run a second time");
        let _ = done2_tx.send(decision);
    });

    let req2_entries = wait_for_entries(&session, 3);
    let id2 = match &req2_entries[2].body {
        EntryBody::ApprovalRequested {
            id,
            command,
            reason,
        } => {
            assert_ne!(id, &id1);
            assert_eq!(command, cmd1);
            assert_eq!(reason, "run a second time");
            id.clone()
        }
        other => panic!("expected second ApprovalRequested, got {other:?}"),
    };

    assert!(done2_rx.recv_timeout(Duration::from_millis(120)).is_err());

    let ans2_seq = session.answer_approval(&id2, ApprovalAnswer::No).unwrap();
    assert_eq!(ans2_seq, 4);
    let outcome2 = done2_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(outcome2, Approval::Denied);
    t2.join().unwrap();

    let all = wait_for_entries(&session, 4);
    assert_eq!(
        all[3].body,
        EntryBody::ApprovalAnswered {
            id: id2,
            command: cmd1.to_string(),
            answer: "no".to_string(),
        }
    );
}

#[test]
fn approval_stop_run_and_shutdown_give_denied() {
    let (runner, _) = ScriptedRunner::new(Vec::new());
    let session = Session::start(Box::new(runner), "gemini-4-neon".into());

    let approver = session.approver();
    let (done_tx, done_rx) = mpsc::channel();
    let t1 = std::thread::spawn(move || {
        let _ = done_tx.send(approver.ask("rm -rf /tmp/demo", "clean up"));
    });

    let entries = wait_for_entries(&session, 1);
    let id = match &entries[0].body {
        EntryBody::ApprovalRequested { id, .. } => id.clone(),
        other => panic!("expected ApprovalRequested, got {other:?}"),
    };

    assert!(done_rx.recv_timeout(Duration::from_millis(80)).is_err());

    session.stop_run();
    assert_eq!(
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Approval::Denied
    );
    t1.join().unwrap();

    let after_stop = wait_for_entries(&session, 2);
    assert_eq!(
        after_stop[1].body,
        EntryBody::ApprovalAnswered {
            id: id.clone(),
            command: "rm -rf /tmp/demo".to_string(),
            answer: "no".to_string(),
        }
    );
    assert_eq!(
        session.answer_approval(&id, ApprovalAnswer::Yes),
        Err(AnswerError::AlreadyAnswered)
    );

    let approver_shutdown = session.approver();
    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    let t2 = std::thread::spawn(move || {
        let _ = shutdown_tx.send(approver_shutdown.ask("curl https://example.com", "fetch"));
    });

    wait_for_entries(&session, 3);
    assert!(shutdown_rx.recv_timeout(Duration::from_millis(80)).is_err());

    drop(session);
    assert_eq!(
        shutdown_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Approval::Denied
    );
    t2.join().unwrap();
}

#[test]
fn approval_second_answer_refused_and_first_stands() {
    let (runner, _) = ScriptedRunner::new(Vec::new());
    let session = Session::start(Box::new(runner), "gemini-4-neon".into());

    assert_eq!(
        session.answer_approval("no-such-id", ApprovalAnswer::Yes),
        Err(AnswerError::NotFound)
    );

    let approver = session.approver();
    let (done_tx, done_rx) = mpsc::channel();
    let t = std::thread::spawn(move || {
        let _ = done_tx.send(approver.ask("cargo publish --dry-run", "check package"));
    });

    let entries = wait_for_entries(&session, 1);
    let id = match &entries[0].body {
        EntryBody::ApprovalRequested { id, .. } => id.clone(),
        other => panic!("expected ApprovalRequested, got {other:?}"),
    };

    assert_eq!(session.answer_approval(&id, ApprovalAnswer::Yes), Ok(2));
    assert_eq!(
        session.answer_approval(&id, ApprovalAnswer::No),
        Err(AnswerError::AlreadyAnswered)
    );
    assert_eq!(
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Approval::Approved
    );
    t.join().unwrap();

    let all = session.entries_after(0, Duration::ZERO);
    assert_eq!(all.len(), 2);
    assert_eq!(
        all[1].body,
        EntryBody::ApprovalAnswered {
            id,
            command: "cargo publish --dry-run".to_string(),
            answer: "yes".to_string(),
        }
    );
}

#[test]
fn approval_allow_file_rule_never_creates_pending_approval() {
    struct EnvAndApprovalGuard {
        prev_xdg: Option<std::ffi::OsString>,
        prev_approval: Option<blyte_agent::approval::ApprovalConfig>,
    }

    impl Drop for EnvAndApprovalGuard {
        fn drop(&mut self) {
            blyte_agent::tools::set_approval(self.prev_approval.take());
            match &self.prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    let root = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let allow_path = xdg.path().join("blyte").join("allow-unsandboxed");
    std::fs::create_dir_all(allow_path.parent().unwrap()).unwrap();
    std::fs::write(&allow_path, "echo allowed\n").unwrap();

    let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
    let prev_approval = blyte_agent::tools::approval();
    std::env::set_var("XDG_CONFIG_HOME", xdg.path());
    let _guard = EnvAndApprovalGuard {
        prev_xdg,
        prev_approval,
    };

    let (runner, _) = ScriptedRunner::new(Vec::new());
    let session = Session::start(Box::new(runner), "gemini-4-neon".into());
    let cfg = blyte_cli::repl::ReplConfig {
        repo_root: root.path().to_path_buf(),
        sandbox: "on".to_string(),
        ..Default::default()
    };
    let _agent_runner = blyte_cli::session::AgentRunner::new(&cfg, session.approver());

    let shell = blyte_agent::shell::LocalShell::new();
    let fs = blyte_agent::fs::LocalFs;

    let allowed_out = blyte_agent::tools::execute(
        root.path(),
        &shell,
        &fs,
        "bash",
        &serde_json::json!({
            "command": "echo allowed by file",
            "unsandboxed": true,
            "reason": "covered by allow file",
        }),
    );
    assert!(!allowed_out.is_error, "{}", allowed_out.content);
    assert!(allowed_out.content.contains("allowed by file"));
    assert!(
        session.entries_after(0, Duration::ZERO).is_empty(),
        "allow-file command must not create a pending approval"
    );

    let session_for_answer = session.clone();
    let answer_thread = std::thread::spawn(move || {
        let requested = wait_for_entries(&session_for_answer, 1);
        let id = match &requested[0].body {
            EntryBody::ApprovalRequested {
                id,
                command,
                reason,
            } => {
                assert_eq!(command, "echo needs_manual_approval");
                assert_eq!(reason, "not covered by allow file");
                id.clone()
            }
            other => panic!("expected ApprovalRequested, got {other:?}"),
        };
        session_for_answer
            .answer_approval(&id, ApprovalAnswer::Yes)
            .unwrap();
    });

    let asked_out = blyte_agent::tools::execute(
        root.path(),
        &shell,
        &fs,
        "bash",
        &serde_json::json!({
            "command": "echo needs_manual_approval",
            "unsandboxed": true,
            "reason": "not covered by allow file",
        }),
    );
    answer_thread.join().unwrap();
    assert!(!asked_out.is_error, "{}", asked_out.content);
    assert!(asked_out.content.contains("needs_manual_approval"));

    let entries = session.entries_after(0, Duration::ZERO);
    assert_eq!(entries.len(), 2);
    assert!(matches!(
        &entries[0].body,
        EntryBody::ApprovalRequested { command, .. } if command == "echo needs_manual_approval"
    ));
    assert!(matches!(
        &entries[1].body,
        EntryBody::ApprovalAnswered { command, answer, .. }
            if command == "echo needs_manual_approval" && answer == "yes"
    ));
}
