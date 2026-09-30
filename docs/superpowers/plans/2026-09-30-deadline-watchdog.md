# A run never outlives its deadline — Plan

A dispatch in provision (R3T2, 2026-09-30) ran 2 h 49 min under `--max-seconds 3600` and left no
report. Its log stopped after turn 60's last tool, and blyte's only children were idle shells: it
was waiting on the model call for turn 61. `--max-seconds` is checked only between turns and
before a retry's sleep; a request in flight is bounded only by ureq's per-read timeout (300 s),
which trickling bytes or a stuck pooled connection can outlast, and no report is written for a run
that never returns.

## Global Constraints

- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.
- Create the new test file exactly as given; do not edit existing test files. No new dependencies.
- Conventional commit, `--no-gpg-sign`, the message given.

### Task 1: a request limit, and a watchdog that reports and exits

**Files:** `crates/blyte-provider/src/lib.rs` (`http_client`), `crates/blyte-cli/src/task_runner.rs`
(`run_task`), new `crates/blyte-cli/tests/test_deadline_watchdog.rs`, `docs/usage.md` (one sentence
where `--max-seconds` is described: the run stops, with a report, at most 30 s after the budget
even if a model call or command never returns).

1. **Request limit.** `http_client()` also sets `.timeout(Duration::from_secs(600))` (ureq's limit
   on a whole request), keeping the connect and read timeouts. A request that hits it is an error
   like any other network error (the existing retry logic decides what happens next).
2. **Watchdog.** In `run_task`, when `config.max_seconds` is `Some(s)`, start a detached thread
   before running the task. It sleeps until `s + 30` seconds after `started`; if the process is
   still running then, it:
   - writes the report, when `config.report` is set, with
     `RunRecord::from_error(&config, &msg, started.elapsed())`, where `msg` is
     `format!("stopped {} s past --max-seconds {s}: a model call or command never returned", 30)`;
   - prints `blyte: <msg>` to stderr;
   - exits the process with status 1 (`std::process::exit(1)`), the status of any run that is not
     verified.
   Give the thread its own clone of the config. Nothing else changes: a run that finishes in time
   never sees the watchdog.

- [ ] **Step 1: test.** Create `crates/blyte-cli/tests/test_deadline_watchdog.rs` exactly:

```rust
//! A model call that never returns cannot keep a run past its deadline, and the run still leaves
//! a report: a dispatch sat 2 h 49 min under --max-seconds 3600 with none (2026-09-30).

use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn a_hung_model_call_ends_at_the_deadline_with_a_report() {
    // A server that accepts connections and never answers.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for s in listener.incoming().flatten() {
            held.push(s);
        }
    });

    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("r");
    let home = d.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    assert!(Command::new("git").args(["init", "-q"]).current_dir(&repo).status().unwrap().success());
    let report = d.path().join("report.json");

    let started = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["say hello", "--gate", "true", "--max-seconds", "2", "--sandbox", "off", "--report"])
        .arg(&report)
        .current_dir(&repo)
        .env("HOME", &home)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_HOST")
        .env_remove("OLLAMA_API_KEY")
        .env("OPENAI_BASE_URL", format!("http://127.0.0.1:{port}/v1"))
        .env("OPENAI_API_KEY", "test")
        .env("BLYTE_MODEL", "openai/test-model")
        .output()
        .unwrap();
    let took = started.elapsed();

    assert!(took < Duration::from_secs(60), "ran {took:?}");
    assert!(took >= Duration::from_secs(30), "ended before the watchdog: {took:?}");
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("past --max-seconds 2"), "{err}");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("a report")).unwrap();
    assert!(v["error"].as_str().unwrap().contains("never returned"), "{v}");
}
```

- [ ] **Step 2:** implement until the check passes. If `BLYTE_MODEL=openai/test-model` does not
  select the OpenAI-compatible provider with this base URL, find the selector the code uses for it
  (`crates/blyte-provider/src/registry.rs`) and say so in your report; do not change the test.
- [ ] **Step 3: commit** `fix(cli): a run never outlives --max-seconds by more than 30 s, and always leaves a report`.
