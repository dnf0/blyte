//! End-to-end integration tests for harness-observed working set tracking and bounded context.
//!
//! Validates:
//! 1. REPL `/state` displays active working set files and last test failure cleanly.
//! 2. Multi-turn coordination: `read`, `edit`, and `bash` synchronously update working set
//!    and test failure state observable by the runner.
//! 3. Zero-latency loop breaker intercepts redundant reads during runner execution.
//! 4. Working set enforces LRU capacity bounds across multi-file workflows.

use std::path::PathBuf;
use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::state::State;
use blyte_agent::working_set::FileSnippet;
use blyte_cli::repl::{handle_repl_command, ReplConfig, ReplControl, ReplSession};
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall, Usage};
use serde_json::json;

/// Replays a script of provider responses while capturing all requests for invariant inspection.
struct ScriptedProvider {
    script: Mutex<Vec<Response>>,
    seen: Mutex<Vec<Request>>,
}

impl ScriptedProvider {
    fn new(script: Vec<Response>) -> Self {
        Self {
            script: Mutex::new(script),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Provider for ScriptedProvider {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        let mut s = self.script.lock().unwrap();
        if s.is_empty() {
            return Ok(Response::default());
        }
        Ok(s.remove(0))
    }

    fn model_id(&self) -> String {
        "scripted-working-set-test".into()
    }
}

fn says(text: &str) -> Response {
    Response {
        text: text.into(),
        ..Default::default()
    }
}

fn calls(name: &str, args: serde_json::Value) -> Response {
    Response {
        tool_calls: vec![ToolCall {
            id: format!("call_{name}"),
            name: name.into(),
            arguments: args,
            signature: None,
        }],
        usage: Usage {
            input: 60,
            cached: 0,
            output: 25,
            thinking: 0,
        },
        ..Default::default()
    }
}

fn create_temp_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("failed to create temp git dir");
    let shell = LocalShell;
    shell
        .exec(
            "git init -q && git config user.email test@blyte.local && git config user.name tester",
            dir.path(),
            std::time::Duration::from_secs(30),
        )
        .expect("git init failed");
    std::fs::write(dir.path().join("README.md"), "# Test Working Set Repo\n").unwrap();
    shell
        .exec(
            "git add -A && git commit -qm 'initial commit'",
            dir.path(),
            std::time::Duration::from_secs(30),
        )
        .expect("git commit failed");
    dir
}

#[test]
fn test_repl_state_command_displays_working_set_and_failure() {
    let config = ReplConfig {
        repo_root: PathBuf::from("."),
        model: Some("gemini-3.8-flash".to_string()),
        gate: Some("cargo test".to_string()),
        context_strategy: "state-only".to_string(),
        container: None,
        max_turns: 60,
        max_gates: 5,
        rollback: true,
    };
    let mut session = ReplSession::new(&config);

    // Populate active working set files and last test failure
    session.state.runtime.working_set.files.push(FileSnippet {
        path: "src/lru.py".into(),
        content: "def get(key):\n    return None\n".into(),
        start_line: Some(1),
        end_line: Some(2),
        last_accessed_turn: 2,
    });
    session.state.runtime.working_set.files.push(FileSnippet {
        path: "tests/test_lru.py".into(),
        content: "def test_get():\n    assert get(1) == 1\n".into(),
        start_line: None,
        end_line: None,
        last_accessed_turn: 1,
    });
    session.state.runtime.working_set.last_failure =
        Some("AssertionError: assert None == 1\n  where None = get(1)".into());

    let mut output = Vec::new();
    let res = handle_repl_command(&mut session, "/state", &mut output).unwrap();
    assert_eq!(res, ReplControl::Handled);

    let text = String::from_utf8(output).expect("output must be valid UTF-8");

    // Must display standard SKILL.state sections
    assert!(text.contains("=== Blyte SKILL.state ==="));
    assert!(text.contains("Orchestrator:"));
    assert!(text.contains("Runtime:"));
    assert!(text.contains("Agent:"));

    // Must display clean, dedicated Active Working Set section
    assert!(text.contains("Active Working Set:"));
    assert!(text.contains("`src/lru.py` (lines 1-2) [turn 2]"));
    assert!(text.contains("`tests/test_lru.py` [turn 1]"));

    // Must display clean, dedicated Last Test Failure section
    assert!(text.contains("Last Test Failure:"));
    assert!(text.contains("AssertionError: assert None == 1"));

    // Clear command resets working set
    let mut clear_out = Vec::new();
    let res_clear = handle_repl_command(&mut session, "/clear", &mut clear_out).unwrap();
    assert_eq!(res_clear, ReplControl::Handled);
    assert!(session.state.runtime.working_set.files.is_empty());
    assert!(session.state.runtime.working_set.last_failure.is_none());
}

#[test]
fn test_working_set_multi_file_coordination_and_context_invariants() {
    let repo = create_temp_git_repo();
    // Write initial source and test files and commit them to baseline git state
    let calc_py = "def add(a, b):\n    return a - b\n";
    let test_py = "import sys\nfrom calc import add\nif add(2, 3) != 5:\n    print('FAIL: 2+3!=5', file=sys.stderr)\n    sys.exit(1)\nprint('PASS')\n";
    std::fs::write(repo.path().join("calc.py"), calc_py).unwrap();
    std::fs::write(repo.path().join("test_calc.py"), test_py).unwrap();
    let shell = LocalShell;
    shell
        .exec(
            "git add -A && git commit -qm 'add calc and test'",
            repo.path(),
            std::time::Duration::from_secs(30),
        )
        .expect("git commit failed");

    let mut state = State::new("Fix calc.py bug", "python3 -B test_calc.py");

    // Scripted session:
    // Turn 1: Model reads calc.py -> working set captures calc.py
    // Turn 2: Model runs test -> fails -> working set captures last failure
    // Turn 3: Model edits calc.py -> working set updates calc.py
    // Turn 4: Model runs test -> passes -> working set clears failure
    // Turn 5: Model states ready -> gate executes and passes
    let provider = ScriptedProvider::new(vec![
        calls(
            "read",
            json!({"path": "calc.py", "start_line": 1, "end_line": 2}),
        ),
        calls("bash", json!({"command": "python3 -B test_calc.py"})),
        calls(
            "edit",
            json!({
                "path": "calc.py",
                "old": "return a - b",
                "new": "return a + b"
            }),
        ),
        calls("bash", json!({"command": "python3 -B test_calc.py"})),
        says("calc.py fixed and verified."),
    ]);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: repo.path().to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 2,
            max_duration: None,
        },
        rollback: true,
        loop_breaker: true,
        on_event: None,
    };

    let (outcome, report) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.outcome_turns, 5);

    // 1. Verify working set state at end of execution
    assert_eq!(state.runtime.working_set.files.len(), 1);
    let cached = &state.runtime.working_set.files[0];
    assert_eq!(cached.path, "calc.py");
    assert!(cached.content.contains("return a + b"));
    assert_eq!(state.runtime.working_set.last_failure, None);
}

