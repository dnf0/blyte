//! The runner stops at the deadline instead of hanging for the watchdog.

use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{claim_check_reserve, Budget, Outcome, Runner};
use blyte_agent::shell::{Output, Shell};
use blyte_agent::state::State;
use blyte_agent::tools::COMMAND_TIMEOUT;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall, Usage};
use serde_json::json;

struct FakeClock {
    base: Instant,
    offset_ms: AtomicU64,
}

impl FakeClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            base: Instant::now(),
            offset_ms: AtomicU64::new(0),
        })
    }

    fn now(&self) -> Instant {
        self.base + Duration::from_millis(self.offset_ms.load(Ordering::SeqCst))
    }

    fn advance(&self, d: Duration) {
        self.offset_ms
            .fetch_add(d.as_millis() as u64, Ordering::SeqCst);
    }
}

struct BlockingProvider {
    clock: Arc<FakeClock>,
    seen_timeouts: Mutex<Vec<Option<Duration>>>,
}

impl Provider for BlockingProvider {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen_timeouts.lock().unwrap().push(req.timeout);
        let t = req.timeout.expect("runner should set req.timeout");
        self.clock.advance(t + Duration::from_millis(20));
        Err(ProviderError::Transport(
            "request timed out after 2s".into(),
        ))
    }

    fn model_id(&self) -> String {
        "fake/blocking".into()
    }
}

#[test]
fn model_call_blocks_until_req_timeout_then_returns_budget_exhausted() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let provider = BlockingProvider {
        clock: Arc::clone(&clock),
        seen_timeouts: Mutex::new(Vec::new()),
    };
    let shell = RecordingShell::new();
    let runner = Runner {
        provider: &provider,
        shell: &shell,
        fs: &LocalFs,
        context: &Conversational,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 2,
            max_duration: Some(Duration::from_secs(2)),
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
        base_check: false,
    };

    let mut state = State::new("finish the task", "true");
    let (outcome, report) = runner.run_with_clock(&mut state, &|| clock.now());

    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert!(
        !report.turn_usage.is_empty(),
        "expected turn_usage to record the timed-out attempt"
    );
    let timeouts = provider.seen_timeouts.lock().unwrap();
    assert_eq!(timeouts.len(), 1);
    assert!(
        timeouts[0].is_some_and(|t| t <= Duration::from_secs(2) && !t.is_zero()),
        "expected req.timeout <= 2s, got {:?}",
        timeouts[0]
    );
}

struct RecordingShell {
    user_timeouts: Mutex<Vec<Duration>>,
    exec_timeouts: Mutex<Vec<Duration>>,
    advance_clock: Option<Arc<FakeClock>>,
    exit_code: i32,
}

impl RecordingShell {
    fn new() -> Self {
        Self {
            user_timeouts: Mutex::new(Vec::new()),
            exec_timeouts: Mutex::new(Vec::new()),
            advance_clock: None,
            exit_code: 0,
        }
    }
}

impl Shell for RecordingShell {
    fn describe(&self) -> String {
        "recording shell".into()
    }

    fn exec(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        if command.contains("__BLYTE_IN_GATE") {
            self.exec_timeouts.lock().unwrap().push(timeout);
            if let Some(clock) = &self.advance_clock {
                clock.advance(timeout + Duration::from_millis(20));
            }
            return Ok(Output {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: self.exit_code,
            });
        }
        if command.contains("ls-files") {
            let mut files = Vec::new();
            if let Ok(entries) = std::fs::read_dir(cwd) {
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|t| t.is_file()) {
                        if let Some(name) = entry.file_name().to_str() {
                            files.push(name.to_string());
                        }
                    }
                }
            }
            files.sort();
            return Ok(Output {
                stdout: files.join("\n"),
                stderr: String::new(),
                exit_code: 0,
            });
        }
        Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
        })
    }

    fn exec_user(&self, _command: &str, _cwd: &Path, timeout: Duration) -> Result<Output, String> {
        self.user_timeouts.lock().unwrap().push(timeout);
        Ok(Output {
            stdout: "ok\n".into(),
            stderr: String::new(),
            exit_code: 0,
        })
    }
}

