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
            queued: false,
            ..
        } if text == "hello" && reports.is_empty()
    ));
    assert!(matches!(entries[1].body, EntryBody::Turn { n: 1, .. }));
    assert!(matches!(entries[2].body, EntryBody::ToolStarted { .. }));
    assert!(matches!(entries[3].body, EntryBody::ToolFinished { .. }));
    assert!(matches!(
        entries[4].body,
        EntryBody::Idle {
            turns: 1,
            cost_usd: Some(c),
            ..
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
        EntryBody::UserMessage { text, reports, queued: false, .. }
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
            cost_usd: Some(c),
            ..
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
            cost_usd: Some(c),
            ..
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
            ..
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
    assert!(matches!(
        entries[3].body,
        EntryBody::Idle {
            turns: 2,
            cost_usd: None,
            ..
        }
    ));
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
        EntryBody::UserMessage { text, reports, queued: false, .. }
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

static GLOBAL_POLICY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn approval_allow_file_rule_never_creates_pending_approval() {
    struct EnvAndApprovalGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev_xdg: Option<std::ffi::OsString>,
        prev_approval: Option<blyte_agent::approval::ApprovalConfig>,
        prev_scope: Option<blyte_agent::tools::WriteScope>,
    }

    impl Drop for EnvAndApprovalGuard {
        fn drop(&mut self) {
            blyte_agent::tools::set_write_scope(self.prev_scope.take());
            blyte_agent::tools::set_approval(self.prev_approval.take());
            match &self.prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    let lock = GLOBAL_POLICY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let allow_path = xdg.path().join("blyte").join("allow-unsandboxed");
    std::fs::create_dir_all(allow_path.parent().unwrap()).unwrap();
    std::fs::write(&allow_path, "echo allowed\n").unwrap();

    let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
    let prev_approval = blyte_agent::tools::approval();
    let prev_scope = blyte_agent::tools::write_scope();
    std::env::set_var("XDG_CONFIG_HOME", xdg.path());
    let _guard = EnvAndApprovalGuard {
        _lock: lock,
        prev_xdg,
        prev_approval,
        prev_scope,
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

#[test]
fn agent_runner_run_keeps_surface_approver_instead_of_tty() {
    struct PolicyGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev_xdg: Option<std::ffi::OsString>,
        prev_approval: Option<blyte_agent::approval::ApprovalConfig>,
        prev_scope: Option<blyte_agent::tools::WriteScope>,
    }

    impl Drop for PolicyGuard {
        fn drop(&mut self) {
            blyte_agent::tools::set_write_scope(self.prev_scope.take());
            blyte_agent::tools::set_approval(self.prev_approval.take());
            match &self.prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    let lock = GLOBAL_POLICY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp_base = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let root = tempfile::tempdir_in(tmp_base).unwrap();
    let xdg = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root.path())
        .status()
        .unwrap()
        .success());

    let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
    let prev_approval = blyte_agent::tools::approval();
    let prev_scope = blyte_agent::tools::write_scope();
    std::env::set_var("XDG_CONFIG_HOME", xdg.path());
    let _guard = PolicyGuard {
        _lock: lock,
        prev_xdg,
        prev_approval,
        prev_scope,
    };

    let (runner, _) = ScriptedRunner::new(Vec::new());
    let session = Session::start(Box::new(runner), "gemini-4-neon".into());
    let cfg = blyte_cli::repl::ReplConfig {
        repo_root: root.path().to_path_buf(),
        sandbox: "on".to_string(),
        ..Default::default()
    };
    let mut agent_runner = blyte_cli::session::AgentRunner::new(&cfg, session.approver());
    let _ = agent_runner.run("hello", "unknown-model-for-test", &|_| {});

    let shell = blyte_agent::shell::LocalShell::new();
    let fs = blyte_agent::fs::LocalFs;

    let session_yes = session.clone();
    let answer_yes = std::thread::spawn(move || {
        let requested = wait_for_entries(&session_yes, 1);
        let id = match &requested[0].body {
            EntryBody::ApprovalRequested {
                id,
                command,
                reason,
            } => {
                assert!(command.starts_with("write "), "{command}");
                assert!(command.ends_with("src/outside.rs"), "{command}");
                assert!(
                    reason.contains("outside the directories this conversation may write"),
                    "{reason}"
                );
                id.clone()
            }
            other => panic!("expected ApprovalRequested on surface, got {other:?}"),
        };
        session_yes
            .answer_approval(&id, ApprovalAnswer::Yes)
            .unwrap();
    });

    let approved_out = blyte_agent::tools::execute(
        root.path(),
        &shell,
        &fs,
        "write",
        &serde_json::json!({
            "path": "src/outside.rs",
            "content": "approved\n",
        }),
    );
    answer_yes.join().unwrap();
    assert!(!approved_out.is_error, "{}", approved_out.content);
    assert_eq!(
        std::fs::read_to_string(root.path().join("src/outside.rs")).unwrap(),
        "approved\n"
    );

    let session_no = session.clone();
    let answer_no = std::thread::spawn(move || {
        let requested = wait_for_entries(&session_no, 3);
        let id = match &requested[2].body {
            EntryBody::ApprovalRequested { id, command, .. } => {
                assert!(command.ends_with("src/denied.rs"), "{command}");
                id.clone()
            }
            other => panic!("expected ApprovalRequested on surface, got {other:?}"),
        };
        session_no.answer_approval(&id, ApprovalAnswer::No).unwrap();
    });

    let denied_out = blyte_agent::tools::execute(
        root.path(),
        &shell,
        &fs,
        "write",
        &serde_json::json!({
            "path": "src/denied.rs",
            "content": "denied\n",
        }),
    );
    answer_no.join().unwrap();
    assert!(denied_out.is_error, "{}", denied_out.content);
    assert!(!root.path().join("src/denied.rs").exists());
}

#[test]
fn entries_are_written_as_they_are_pushed() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("sessions").join("entries.jsonl");

    let (runner, _) = ScriptedRunner::new(vec![ScriptAction::Ok(vec![
        EntryBody::Turn {
            n: 1,
            text: "working".into(),
            input_tokens: 100,
            output_tokens: 40,
            cost_usd: Some(0.004),
        },
        EntryBody::ToolStarted {
            name: "read".into(),
            input: "README.md".into(),
        },
        EntryBody::ToolFinished {
            name: "read".into(),
            summary: "ok".into(),
            is_error: false,
        },
    ])]);

    let session = Session::start_with_log("gemini-4-neon".into(), Some(log_path.clone()), |_| {
        Box::new(runner)
    })
    .unwrap();

    let reader_session = session.clone();
    let reader_log = log_path.clone();
    let reader = std::thread::spawn(move || {
        let mut after = 0u64;
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while seen.len() < 6 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "timed out; seen={seen:?}");
            let batch = reader_session.entries_after(after, remaining);
            if batch.is_empty() {
                continue;
            }
            let disk = std::fs::read_to_string(&reader_log).unwrap();
            let disk_entries: Vec<Entry> = disk
                .lines()
                .map(|line| serde_json::from_str::<Entry>(line).unwrap())
                .collect();
            for entry in batch {
                assert!(
                    disk_entries.iter().any(|d| d == &entry),
                    "entry seq {} returned by entries_after before it was flushed to disk",
                    entry.seq
                );
                after = entry.seq;
                seen.push(entry);
            }
        }
        seen
    });

    session.submit("hello", Vec::new()).unwrap();
    session.record_error("side note");

    let seen = reader.join().unwrap();
    assert_eq!(seen.len(), 6);
    let final_disk = std::fs::read_to_string(&log_path).unwrap();
    let parsed: Vec<Entry> = final_disk
        .lines()
        .map(|l| serde_json::from_str::<Entry>(l).unwrap())
        .collect();
    assert_eq!(parsed, seen);
}

#[test]
fn a_restart_loads_entries_and_continues_numbering() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("entries.jsonl");

    let (runner1, _) = ScriptedRunner::new(vec![ScriptAction::Ok(vec![EntryBody::Turn {
        n: 1,
        text: "first reply".into(),
        input_tokens: 50,
        output_tokens: 20,
        cost_usd: Some(0.001),
    }])]);
    let session1 = Session::start_with_log("gemini-4-neon".into(), Some(log_path.clone()), |_| {
        Box::new(runner1)
    })
    .unwrap();
    session1.submit("before restart", Vec::new()).unwrap();
    let before = wait_for_entries(&session1, 3);
    assert_eq!(
        before.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    drop(session1);

    let (runner2, _) = ScriptedRunner::new(vec![ScriptAction::Ok(vec![EntryBody::Turn {
        n: 1,
        text: "after reply".into(),
        input_tokens: 60,
        output_tokens: 25,
        cost_usd: Some(0.002),
    }])]);
    let session2 = Session::start_with_log("gemini-4-neon".into(), Some(log_path.clone()), |_| {
        Box::new(runner2)
    })
    .unwrap();

    let loaded = session2.entries_after(0, Duration::ZERO);
    assert_eq!(loaded.len(), 4);
    assert_eq!(&loaded[..3], &before[..]);
    assert_eq!(loaded[3].seq, 4);
    assert_eq!(
        loaded[3].body,
        EntryBody::Error {
            message: blyte_cli::session::RESTART_NOTICE_TEXT.to_string(),
        }
    );
    assert_eq!(session2.info().last_seq, 4);

    let next_seq = session2.submit("after restart", Vec::new()).unwrap();
    assert_eq!(next_seq, 5);
    let all = wait_for_entries(&session2, 7);
    assert_eq!(
        all.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 6, 7]
    );
    assert_eq!(
        all[3].body,
        EntryBody::Error {
            message:
                "blyte restarted: the entries above are from before; the model does not remember them"
                    .to_string(),
        }
    );

    let disk_lines: Vec<Entry> = std::fs::read_to_string(&log_path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Entry>(l).unwrap())
        .collect();
    assert_eq!(disk_lines, all);
}

#[test]
fn a_cut_off_last_line_is_dropped_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("entries.jsonl");

    let e1 = Entry {
        seq: 1,
        ts: 1_700_000_001,
        body: EntryBody::UserMessage {
            text: "hello".into(),
            reports: Vec::new(),
            queued: false,
            attachments: Vec::new(),
            pastes: Vec::new(),
        },
    };
    let e2 = Entry {
        seq: 2,
        ts: 1_700_000_002,
        body: EntryBody::Idle {
            turns: 0,
            cost_usd: Some(0.0),
            seconds: None,
            first_output_seconds: None,
        },
    };
    let raw = format!(
        "{}\n{}\n{{\"seq\":3,\"ts\":1700000003,\"kind\":\"turn\",\"text\":\"cut off",
        serde_json::to_string(&e1).unwrap(),
        serde_json::to_string(&e2).unwrap(),
    );
    std::fs::write(&log_path, raw).unwrap();

    let mut err_buf = Vec::new();
    let (runner, _) = ScriptedRunner::new(Vec::new());
    let session = Session::start_with_log_io(
        "gemini-4-neon".into(),
        Some(log_path.clone()),
        &mut err_buf,
        |_| Box::new(runner),
    )
    .unwrap();

    let stderr_text = String::from_utf8(err_buf).unwrap();
    assert!(
        stderr_text.contains("entries.jsonl"),
        "expected stderr to name the file, got: {stderr_text}"
    );
    assert!(
        stderr_text.contains("3"),
        "expected stderr to name line 3, got: {stderr_text}"
    );

    let entries = session.entries_after(0, Duration::ZERO);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0], e1);
    assert_eq!(entries[1], e2);
    assert_eq!(entries[2].seq, 3);
    assert_eq!(
        entries[2].body,
        EntryBody::Error {
            message: blyte_cli::session::RESTART_NOTICE_TEXT.to_string(),
        }
    );
    drop(session);

    let mut err_buf2 = Vec::new();
    let (runner2, _) = ScriptedRunner::new(Vec::new());
    let session2 = Session::start_with_log_io(
        "gemini-4-neon".into(),
        Some(log_path),
        &mut err_buf2,
        |_| Box::new(runner2),
    )
    .unwrap();
    assert!(
        err_buf2.is_empty(),
        "expected clean reload after truncation, got: {}",
        String::from_utf8_lossy(&err_buf2)
    );
    assert_eq!(session2.entries_after(0, Duration::ZERO).len(), 4);
}