#[test]
fn test_working_set_guard_intercepts_redundant_read_e2e() {
    let repo = create_temp_git_repo();
    let mut state = State::new("Inspect and solve", "true");

    let source = "def greet():\n    return 'hello'\n";
    std::fs::write(repo.path().join("greet.py"), source).unwrap();

    // Turn 1: Model reads greet.py
    // Turn 2: Model immediately reads greet.py again -> guard intercepts!
    // Turn 3: Model finishes
    let provider = ScriptedProvider::new(vec![
        calls("read", json!({"path": "greet.py"})),
        calls("read", json!({"path": "greet.py"})),
        says("Done reading."),
    ]);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: repo.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    };

    let (outcome, report) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.outcome_turns, 3);

    // Verify action ledger recorded the interception
    assert_eq!(state.runtime.actions.len(), 2);
    assert_eq!(state.runtime.actions[0].tool, "read");
    assert_eq!(state.runtime.actions[0].summary, "2 lines read");

    assert_eq!(state.runtime.actions[1].tool, "read");
    assert_eq!(
        state.runtime.actions[1].summary,
        "intercepted: already in working set"
    );
    assert!(!state.runtime.actions[1].is_error);

    // Verify working set retains the original snippet without corruption
    assert_eq!(state.runtime.working_set.files.len(), 1);
    assert_eq!(state.runtime.working_set.files[0].path, "greet.py");
    assert_eq!(
        state.runtime.working_set.files[0].content.trim(),
        "def greet():\n    return 'hello'"
    );
}