struct ScriptedProvider {
    turns: Vec<Response>,
    idx: AtomicUsize,
}

impl ScriptedProvider {
    fn new(turns: Vec<Response>) -> Self {
        Self {
            turns,
            idx: AtomicUsize::new(0),
        }
    }
}

impl Provider for ScriptedProvider {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let i = self.idx.fetch_add(1, Ordering::SeqCst);
        Ok(self.turns.get(i).cloned().unwrap_or(Response {
            text: "done".into(),
            tool_calls: vec![],
            usage: Usage::default(),
            finish_reason: None,
        }))
    }

    fn model_id(&self) -> String {
        "fake/scripted".into()
    }
}

#[test]
fn bash_timeout_is_clamped_to_deadline_or_command_timeout_without_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();

    let bash_turn = Response {
        text: String::new(),
        tool_calls: vec![ToolCall {
            id: "1".into(),
            name: "bash".into(),
            arguments: json!({"command": "echo hi"}),
            signature: None,
        }],
        usage: Usage::default(),
        finish_reason: None,
    };
    let done_turn = Response {
        text: "done".into(),
        tool_calls: vec![],
        usage: Usage::default(),
        finish_reason: None,
    };

    let shell = RecordingShell::new();
    let provider = ScriptedProvider::new(vec![bash_turn.clone(), done_turn.clone()]);
    let runner = Runner {
        provider: &provider,
        shell: &shell,
        fs: &LocalFs,
        context: &Conversational,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 2,
            max_duration: Some(Duration::from_secs(5)),
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
        base_check: false,
    };
    let mut state = State::new("run echo", "true");
    let (outcome, _) = runner.run_with_clock(&mut state, &|| clock.now());
    assert_eq!(outcome, Outcome::Verified);
    let recorded = shell.user_timeouts.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(
        recorded[0] <= Duration::from_secs(5) && !recorded[0].is_zero(),
        "expected bash timeout <= 5s, got {:?}",
        recorded[0]
    );

    let shell_no_deadline = RecordingShell::new();
    let provider_no_deadline = ScriptedProvider::new(vec![bash_turn, done_turn]);
    let runner_no_deadline = Runner {
        provider: &provider_no_deadline,
        shell: &shell_no_deadline,
        fs: &LocalFs,
        context: &Conversational,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 2,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
        base_check: false,
    };
    let mut state2 = State::new("run echo", "true");
    let (outcome2, _) = runner_no_deadline.run_with_clock(&mut state2, &|| clock.now());
    assert_eq!(outcome2, Outcome::Verified);
    let recorded2 = shell_no_deadline.user_timeouts.lock().unwrap();
    assert_eq!(recorded2.len(), 1);
    assert_eq!(recorded2[0], COMMAND_TIMEOUT);
}

#[test]
fn gate_clamps_timeout_and_ends_budget_exhausted_when_deadline_passes() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let shell = RecordingShell {
        user_timeouts: Mutex::new(Vec::new()),
        exec_timeouts: Mutex::new(Vec::new()),
        advance_clock: Some(Arc::clone(&clock)),
        exit_code: 124,
    };
    let provider = ScriptedProvider::new(vec![Response {
        text: "done".into(),
        tool_calls: vec![],
        usage: Usage::default(),
        finish_reason: None,
    }]);
    let runner = Runner {
        provider: &provider,
        shell: &shell,
        fs: &LocalFs,
        context: &Conversational,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 3,
            max_duration: Some(Duration::from_secs(1)),
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
        base_check: false,
    };
    let mut state = State::new("say done", "sleep 120");
    let (outcome, report) = runner.run_with_clock(&mut state, &|| clock.now());

    assert_eq!(outcome, Outcome::BudgetExhausted);
    let exec_timeouts = shell.exec_timeouts.lock().unwrap();
    assert!(
        exec_timeouts
            .first()
            .is_some_and(|t| *t <= Duration::from_secs(1) && !t.is_zero()),
        "expected gate exec timeout <= 1s, got {:?}",
        exec_timeouts
    );
    assert!(
        report
            .gate_failures
            .iter()
            .any(|f| f.contains("gate ran out of time")),
        "expected gate_failures to say gate ran out of time, got {:?}",
        report.gate_failures
    );
}