#[test]
fn a_bad_line_in_the_middle_stops_the_start_naming_the_line() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("entries.jsonl");

    let e1 = Entry {
        seq: 1,
        ts: 1_700_000_001,
        body: EntryBody::UserMessage {
            text: "first".into(),
            reports: Vec::new(),
            queued: false,
            attachments: Vec::new(),
            pastes: Vec::new(),
        },
    };
    let e3 = Entry {
        seq: 3,
        ts: 1_700_000_003,
        body: EntryBody::Idle {
            turns: 0,
            cost_usd: Some(0.0),
            seconds: None,
            first_output_seconds: None,
        },
    };
    let raw = format!(
        "{}\n{{not valid json\n{}\n",
        serde_json::to_string(&e1).unwrap(),
        serde_json::to_string(&e3).unwrap(),
    );
    std::fs::write(&log_path, raw).unwrap();

    let mut err_buf = Vec::new();
    let (runner, _) = ScriptedRunner::new(Vec::new());
    let result = Session::start_with_log_io(
        "gemini-4-neon".into(),
        Some(log_path.clone()),
        &mut err_buf,
        |_| Box::new(runner),
    );
    let err = match result {
        Ok(_) => panic!("expected bad line in the middle to fail startup"),
        Err(err) => err,
    };
    assert!(
        err.contains("entries.jsonl"),
        "expected error to name the file, got: {err}"
    );
    assert!(
        err.contains("2"),
        "expected error to name line 2, got: {err}"
    );
}

