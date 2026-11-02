//! At the deadline, the runner stops with `Outcome::BudgetExhausted` and a populated report
//! instead of letting a model call, bash call, gate, or claim check run past the deadline.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::{LocalShell, Output, Shell};
use blyte_agent::state::State;
use blyte_agent::tools::COMMAND_TIMEOUT;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall, Usage};
use serde_json::json;

struct BlockingProvider {
    seen_timeouts: Mutex<Vec<Option<Duration>>>,
}

impl BlockingProvider {
    fn new() -> Self {
        Self {
            seen_timeouts: Mutex::new(Vec::new()),
        }
    }
}

impl Provider for BlockingProvider {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen_timeouts.lock().unwrap().push(req.timeout);
        if let Some(t) = req.timeout {
            std::thread::sleep(t + Duration::from_millis(20));
        }
        Err(ProviderError::Transport(
            "request timed out after 2s".into(),
        ))
    }

    fn model_id(&self) -> String {
        "blocking-fake".into()
    }
}

#[test]
fn model_call_blocks_until_req_timeout_then_returns_budget_exhausted() {
    let dir = tempfile::tempdir().unwrap();
    let provider = BlockingProvider::new();
    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
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
    };
    let mut state = State::new("goal", "true");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert!(
        !report.turn_usage.is_empty(),
        "turn_usage should record the timed-out turn"
    );
    let timeouts = provider.seen_timeouts.lock().unwrap();
    assert_eq!(timeouts.len(), 1);
    let seen = timeouts[0].expect("expected req.timeout to be Some");
    assert!(
        seen <= Duration::from_secs(2),
        "expected req.timeout <= 2s, got {seen:?}"
    );
}

struct RecordingShell {
    user_timeouts: Mutex<Vec<Duration>>,
    exec_timeouts: Mutex<Vec<Duration>>,
    sleep_on_exec: Duration,
    exit_code: i32,
}

impl RecordingShell {
    fn new() -> Self {
        Self {
            user_timeouts: Mutex::new(Vec::new()),
            exec_timeouts: Mutex::new(Vec::new()),
            sleep_on_exec: Duration::ZERO,
            exit_code: 0,
        }
    }
}

impl Shell for RecordingShell {
    fn exec(&self, command: &str, _cwd: &Path, timeout: Duration) -> Result<Output, String> {
        if !command.contains("BLYTE_IN_GATE=1") {
            return Ok(Output {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 1,
            });
        }
        self.exec_timeouts.lock().unwrap().push(timeout);
        if !self.sleep_on_exec.is_zero() {
            std::thread::sleep(self.sleep_on_exec);
        }
        Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: self.exit_code,
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

    fn describe(&self) -> String {
        "recording-shell".into()
    }
}

struct ScriptedProvider {
    turns: Mutex<Vec<Response>>,
    seen_timeouts: Mutex<Vec<Option<Duration>>>,
    sleep_before_complete: Duration,
}

impl ScriptedProvider {
    fn new(turns: Vec<Response>) -> Self {
        Self {
            turns: Mutex::new(turns),
            seen_timeouts: Mutex::new(Vec::new()),
            sleep_before_complete: Duration::ZERO,
        }
    }
}

impl Provider for ScriptedProvider {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen_timeouts.lock().unwrap().push(req.timeout);
        if !self.sleep_before_complete.is_zero() {
            std::thread::sleep(self.sleep_before_complete);
        }
        let mut t = self.turns.lock().unwrap();
        if t.is_empty() {
            Ok(Response {
                text: "done".into(),
                ..Default::default()
            })
        } else {
            Ok(t.remove(0))
        }
    }

    fn model_id(&self) -> String {
        "scripted-fake".into()
    }
}