struct DelayedResearchProvider {
    clock: Arc<FakeClock>,
    advance_before_complete: Duration,
    seen_timeouts: Mutex<Vec<Option<Duration>>>,
    calls: AtomicUsize,
}

impl Provider for DelayedResearchProvider {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen_timeouts.lock().unwrap().push(req.timeout);
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            self.clock.advance(self.advance_before_complete);
            return Ok(Response {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "1".into(),
                    name: "read".into(),
                    arguments: json!({"path": "src.py"}),
                    signature: None,
                }],
                usage: Usage::default(),
                finish_reason: None,
            });
        }
        Ok(Response {
            text: r#"[{"id": 1, "verdict": "supported", "reason": "line 1 sets alpha = 1"}]"#
                .into(),
            tool_calls: vec![],
            usage: Usage::default(),
            finish_reason: None,
        })
    }

    fn model_id(&self) -> String {
        "fake/delayed-research".into()
    }
}

#[test]
fn research_run_whose_deadline_passes_before_claim_check_sets_skipped_and_runs_quote_check() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("src.py"), "alpha = 1\nbeta = 2\n").unwrap();
    std::fs::write(
        dir.path().join("report.md"),
        "The value `alpha` is set to 1 in `src.py:1`.\n",
    )
    .unwrap();

    let clock = FakeClock::new();
    let provider = DelayedResearchProvider {
        clock: Arc::clone(&clock),
        advance_before_complete: Duration::from_secs(11),
        seen_timeouts: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
    };
    let shell = RecordingShell::new();
    let runner = Runner {
        provider: &provider,
        shell: &shell,
        fs: &LocalFs,
        context: &Conversational,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 2,
            max_duration: Some(Duration::from_secs(10)),
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: true,
        base_check: false,
    };

    let mut state = State::new("investigate alpha", "");
    let (outcome, report) = runner.run_with_clock(&mut state, &|| clock.now());

    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert!(
        report.quote_problems.is_some(),
        "expected quote check to run after deadline"
    );
    assert!(
        report
            .claim_check_skipped
            .as_deref()
            .is_some_and(|s| s.contains("out of time")),
        "expected claim_check_skipped to say out of time, got {:?}",
        report.claim_check_skipped
    );
    assert!(
        report.claim_check.is_none(),
        "expected no claim_check report when no batches ran, got {:?}",
        report.claim_check
    );
    assert_eq!(
        provider.seen_timeouts.lock().unwrap().len(),
        1,
        "claim check should not call provider after deadline"
    );
}

#[test]
fn claim_check_reserve_is_a_tenth_capped_at_180s() {
    assert_eq!(
        claim_check_reserve(Duration::from_secs(300)),
        Duration::from_secs(30)
    );
    assert_eq!(
        claim_check_reserve(Duration::from_secs(1800)),
        Duration::from_secs(180)
    );
    assert_eq!(
        claim_check_reserve(Duration::from_secs(3600)),
        Duration::from_secs(180)
    );
    assert_eq!(
        claim_check_reserve(Duration::from_secs(10)),
        Duration::from_secs(1)
    );
}