#[test]
fn a_loaded_approval_request_is_not_live() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("entries.jsonl");

    let e1 = Entry {
        seq: 1,
        ts: 1_700_000_001,
        body: EntryBody::UserMessage {
            text: "do something risky".into(),
            reports: Vec::new(),
            queued: false,
            attachments: Vec::new(),
            pastes: Vec::new(),
        },
    };
    let e2 = Entry {
        seq: 2,
        ts: 1_700_000_002,
        body: EntryBody::ApprovalRequested {
            id: "approval-1".into(),
            command: "rm -rf /tmp/old".into(),
            reason: "outside sandbox".into(),
        },
    };
    let raw = format!(
        "{}\n{}\n",
        serde_json::to_string(&e1).unwrap(),
        serde_json::to_string(&e2).unwrap(),
    );
    std::fs::write(&log_path, raw).unwrap();

    let (runner, _) = ScriptedRunner::new(vec![ScriptAction::Ok(vec![EntryBody::Turn {
        n: 1,
        text: "still responsive".into(),
        input_tokens: 10,
        output_tokens: 5,
        cost_usd: Some(0.001),
    }])]);
    let session =
        Session::start_with_log("gemini-4-neon".into(), Some(log_path), |_| Box::new(runner))
            .unwrap();

    let loaded = session.entries_after(0, Duration::ZERO);
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[1], e2);
    assert_eq!(
        loaded[2].body,
        EntryBody::Error {
            message: blyte_cli::session::RESTART_NOTICE_TEXT.to_string(),
        }
    );

    assert_eq!(
        session.answer_approval("approval-1", ApprovalAnswer::Yes),
        Err(AnswerError::NotFound)
    );
    assert_eq!(
        session.answer_approval("approval-1", ApprovalAnswer::No),
        Err(AnswerError::NotFound)
    );

    session.submit("new prompt", Vec::new()).unwrap();
    let after_prompt = wait_for_entries(&session, 6);
    assert!(matches!(after_prompt[5].body, EntryBody::Idle { .. }));

    let approver = session.approver();
    let (done_tx, done_rx) = mpsc::channel();
    let t = std::thread::spawn(move || {
        let _ = done_tx.send(approver.ask("echo fresh", "fresh approval"));
    });
    let with_new_approval = wait_for_entries(&session, 7);
    let new_id = match &with_new_approval[6].body {
        EntryBody::ApprovalRequested { id, .. } => id.clone(),
        other => panic!("expected ApprovalRequested, got {other:?}"),
    };
    assert_ne!(new_id, "approval-1");
    session
        .answer_approval(&new_id, ApprovalAnswer::Yes)
        .unwrap();
    assert_eq!(
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Approval::Approved
    );
    t.join().unwrap();
}

