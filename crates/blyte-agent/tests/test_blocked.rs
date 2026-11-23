//! A check that fails before the change is reported as blocked, proven at the base commit, and a
//! model that only claims it is not blocked keeps the old behaviour (2026-10-01).

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::{Output, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

struct DirectShell;

impl Shell for DirectShell {
    fn exec(&self, command: &str, cwd: &Path, _timeout: Duration) -> Result<Output, String> {
        if command.starts_with("for c in ") {
            return Ok(Output {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            });
        }
        if command.trim() == "test -f ok"
            || (command.contains("BLYTE_IN_GATE=1") && command.contains("\ntest -f ok\n"))
        {
            return Ok(Output {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: if cwd.join("ok").is_file() { 0 } else { 1 },
            });
        }
        if command.trim() == "rm ok" {
            let exit_code = if std::fs::remove_file(cwd.join("ok")).is_ok() {
                0
            } else {
                1
            };
            return Ok(Output {
                stdout: String::new(),
                stderr: String::new(),
                exit_code,
            });
        }
        let out = Command::new("sh")
            .args(["-c", command])
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("spawn failed: {e}"))?;
        Ok(Output {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            exit_code: out.status.code().unwrap_or(-1),
        })
    }

    fn describe(&self) -> String {
        String::new()
    }
}

/// Plays its responses in order: each is either a bash command (a tool call) or a reply that stops.
struct Script {
    steps: Mutex<Vec<Result<String, String>>>,
}

impl Provider for Script {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let mut s = self.steps.lock().unwrap();
        let next = if s.is_empty() {
            Err("Nothing more to do.".to_string())
        } else {
            s.remove(0)
        };
        Ok(match next {
            Ok(cmd) => Response {
                tool_calls: vec![ToolCall {
                    id: "c".into(),
                    name: "bash".into(),
                    arguments: json!({"command": cmd}),
                    signature: None,
                }],
                ..Default::default()
            },
            Err(text) => Response {
                text,
                ..Default::default()
            },
        })
    }
    fn model_id(&self) -> String {
        "gemini/gemini-3.8-flash".into()
    }
}

fn repo(with_ok: bool) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        assert!(Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(d.path())
            .status()
            .unwrap()
            .success())
    };
    git(&["init", "-q"]);
    std::fs::write(d.path().join("README"), "x\n").unwrap();
    if with_ok {
        std::fs::write(d.path().join("ok"), "\n").unwrap();
    }
    git(&["add", "."]);
    git(&["commit", "-qm", "base", "--no-gpg-sign"]);
    d
}

fn run(dir: &std::path::Path, steps: Vec<Result<String, String>>) -> (Outcome, u32) {
    let p = Script {
        steps: Mutex::new(steps),
    };
    let r = Runner {
        provider: &p,
        shell: &DirectShell,
        fs: &LocalFs,
        context: &Conversational,
        root: dir.to_path_buf(),
        budget: Budget {
            max_turns: 12,
            max_gate_attempts: 4,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
        base_check: blyte_agent::run::BaseCheck::None,
    };
    let mut st = State::new("make the check pass", "test -f ok");
    let (outcome, report) = r.run(&mut st);
    (outcome, report.gate_attempts)
}

#[test]
fn a_check_failing_at_the_base_too_is_blocked_without_another_gate_run() {
    let d = repo(false);
    let (outcome, attempts) = run(
        d.path(),
        vec![
            Err("Done.".into()),
            Err("The failure is not caused by my change; that check was failing before.".into()),
        ],
    );
    match outcome {
        Outcome::Blocked(why) => {
            assert!(
                why.contains("test -f ok") && why.contains("not caused by my change"),
                "{why}"
            );
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
    assert_eq!(
        attempts, 1,
        "the gate is not run again once the base is shown to fail"
    );
    let wts = Command::new("git")
        .args(["worktree", "list"])
        .current_dir(d.path())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&wts.stdout).lines().count(),
        1,
        "the base worktree is removed"
    );
}

#[test]
fn a_wrong_claim_is_checked_and_the_run_carries_on() {
    // The base passes (ok exists); the model deletes ok, then claims the failure is not its doing.
    let d = repo(true);
    let (outcome, attempts) = run(
        d.path(),
        vec![
            Ok("rm ok".into()),
            Err("Done.".into()),
            Err("This failure is not caused by my change.".into()),
            Err("Done.".into()),
            Err("Done.".into()),
        ],
    );
    assert_eq!(outcome, Outcome::GateFailed);
    assert_eq!(attempts, 4);
}