#[test]
fn research_loop_stops_a_reserve_before_the_budget() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let provider = DelayedResearchProvider {
        clock: Arc::clone(&clock),
        advance_before_complete: Duration::ZERO,
        seen_timeouts: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
    };
    let shell = RecordingShell::new();
    let runner = Runner {
        provider: &provider,
        shell: &shell,
        fs: &LocalFs,
        context: &Conversational,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 1,
            max_gate_attempts: 1,
            max_duration: Some(Duration::from_secs(100)),
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: true,
        base_check: false,
    };

    let mut state = State::new("investigate", "");
    let _ = runner.run_with_clock(&mut state, &|| clock.now());
    let seen = provider.seen_timeouts.lock().unwrap();
    let t = seen[0].expect("timeout");
    assert!(
        t <= Duration::from_secs(90) && t > Duration::from_secs(80),
        "expected loop timeout near 90s (100s minus 10s reserve), got {t:?}"
    );
}

#[test]
fn non_research_loop_keeps_the_whole_budget() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let shell = RecordingShell::new();

    for (research, claim_check) in [(false, false), (true, false)] {
        let provider = DelayedResearchProvider {
            clock: Arc::clone(&clock),
            advance_before_complete: Duration::ZERO,
            seen_timeouts: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
        };
        let runner = Runner {
            provider: &provider,
            shell: &shell,
            fs: &LocalFs,
            context: &Conversational,
            root: dir.path().to_path_buf(),
            budget: Budget {
                max_turns: 1,
                max_gate_attempts: 1,
                max_duration: Some(Duration::from_secs(100)),
            },
            rollback: false,
            on_event: None,
            research,
            claim_check,
            base_check: false,
        };
        let mut state = State::new("work", "");
        let _ = runner.run_with_clock(&mut state, &|| clock.now());
        let seen = provider.seen_timeouts.lock().unwrap();
        let t = seen[0].expect("timeout");
        assert!(
            t > Duration::from_secs(90) && t <= Duration::from_secs(100),
            "research={research} claim_check={claim_check}: expected timeout near 100s, got {t:?}"
        );
    }
}

#[test]
fn claim_check_runs_in_the_reserve_after_the_loop_runs_out() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("src.py"), "alpha = 1\nbeta = 2\n").unwrap();
    std::fs::write(
        dir.path().join("report.md"),
        "The value `alpha` is set to 1 in `src.py:1`.\n",
    )
    .unwrap();

    let clock = FakeClock::new();
    let shell = RecordingShell::new();
    let provider = DelayedResearchProvider {
        clock: Arc::clone(&clock),
        advance_before_complete: Duration::from_secs(95),
        seen_timeouts: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
    };
    let runner = Runner {
        provider: &provider,
        shell: &shell,
        fs: &LocalFs,
        context: &Conversational,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 2,
            max_duration: Some(Duration::from_secs(100)),
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: true,
        base_check: false,
    };

    let mut state = State::new("investigate alpha", "");
    let (outcome, report) = runner.run_with_clock(&mut state, &|| clock.now());

    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert!(
        report.claim_check_skipped.is_none(),
        "claim check should run in the reserve, not skip: {:?}",
        report.claim_check_skipped
    );
    let cc = report
        .claim_check
        .as_ref()
        .expect("claim check should have run in the reserve");
    assert!(cc.calls >= 1);
    assert_eq!(cc.checked, 1);
    assert!(cc.contradicted.is_empty());
    let timeouts = provider.seen_timeouts.lock().unwrap();
    assert_eq!(
        timeouts.len(),
        2,
        "expected 1 loop call + 1 claim check call, got {timeouts:?}"
    );
    assert!(
        timeouts[0].is_some_and(|t| t <= Duration::from_secs(90) && t > Duration::from_secs(80)),
        "loop call should have the 90s loop budget, got {:?}",
        timeouts[0]
    );
    assert!(
        timeouts[1].is_some_and(|t| t <= Duration::from_secs(10) && !t.is_zero()),
        "claim check should get the remaining reserve, got {:?}",
        timeouts[1]
    );
}