#[test]
fn bash_timeout_is_clamped_to_deadline_or_command_timeout_without_deadline() {
    let bash_turn = || Response {
        text: String::new(),
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "bash".into(),
            arguments: json!({"command": "echo hi"}),
            signature: None,
        }],
        usage: Usage::default(),
        finish_reason: None,
    };
    let done_turn = || Response {
        text: "done".into(),
        tool_calls: vec![],
        usage: Usage::default(),
        finish_reason: None,
    };

    // With a 5 s deadline, the bash timeout is <= 5 s.
    let dir = tempfile::tempdir().unwrap();
    let shell_with_deadline = RecordingShell::new();
    let provider_with_deadline = ScriptedProvider::new(vec![bash_turn(), done_turn()]);
    let runner = Runner {
        provider: &provider_with_deadline,
        shell: &shell_with_deadline,
        fs: &LocalFs,
        context: &Conversational,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: Some(Duration::from_secs(5)),
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };
    let mut state = State::new("goal", "true");
    let (outcome, _) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);
    let recorded = shell_with_deadline.user_timeouts.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(
        recorded[0] <= Duration::from_secs(5) && !recorded[0].is_zero(),
        "expected bash timeout <= 5s, got {:?}",
        recorded[0]
    );

    // With no deadline, the bash timeout is COMMAND_TIMEOUT, and requests carry timeout: None.
    let shell_no_deadline = RecordingShell::new();
    let provider_no_deadline = ScriptedProvider::new(vec![bash_turn(), done_turn()]);
    let runner_no_deadline = Runner {
        provider: &provider_no_deadline,
        shell: &shell_no_deadline,
        fs: &LocalFs,
        context: &Conversational,
        root: dir.path().to_path_buf(),
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
    let mut state2 = State::new("goal", "true");
    let (outcome2, _) = runner_no_deadline.run(&mut state2);
    assert_eq!(outcome2, Outcome::Verified);
    let recorded_no_deadline = shell_no_deadline.user_timeouts.lock().unwrap();
    assert_eq!(*recorded_no_deadline, vec![COMMAND_TIMEOUT]);
    let seen_req_timeouts = provider_no_deadline.seen_timeouts.lock().unwrap();
    assert_eq!(*seen_req_timeouts, vec![None, None]);
}

#[test]
fn gate_clamps_timeout_and_ends_budget_exhausted_when_deadline_passes() {
    let dir = tempfile::tempdir().unwrap();
    let shell = RecordingShell {
        user_timeouts: Mutex::new(Vec::new()),
        exec_timeouts: Mutex::new(Vec::new()),
        sleep_on_exec: Duration::from_millis(1050),
        exit_code: 124,
    };
    let provider = ScriptedProvider::new(vec![Response {
        text: "done".into(),
        ..Default::default()
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
    };
    let mut state = State::new("goal", "cargo test");
    let (outcome, report) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert_eq!(report.gate_attempts, 1);
    assert!(
        report
            .gate_failures
            .first()
            .is_some_and(|f| f.contains("gate ran out of time")),
        "expected gate failure to say gate ran out of time, got {:?}",
        report.gate_failures
    );
    let exec_timeouts = shell.exec_timeouts.lock().unwrap();
    assert!(
        exec_timeouts
            .first()
            .is_some_and(|&t| t <= Duration::from_secs(1)),
        "expected gate timeout <= 1s, got {exec_timeouts:?}"
    );
}

#[test]
fn research_run_whose_deadline_passes_before_claim_check_sets_skipped_and_runs_quote_check() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("src.py"), "def main():\n    run()\n").unwrap();
    // Write a report with both a prose claim citing src.py:1-2 and a mismatched quote.
    std::fs::write(
        dir.path().join("report.md"),
        "In `src.py:1-2`, `main` calls `run`.\n```python\ndef main():\n    wrong()\n```\n",
    )
    .unwrap();
    LocalShell
        .exec(
            "git init -q && git add src.py",
            dir.path(),
            Duration::from_secs(10),
        )
        .unwrap();

    // Turn 1 sleeps past the 1s deadline while completing, then the loop exits BudgetExhausted
    // before the claim check can run.
    let provider = ScriptedProvider {
        turns: Mutex::new(vec![Response {
            text: "reading".into(),
            tool_calls: vec![ToolCall {
                id: "r1".into(),
                name: "read".into(),
                arguments: json!({"path": "src.py"}),
                signature: None,
            }],
            usage: Usage::default(),
            finish_reason: None,
        }]),
        seen_timeouts: Mutex::new(Vec::new()),
        sleep_before_complete: Duration::from_millis(1050),
    };

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: dir.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 2,
            max_duration: Some(Duration::from_secs(1)),
        },
        rollback: false,
        on_event: None,
        research: true,
        claim_check: true,
    };
    let mut state = State::new("investigate src.py", "test -s report.md");
    let (outcome, report) = runner.run(&mut state);

    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert_eq!(
        report.claim_check_skipped.as_deref(),
        Some("out of time after 0 of 1 batches")
    );
    assert!(
        report.claim_check.is_none(),
        "expected claim_check to be None when 0 batches ran, got {:?}",
        report.claim_check
    );
    let problems = report
        .quote_problems
        .expect("quote_problems should be present even when deadline passed");
    assert!(
        !problems.is_empty(),
        "quote check should still detect the mismatched quote"
    );
    // Only the 1 worker turn called the provider; the claim check made 0 calls.
    assert_eq!(provider.seen_timeouts.lock().unwrap().len(), 1);
}
