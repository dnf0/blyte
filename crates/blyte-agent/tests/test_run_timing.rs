//! The report says where the time went: model and tool time per turn, time per gate attempt,
//! calls per tool. Shadow run S3 was slower with half the turns and the report could not say why.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::{Output, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

struct Clock {
    now: Mutex<Instant>,
}

impl Clock {
    fn new() -> Self {
        Self {
            now: Mutex::new(Instant::now()),
        }
    }

    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    fn advance(&self, d: Duration) {
        *self.now.lock().unwrap() += d;
    }
}

struct TimedShell {
    clock: Arc<Clock>,
}

impl Shell for TimedShell {
    fn exec(&self, command: &str, _cwd: &Path, _timeout: Duration) -> Result<Output, String> {
        for line in command.lines().map(str::trim) {
            if let Some(secs) = line
                .strip_prefix("sleep ")
                .and_then(|s| s.parse::<f64>().ok())
            {
                self.clock.advance(Duration::from_secs_f64(secs));
                return Ok(Output {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                });
            }
            if line == "true" {
                return Ok(Output {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                });
            }
        }
        Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
        })
    }

    fn exec_host(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<Output, String> {
        self.exec(command, cwd, timeout)
    }

    fn describe(&self) -> String {
        String::new()
    }

    fn available_binaries(&self, _cwd: &Path, _candidates: &[&'static str]) -> Vec<String> {
        Vec::new()
    }
}

struct Slow {
    calls: Mutex<u32>,
    clock: Arc<Clock>,
}

impl Provider for Slow {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        self.clock.advance(Duration::from_millis(100));
        let mut n = self.calls.lock().unwrap();
        *n += 1;
        let call = |name: &str, args: serde_json::Value| ToolCall {
            id: format!("c{n}"),
            name: name.into(),
            arguments: args,
            signature: None,
        };
        Ok(match *n {
            1 => Response {
                tool_calls: vec![call("bash", json!({"command": "sleep 0.3"}))],
                ..Default::default()
            },
            2 => Response {
                tool_calls: vec![
                    call("read", json!({"path": "marker"})),
                    call("bash", json!({"command": "true"})),
                ],
                ..Default::default()
            },
            _ => Response::default(),
        })
    }
    fn model_id(&self) -> String {
        "slow".into()
    }
}

fn run(gate: &str) -> (Outcome, blyte_agent::run::RunReport) {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("marker"), "0\n").unwrap();
    let clock = Arc::new(Clock::new());
    let shell = TimedShell {
        clock: Arc::clone(&clock),
    };
    let p = Slow {
        calls: Mutex::new(0),
        clock: Arc::clone(&clock),
    };
    let r = Runner {
        provider: &p,
        shell: &shell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 6,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
        base_check: blyte_agent::run::BaseCheck::None,
    };
    r.run_with_clock(&mut State::new("g", gate), &|| clock.now())
}

#[test]
fn every_turn_records_its_model_time() {
    let (_, rep) = run("true");
    assert!(!rep.turn_usage.is_empty());
    for t in &rep.turn_usage {
        assert!(t.model_ms >= 90, "turn {} model_ms {}", t.turn, t.model_ms);
    }
}

#[test]
fn tool_time_is_recorded_on_the_turn_that_ran_the_tools() {
    let (_, rep) = run("true");
    assert!(
        rep.turn_usage[0].tools_ms >= 250,
        "{}",
        rep.turn_usage[0].tools_ms
    );
    let last = rep.turn_usage.last().unwrap();
    assert_eq!(
        last.tools_ms, 0,
        "a turn with no tool calls spends no tool time"
    );
}

#[test]
fn each_gate_attempt_is_timed() {
    let (outcome, rep) = run("sleep 0.3");
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(rep.gate_ms.len() as u32, rep.gate_attempts);
    assert!(rep.gate_ms[0] >= 250, "{:?}", rep.gate_ms);
}

#[test]
fn calls_are_counted_per_tool() {
    let (_, rep) = run("true");
    assert_eq!(rep.tool_counts.get("bash"), Some(&2));
    assert_eq!(rep.tool_counts.get("read"), Some(&1));
    assert_eq!(rep.tool_counts.values().sum::<u32>(), rep.tool_calls);
}