#[test]
fn a_complete_bad_last_line_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("entries.jsonl");
    let e1 = Entry {
        seq: 1,
        ts: 1_700_000_001,
        body: EntryBody::Idle {
            turns: 0,
            cost_usd: Some(0.0),
            seconds: None,
            first_output_seconds: None,
        },
    };
    // The bad line ends in a newline, so it was written whole: not a cut-off write.
    let raw = format!(
        "{}\n{{not valid json\n",
        serde_json::to_string(&e1).unwrap()
    );
    std::fs::write(&log_path, &raw).unwrap();

    let mut err_buf = Vec::new();
    let (runner, _) = ScriptedRunner::new(Vec::new());
    let result = Session::start_with_log_io(
        "gemini-4-neon".into(),
        Some(log_path.clone()),
        &mut err_buf,
        |_| Box::new(runner),
    );
    let err = match result {
        Ok(_) => panic!("expected a complete bad last line to fail startup"),
        Err(err) => err,
    };
    assert!(err.contains("entries.jsonl"), "got: {err}");
    assert!(err.contains("line 2"), "got: {err}");
    assert_eq!(std::fs::read_to_string(&log_path).unwrap(), raw);
}

#[test]
fn a_failed_write_shows_one_notice_on_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("entries.jsonl");

    let mut err_buf = Vec::new();
    let (runner, _) = ScriptedRunner::new(Vec::new());
    let session = Session::start_with_log_io(
        "gemini-4-neon".into(),
        Some(log_path.clone()),
        &mut err_buf,
        |_| Box::new(runner),
    )
    .unwrap();

    session.fail_log_writes("disk full");

    let seq1 = session
        .record_decision("run-1", "accept", Some("first"), None)
        .unwrap();
    let seq2 = session
        .record_decision("run-2", "accept", Some("second"), None)
        .unwrap();
    let seq3 = session
        .record_decision("run-3", "accept", Some("third"), None)
        .unwrap();
    assert_eq!(seq1, 1);
    assert_eq!(seq2, 3);
    assert_eq!(seq3, 4);

    let entries = session.entries_after(0, Duration::ZERO);
    assert_eq!(entries.len(), 4);
    assert_eq!(
        entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );

    let notices: Vec<&str> = entries
        .iter()
        .filter_map(|e| match &e.body {
            EntryBody::Error { message } if message.contains("could not save") => {
                Some(message.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(notices.len(), 1, "entries: {entries:?}");
    assert_eq!(
        notices[0],
        format!(
            "blyte: could not save this session's entries to {}: disk full; entries from here on may be missing after a restart",
            log_path.display()
        )
    );

    let decisions: Vec<&str> = entries
        .iter()
        .filter_map(|e| match &e.body {
            EntryBody::Decision { run, .. } => Some(run.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(decisions, vec!["run-1", "run-2", "run-3"]);
    assert!(std::fs::read_to_string(&log_path).unwrap().is_empty());
}

#[test]
fn a_gap_in_the_saved_entries_is_shown_at_load() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("entries.jsonl");

    let e1 = Entry {
        seq: 1,
        ts: 1_700_000_001,
        body: EntryBody::UserMessage {
            text: "one".into(),
            reports: Vec::new(),
            queued: false,
            attachments: Vec::new(),
            pastes: Vec::new(),
        },
    };
    let e2 = Entry {
        seq: 2,
        ts: 1_700_000_002,
        body: EntryBody::Idle {
            turns: 0,
            cost_usd: Some(0.0),
            seconds: None,
            first_output_seconds: None,
        },
    };
    let e5 = Entry {
        seq: 5,
        ts: 1_700_000_005,
        body: EntryBody::Idle {
            turns: 1,
            cost_usd: Some(0.01),
            seconds: None,
            first_output_seconds: None,
        },
    };
    let l1 = serde_json::to_string(&e1).unwrap();
    let l2 = serde_json::to_string(&e2).unwrap();
    let l5 = serde_json::to_string(&e5).unwrap();
    let raw = format!("{l1}\n{l2}\n{l5}\n");
    std::fs::write(&log_path, &raw).unwrap();

    let mut err_buf = Vec::new();
    let (runner, _) = ScriptedRunner::new(Vec::new());
    let session = Session::start_with_log_io(
        "gemini-4-neon".into(),
        Some(log_path.clone()),
        &mut err_buf,
        |_| Box::new(runner),
    )
    .unwrap();

    let loaded = session.entries_after(0, Duration::ZERO);
    assert_eq!(loaded.len(), 5);
    assert_eq!(
        loaded.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![1, 2, 3, 5, 6]
    );
    assert_eq!(loaded[0], e1);
    assert_eq!(loaded[1], e2);
    assert_eq!(
        loaded[2],
        Entry {
            seq: 3,
            ts: 1_700_000_005,
            body: EntryBody::Error {
                message: "blyte: 2 entries were not saved here".to_string(),
            },
        }
    );
    assert_eq!(loaded[3], e5);
    assert_eq!(loaded[4].seq, 6);
    assert_eq!(
        loaded[4].body,
        EntryBody::Error {
            message: blyte_cli::session::RESTART_NOTICE_TEXT.to_string(),
        }
    );

    let after_2 = session.entries_after(2, Duration::ZERO);
    assert_eq!(
        after_2.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![3, 5, 6]
    );

    let disk = std::fs::read_to_string(&log_path).unwrap();
    let disk_lines: Vec<&str> = disk.lines().collect();
    assert_eq!(disk_lines.len(), 4);
    assert_eq!(disk_lines[0], l1);
    assert_eq!(disk_lines[1], l2);
    assert_eq!(disk_lines[2], l5);
    let restart_on_disk: Entry = serde_json::from_str(disk_lines[3]).unwrap();
    assert_eq!(restart_on_disk.seq, 6);
}

#[test]
fn a_missing_first_entry_is_shown_at_load() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("entries.jsonl");

    let e2 = Entry {
        seq: 2,
        ts: 1_700_000_002,
        body: EntryBody::UserMessage {
            text: "second".into(),
            reports: Vec::new(),
            queued: false,
            attachments: Vec::new(),
            pastes: Vec::new(),
        },
    };
    let e3 = Entry {
        seq: 3,
        ts: 1_700_000_003,
        body: EntryBody::Idle {
            turns: 0,
            cost_usd: Some(0.0),
            seconds: None,
            first_output_seconds: None,
        },
    };
    let l2 = serde_json::to_string(&e2).unwrap();
    let l3 = serde_json::to_string(&e3).unwrap();
    std::fs::write(&log_path, format!("{l2}\n{l3}\n")).unwrap();

    let mut err_buf = Vec::new();
    let (runner, _) = ScriptedRunner::new(Vec::new());
    let session = Session::start_with_log_io(
        "gemini-4-neon".into(),
        Some(log_path.clone()),
        &mut err_buf,
        |_| Box::new(runner),
    )
    .unwrap();

    let loaded = session.entries_after(0, Duration::ZERO);
    assert_eq!(loaded.len(), 4);
    assert_eq!(
        loaded[0],
        Entry {
            seq: 1,
            ts: 1_700_000_002,
            body: EntryBody::Error {
                message: "blyte: 1 entry was not saved here".to_string(),
            },
        }
    );
    assert_eq!(loaded[1], e2);
    assert_eq!(loaded[2], e3);
    assert_eq!(loaded[3].seq, 4);
    assert_eq!(
        loaded[3].body,
        EntryBody::Error {
            message: blyte_cli::session::RESTART_NOTICE_TEXT.to_string(),
        }
    );

    let disk = std::fs::read_to_string(&log_path).unwrap();
    let disk_lines: Vec<&str> = disk.lines().collect();
    assert_eq!(disk_lines.len(), 3);
    assert_eq!(disk_lines[0], l2);
    assert_eq!(disk_lines[1], l3);
}

#[test]
fn entries_out_of_order_stop_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("entries.jsonl");

    let e1 = Entry {
        seq: 1,
        ts: 1_700_000_001,
        body: EntryBody::Idle {
            turns: 0,
            cost_usd: Some(0.0),
            seconds: None,
            first_output_seconds: None,
        },
    };
    let e3 = Entry {
        seq: 3,
        ts: 1_700_000_003,
        body: EntryBody::Idle {
            turns: 0,
            cost_usd: Some(0.0),
            seconds: None,
            first_output_seconds: None,
        },
    };
    let e2 = Entry {
        seq: 2,
        ts: 1_700_000_002,
        body: EntryBody::Idle {
            turns: 0,
            cost_usd: Some(0.0),
            seconds: None,
            first_output_seconds: None,
        },
    };
    let raw = format!(
        "{}\n{}\n{}\n",
        serde_json::to_string(&e1).unwrap(),
        serde_json::to_string(&e3).unwrap(),
        serde_json::to_string(&e2).unwrap(),
    );
    std::fs::write(&log_path, &raw).unwrap();

    let mut err_buf = Vec::new();
    let (runner, _) = ScriptedRunner::new(Vec::new());
    let result = Session::start_with_log_io(
        "gemini-4-neon".into(),
        Some(log_path.clone()),
        &mut err_buf,
        |_| Box::new(runner),
    );
    let err = match result {
        Ok(_) => panic!("expected out-of-order entries to fail startup"),
        Err(err) => err,
    };
    assert!(err.contains("entries.jsonl"), "got: {err}");
    assert!(err.contains("line 3"), "got: {err}");
    assert!(err.contains("seq 2 does not follow seq 3"), "got: {err}");
    assert_eq!(std::fs::read_to_string(&log_path).unwrap(), raw);
}

#[test]
fn a_repeated_seq_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("entries.jsonl");

    let e1 = Entry {
        seq: 1,
        ts: 1_700_000_001,
        body: EntryBody::Idle {
            turns: 0,
            cost_usd: Some(0.0),
            seconds: None,
            first_output_seconds: None,
        },
    };
    let raw = format!(
        "{}\n{}\n",
        serde_json::to_string(&e1).unwrap(),
        serde_json::to_string(&e1).unwrap(),
    );
    std::fs::write(&log_path, &raw).unwrap();

    let mut err_buf = Vec::new();
    let (runner, _) = ScriptedRunner::new(Vec::new());
    let result = Session::start_with_log_io(
        "gemini-4-neon".into(),
        Some(log_path.clone()),
        &mut err_buf,
        |_| Box::new(runner),
    );
    let err = match result {
        Ok(_) => panic!("expected repeated seq to fail startup"),
        Err(err) => err,
    };
    assert!(err.contains("entries.jsonl"), "got: {err}");
    assert!(err.contains("line 2"), "got: {err}");
    assert!(err.contains("seq 1 does not follow seq 1"), "got: {err}");
    assert_eq!(std::fs::read_to_string(&log_path).unwrap(), raw);
}

#[test]
fn seq_zero_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("entries.jsonl");

    let e0 = Entry {
        seq: 0,
        ts: 1_700_000_000,
        body: EntryBody::Idle {
            turns: 0,
            cost_usd: Some(0.0),
            seconds: None,
            first_output_seconds: None,
        },
    };
    let raw = format!("{}\n", serde_json::to_string(&e0).unwrap());
    std::fs::write(&log_path, &raw).unwrap();

    let mut err_buf = Vec::new();
    let (runner, _) = ScriptedRunner::new(Vec::new());
    let result = Session::start_with_log_io(
        "gemini-4-neon".into(),
        Some(log_path.clone()),
        &mut err_buf,
        |_| Box::new(runner),
    );
    let err = match result {
        Ok(_) => panic!("expected seq 0 to fail startup"),
        Err(err) => err,
    };
    assert!(err.contains("entries.jsonl"), "got: {err}");
    assert!(err.contains("line 1"), "got: {err}");
    assert!(err.contains("seq 0 does not follow seq 0"), "got: {err}");
    assert_eq!(std::fs::read_to_string(&log_path).unwrap(), raw);
}

struct TimedStreamingRunner {
    steps: VecDeque<TimedStep>,
}

enum TimedStep {
    StreamedTurn {
        delay_to_first_delta: Duration,
        delay_after_first_delta: Duration,
        entries: Vec<EntryBody>,
    },
    ToolBeforeTurn {
        delay_to_tool: Duration,
        delay_after_tool: Duration,
        tool_started: Box<EntryBody>,
        after: Vec<EntryBody>,
    },
    ErrorWithoutOutput {
        delay: Duration,
        message: String,
    },
}

impl PromptRunner for TimedStreamingRunner {
    fn run(
        &mut self,
        prompt: &str,
        model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        self.run_with_first_output(prompt, model, emit, &|_| {})
    }

    fn run_with_first_output(
        &mut self,
        _prompt: &str,
        _model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
        on_first_output: &(dyn Fn(Instant) + Sync),
    ) -> Result<(), String> {
        match self.steps.pop_front().expect("unexpected extra prompt") {
            TimedStep::StreamedTurn {
                delay_to_first_delta,
                delay_after_first_delta,
                entries,
            } => {
                std::thread::sleep(delay_to_first_delta);
                on_first_output(Instant::now());
                std::thread::sleep(delay_after_first_delta);
                for body in entries {
                    emit(body);
                }
                Ok(())
            }
            TimedStep::ToolBeforeTurn {
                delay_to_tool,
                delay_after_tool,
                tool_started,
                after,
            } => {
                std::thread::sleep(delay_to_tool);
                emit(*tool_started);
                std::thread::sleep(delay_after_tool);
                for body in after {
                    emit(body);
                }
                Ok(())
            }
            TimedStep::ErrorWithoutOutput { delay, message } => {
                std::thread::sleep(delay);
                Err(message)
            }
        }
    }
}

#[test]
fn idle_entry_records_how_long_the_reply_took() {
    let runner = TimedStreamingRunner {
        steps: VecDeque::from([
            TimedStep::StreamedTurn {
                delay_to_first_delta: Duration::from_millis(150),
                delay_after_first_delta: Duration::from_millis(250),
                entries: vec![EntryBody::Turn {
                    n: 1,
                    text: "done".into(),
                    input_tokens: 100,
                    output_tokens: 50,
                    cost_usd: Some(0.01),
                }],
            },
            TimedStep::ErrorWithoutOutput {
                delay: Duration::from_millis(150),
                message: "no output".into(),
            },
        ]),
    };

    let session = Session::start(Box::new(runner), "gemini-4-neon".into());
    session.submit("first", Vec::new()).unwrap();
    let entries = wait_for_entries(&session, 3);
    match entries[2].body {
        EntryBody::Idle {
            seconds: Some(s),
            first_output_seconds: Some(f),
            ..
        } => {
            assert!(s >= 0.3, "expected seconds >= 0.3, got {s}");
            assert!(f >= 0.1, "expected first_output_seconds >= 0.1, got {f}");
            assert!(f < s, "expected first_output_seconds ({f}) < seconds ({s})");
        }
        ref other => panic!("expected Idle with reply timings, got {other:?}"),
    }

    session.submit("second", Vec::new()).unwrap();
    let all = wait_for_entries(&session, 6);
    match all[5].body {
        EntryBody::Idle {
            seconds: Some(s),
            first_output_seconds: None,
            ..
        } => {
            assert!(s >= 0.1, "expected seconds >= 0.1, got {s}");
        }
        ref other => {
            panic!("expected Idle with seconds and no first_output_seconds, got {other:?}")
        }
    }
}

#[test]
fn idle_entry_records_first_output_on_tool_started() {
    let runner = TimedStreamingRunner {
        steps: VecDeque::from([TimedStep::ToolBeforeTurn {
            delay_to_tool: Duration::from_millis(150),
            delay_after_tool: Duration::from_millis(250),
            tool_started: Box::new(EntryBody::ToolStarted {
                name: "read".into(),
                input: "Cargo.toml".into(),
            }),
            after: vec![
                EntryBody::ToolFinished {
                    name: "read".into(),
                    summary: "ok".into(),
                    is_error: false,
                },
                EntryBody::Turn {
                    n: 1,
                    text: "checked".into(),
                    input_tokens: 100,
                    output_tokens: 40,
                    cost_usd: Some(0.01),
                },
            ],
        }]),
    };

    let session = Session::start(Box::new(runner), "gemini-4-neon".into());
    session.submit("inspect", Vec::new()).unwrap();
    let entries = wait_for_entries(&session, 5);
    match entries[4].body {
        EntryBody::Idle {
            seconds: Some(s),
            first_output_seconds: Some(f),
            ..
        } => {
            assert!(s >= 0.3, "expected seconds >= 0.3, got {s}");
            assert!(f >= 0.1, "expected first_output_seconds >= 0.1, got {f}");
            assert!(f < s, "expected first_output_seconds ({f}) < seconds ({s})");
        }
        ref other => panic!("expected Idle with tool-start first_output timing, got {other:?}"),
    }
}

#[test]
fn idle_entry_without_times_still_reads() {
    let line = r#"{"seq":1,"ts":1,"kind":"idle","turns":1,"cost_usd":0.01}"#;
    let entry: Entry = serde_json::from_str(line).unwrap();
    assert_eq!(
        entry,
        Entry {
            seq: 1,
            ts: 1,
            body: EntryBody::Idle {
                turns: 1,
                cost_usd: Some(0.01),
                seconds: None,
                first_output_seconds: None,
            },
        }
    );

    let encoded: serde_json::Value = serde_json::to_value(&entry).unwrap();
    let obj = encoded.as_object().unwrap();
    assert!(!obj.contains_key("seconds"), "unexpected key in {encoded}");
    assert!(
        !obj.contains_key("first_output_seconds"),
        "unexpected key in {encoded}"
    );
}

#[test]
fn surface_page_shows_reply_time() {
    let html = include_str!("../assets/surface/index.html");
    assert!(html.contains("first_output_seconds"));
    assert!(html.contains("first output"));
}