#[test]
fn test_working_set_multi_file_lru_pruning_e2e() {
    let repo = create_temp_git_repo();
    let mut state = State::new("Read multiple files", "true");

    std::fs::write(repo.path().join("f1.txt"), "file 1 content\n").unwrap();
    std::fs::write(repo.path().join("f2.txt"), "file 2 content\n").unwrap();
    std::fs::write(repo.path().join("f3.txt"), "file 3 content\n").unwrap();
    std::fs::write(repo.path().join("f4.txt"), "file 4 content\n").unwrap();

    let provider = ScriptedProvider::new(vec![
        calls("read", json!({"path": "f1.txt"})),
        calls("read", json!({"path": "f2.txt"})),
        calls("read", json!({"path": "f3.txt"})),
        calls("read", json!({"path": "f4.txt"})),
        says("All files read."),
    ]);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: repo.path().to_path_buf(),
        budget: Budget {
            max_turns: 6,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    };

    let (outcome, _) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);

    // MAX_ACTIVE_FILES is 3. f1.txt (accessed in turn 1) should be evicted;
    // f2.txt, f3.txt, f4.txt should remain.
    let paths: Vec<&str> = state
        .runtime
        .working_set
        .files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(paths, vec!["f2.txt", "f3.txt", "f4.txt"]);
}

#[test]
fn test_loop_breaker_serves_an_insisted_read_e2e() {
    let repo = create_temp_git_repo();
    let mut state = State::new("Inspect and solve", "true");
    std::fs::write(
        repo.path().join("greet.py"),
        "def greet():\n    return 'hello'\n",
    )
    .unwrap();

    // Turn 2 is intercepted. Turn 3 repeats it verbatim: insisting wins, so it is served
    // rather than refused again (FINDINGS 13.22 / 13.25).
    let provider = ScriptedProvider::new(vec![
        calls("read", json!({"path": "greet.py"})),
        calls("read", json!({"path": "greet.py"})),
        calls("read", json!({"path": "greet.py"})),
        says("Done reading."),
    ]);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: repo.path().to_path_buf(),
        budget: Budget {
            max_turns: 6,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    };

    let (outcome, _) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);
    let summaries: Vec<&str> = state
        .runtime
        .actions
        .iter()
        .map(|a| a.summary.as_str())
        .collect();
    assert_eq!(
        summaries,
        vec![
            "2 lines read",
            "intercepted: already in working set",
            "2 lines read"
        ]
    );
}

#[test]
fn test_loop_breaker_off_executes_every_read_e2e() {
    let repo = create_temp_git_repo();
    let mut state = State::new("Inspect and solve", "true");
    std::fs::write(
        repo.path().join("greet.py"),
        "def greet():\n    return 'hello'\n",
    )
    .unwrap();

    let provider = ScriptedProvider::new(vec![
        calls("read", json!({"path": "greet.py"})),
        calls("read", json!({"path": "greet.py"})),
        says("Done reading."),
    ]);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: repo.path().to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };

    let (outcome, _) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);
    let summaries: Vec<&str> = state
        .runtime
        .actions
        .iter()
        .map(|a| a.summary.as_str())
        .collect();
    assert_eq!(summaries, vec!["2 lines read", "2 lines read"]);
}

#[test]
fn test_budget_max_duration_ends_the_run_before_the_next_turn() {
    let repo = create_temp_git_repo();
    let mut state = State::new("Inspect and solve", "true");

    let provider = ScriptedProvider::new(vec![calls("read", json!({"path": "greet.py"}))]);

    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: repo.path().to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 1,
            max_duration: Some(std::time::Duration::ZERO),
        },
        rollback: false,
        loop_breaker: true,
        on_event: None,
    };

    let (outcome, report) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert_eq!(report.outcome_turns, 0);
    assert!(
        provider.seen.lock().unwrap().is_empty(),
        "no model call may start once the deadline has passed"
    );
}
