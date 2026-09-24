# Harness Benchmark Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** `cargo run --release -p blyte-agent --example harness_bench` measures **the harness's own time** with no model: a scripted provider replays a realistic 200-turn trajectory (reads, ranged reads, greps, globs, writes, edits, shell commands) at zero model latency, against a copy of this repository's `crates/`, either locally or inside a Docker container (`--image`). It reports:
- total harness time and per-turn p50/p95;
- per-tool latency;
- time building context between the last tool and the next model call.

It writes JSON (`--out`) so runs can be compared.

**Why:** The objective is a harness that is fast and good: overhead near zero, and the model never held back. Harness overhead was estimated at about 10% of wall time (each `docker exec` costs 60–110 ms; `docs/research/2026-09-24-harness-research.md`). Every speed change from here is judged by this benchmark, plus equivalence tests showing the model sees exactly the same thing. No model runs, no cost.

**Design:**
- **Timing.**
  - The scripted provider records when each `complete` is called and when it returns.
  - The runner's `on_event` records each `Event::Tool`.
  - Tool latency is the time since the previous marker (the last `complete` return, or the previous tool event in the same turn).
  - Context time is the time from the turn's last tool event to the next `complete` call.
- **Realism.** The loop-breaker is off (the default is on), so every call actually executes and runs are comparable. The context is `Folded::default()`, the default strategy.
- **Determinism.** The trajectory is a fixed 20-step cycle over the fixture's files: no randomness, no new dependencies.

**Tech Stack:** Rust 2021 (std only; `serde_json` and `tempfile` are already dependencies).

## Global Constraints

- Base branch: `feat/harness-bench` (from `main`, after the blyte rename). The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- You may run the benchmark locally (`--turns 40`). Do NOT pull or run Docker images (no `--image`); Claude runs the container mode.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `blyte-mcp`'s `test_blyte_status_and_cancel_lifecycle` and `blyte-cli`'s `test_cli_daemon_detached_forwarding` are known to be timing-flaky: if one alone fails, re-run once and report both results.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `harness_bench` example

**Files:**
- Create: `crates/blyte-agent/examples/harness_bench.rs`
- Test: `crates/blyte-agent/tests/test_harness_bench_trajectory.rs` (create)
- Modify: `crates/blyte-agent/src/lib.rs` (one module)
- Create: `crates/blyte-agent/src/bench.rs`

- [x] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_harness_bench_trajectory.rs`:

```rust
//! The benchmark's trajectory is deterministic, realistic in mix, and every step is valid
//! against its fixture, so a benchmark run measures tools, not errors.

use blyte_agent::bench::{percentile, trajectory};

fn files() -> Vec<String> {
    (0..10).map(|i| format!("crates/x/src/f{i}.rs")).collect()
}

#[test]
fn the_trajectory_is_deterministic_and_has_the_intended_mix() {
    let a = trajectory(&files(), 200);
    let b = trajectory(&files(), 200);
    assert_eq!(a, b);
    assert_eq!(a.len(), 200);
    let count = |n: &str| a.iter().filter(|(name, _)| name == n).count();
    assert_eq!(count("read"), 60);
    assert_eq!(count("grep"), 30);
    assert_eq!(count("glob"), 10);
    assert_eq!(count("write"), 20);
    assert_eq!(count("edit"), 20);
    assert_eq!(count("bash"), 60);
}

#[test]
fn every_edit_targets_a_file_written_earlier_with_its_exact_text() {
    let t = trajectory(&files(), 200);
    for (i, (name, args)) in t.iter().enumerate() {
        if name != "edit" {
            continue;
        }
        let path = args["path"].as_str().unwrap();
        let old = args["old"].as_str().unwrap();
        let written = t[..i].iter().any(|(n, a)| {
            n == "write"
                && a["path"].as_str() == Some(path)
                && a["content"].as_str().is_some_and(|c| c.contains(old))
        });
        assert!(written, "edit at step {i} has no prior write of {path} containing {old:?}");
    }
}

#[test]
fn reads_use_fixture_files_and_percentiles_are_nearest_rank() {
    let t = trajectory(&files(), 40);
    for (name, args) in &t {
        if name == "read" {
            assert!(files().iter().any(|f| Some(f.as_str()) == args["path"].as_str()));
        }
    }
    let xs = [5.0, 1.0, 3.0, 2.0, 4.0];
    assert_eq!(percentile(&xs, 50.0), 3.0);
    assert_eq!(percentile(&xs, 95.0), 5.0);
    assert_eq!(percentile(&[], 50.0), 0.0);
}
```

- [x] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-agent --test test_harness_bench_trajectory`. Expected: a compile error, because `blyte_agent::bench` does not exist.

- [x] **Step 3: The trajectory module.** Create `crates/blyte-agent/src/bench.rs`:

```rust
//! A model-free benchmark of the harness's own time: a fixed trajectory of tool calls, replayed
//! at zero model latency (`examples/harness_bench.rs`). Every speed change is judged by it.

use serde_json::{json, Value};

/// A realistic mix of tool calls over `files`, deterministic, `turns` long: per 20 steps, 6 reads
/// (2 of them ranged), 3 greps, 1 glob, 2 writes, 2 edits (each of a file written just before,
/// with its exact text) and 6 shell commands.
pub fn trajectory(files: &[String], turns: usize) -> Vec<(String, Value)> {
    let file = |i: usize| files[(i * 7) % files.len().max(1)].clone();
    (0..turns)
        .map(|i| {
            let (name, args) = match i % 20 {
                0 | 8 | 11 | 18 => ("read", json!({ "path": file(i) })),
                3 | 13 => (
                    "read",
                    json!({ "path": file(i), "start_line": 1, "end_line": 40 }),
                ),
                1 => ("grep", json!({ "pattern": "fn " })),
                10 => ("grep", json!({ "pattern": "impl .* for" })),
                17 => ("grep", json!({ "pattern": "Message::" })),
                7 => ("glob", json!({ "pattern": "**/*.rs" })),
                4 | 14 => (
                    "write",
                    json!({ "path": format!("scratch/s{i}.txt"), "content": format!("alpha {i}\nsecond line\n") }),
                ),
                5 | 15 => (
                    "edit",
                    json!({ "path": format!("scratch/s{}.txt", i - 1), "old": format!("alpha {}", i - 1), "new": format!("beta {}", i - 1) }),
                ),
                2 => ("bash", json!({ "command": "ls" })),
                6 => ("bash", json!({ "command": "git status --short | head -20" })),
                9 => ("bash", json!({ "command": "wc -l $(git ls-files | head -50) | tail -1" })),
                12 => ("bash", json!({ "command": "head -30 Cargo.toml" })),
                16 => ("bash", json!({ "command": "echo done" })),
                _ => ("bash", json!({ "command": "find . -name '*.rs' | wc -l" })),
            };
            (name.to_string(), args)
        })
        .collect()
}

/// Nearest-rank percentile of `xs` (any order); 0 for an empty slice.
pub fn percentile(xs: &[f64], p: f64) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((p / 100.0) * v.len() as f64).ceil() as usize;
    v[rank.clamp(1, v.len()) - 1]
}
```

  Add `pub mod bench;` to `crates/blyte-agent/src/lib.rs` (keep the module list alphabetical).

- [x] **Step 4: The example.** Create `crates/blyte-agent/examples/harness_bench.rs`:

```rust
//! Model-free harness benchmark. A scripted provider replays `bench::trajectory` at zero model
//! latency, so every millisecond measured is the harness's own.
//!
//!   cargo run --release -p blyte-agent --example harness_bench -- [--turns N] [--image IMG] [--out FILE]
//!
//! Local mode runs tools on a temporary copy of this repository's `crates/`. With `--image`, the
//! copy is bind-mounted at /work in a fresh container of IMG, removed afterwards.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Instant;

use blyte_agent::bench::{percentile, trajectory};
use blyte_agent::context::Folded;
use blyte_agent::fs::{ContainerFs, Fs, LocalFs};
use blyte_agent::run::{Budget, Event, Runner};
use blyte_agent::shell::{ContainerShell, LocalShell, Shell};
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall};

#[derive(Default)]
struct Clock {
    /// When the last `complete` returned, or the last tool event of the turn.
    mark: Option<Instant>,
    turn_started: Option<Instant>,
    tools: BTreeMap<String, Vec<f64>>,
    context_ms: Vec<f64>,
    turn_ms: Vec<f64>,
}

struct Replay {
    steps: Mutex<Vec<(String, serde_json::Value)>>,
    clock: Mutex<Clock>,
    next_id: Mutex<usize>,
}

impl Provider for Replay {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let now = Instant::now();
        {
            let mut c = self.clock.lock().unwrap();
            if let Some(m) = c.mark {
                let ms = now.duration_since(m).as_secs_f64() * 1e3;
                c.context_ms.push(ms);
            }
            if let Some(t) = c.turn_started {
                let ms = now.duration_since(t).as_secs_f64() * 1e3;
                c.turn_ms.push(ms);
            }
        }
        let mut steps = self.steps.lock().unwrap();
        let resp = if steps.is_empty() {
            Response {
                text: "done".into(),
                ..Default::default()
            }
        } else {
            let (name, arguments) = steps.remove(0);
            let mut id = self.next_id.lock().unwrap();
            *id += 1;
            Response {
                tool_calls: vec![ToolCall {
                    id: format!("call_{id}"),
                    name,
                    arguments,
                    signature: None,
                }],
                ..Default::default()
            }
        };
        let done = Instant::now();
        let mut c = self.clock.lock().unwrap();
        c.mark = Some(done);
        c.turn_started = Some(done);
        Ok(resp)
    }
    fn model_id(&self) -> String {
        "replay".into()
    }
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let (s, d) = (e.path(), dst.join(e.file_name()));
        if e.file_type().unwrap().is_dir() {
            if e.file_name() != "target" {
                copy_dir(&s, &d);
            }
        } else {
            std::fs::copy(&s, &d).unwrap();
        }
    }
}

fn rs_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rs_files(root, &p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p.strip_prefix(root).unwrap().to_string_lossy().into_owned());
        }
    }
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let turns: usize = arg(&args, "--turns").map_or(200, |t| t.parse().expect("--turns N"));
    let image = arg(&args, "--image");
    let out = arg(&args, "--out");

    // Fixture: this repository's crates/ and Cargo.toml, in a fresh git repository.
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tmp = tempfile::tempdir().unwrap();
    let fixture = tmp.path().join("work");
    copy_dir(&repo.join("crates"), &fixture.join("crates"));
    std::fs::copy(repo.join("Cargo.toml"), fixture.join("Cargo.toml")).unwrap();
    std::fs::create_dir_all(fixture.join("scratch")).unwrap();
    let git = |a: &[&str]| {
        Command::new("git")
            .args(a)
            .current_dir(&fixture)
            .output()
            .unwrap()
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&[
        "-c", "user.name=bench", "-c", "user.email=bench@localhost",
        "commit", "-q", "--no-gpg-sign", "-m", "fixture",
    ]);
    let mut files = Vec::new();
    rs_files(&fixture, &fixture.join("crates"), &mut files);
    files.sort();

    let container = image.as_ref().map(|img| {
        let o = Command::new("docker")
            .args(["run", "-d", "--rm", "-v"])
            .arg(format!("{}:/work", fixture.display()))
            .args(["-w", "/work", img, "sleep", "3600"])
            .output()
            .expect("docker run");
        assert!(o.status.success(), "docker run failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    });

    let replay = Replay {
        steps: Mutex::new(trajectory(&files, turns)),
        clock: Mutex::new(Clock::default()),
        next_id: Mutex::new(0),
    };
    let on_event = |e: Event<'_>| {
        if let Event::Tool { name, .. } = e {
            let now = Instant::now();
            let mut c = replay.clock.lock().unwrap();
            if let Some(m) = c.mark {
                let ms = now.duration_since(m).as_secs_f64() * 1e3;
                c.tools.entry(name.to_string()).or_default().push(ms);
            }
            c.mark = Some(now);
        }
    };

    let (local_shell, local_fs) = (LocalShell, LocalFs);
    let container_shell = container.as_ref().map(|id| ContainerShell {
        container_id: id.clone(),
        workdir: PathBuf::from("/work"),
    });
    let container_fs = container.as_ref().map(|id| ContainerFs {
        container_id: id.clone(),
    });
    let (shell, fs, root): (&dyn Shell, &dyn Fs, PathBuf) = match (&container_shell, &container_fs)
    {
        (Some(s), Some(f)) => (s, f, PathBuf::from("/work")),
        _ => (&local_shell, &local_fs, fixture.clone()),
    };
    let context = Folded::default();
    let runner = Runner {
        provider: &replay,
        shell,
        fs,
        context: &context,
        root,
        budget: Budget {
            max_turns: (turns + 5) as u32,
            max_gate_attempts: 1,
            max_duration: None,
            max_cost_usd: None,
        },
        rollback: false,
        loop_breaker: false,
        on_event: Some(&on_event),
    };
    let started = Instant::now();
    let _ = runner.run(&mut State::new("benchmark", "true"));
    let total_ms = started.elapsed().as_secs_f64() * 1e3;
    if let Some(id) = &container {
        let _ = Command::new("docker").args(["rm", "-f", id]).output();
    }

    let c = replay.clock.lock().unwrap();
    let mode = image.map_or("local".to_string(), |i| format!("container:{i}"));
    let mut tools = serde_json::Map::new();
    println!("harness_bench  mode={mode}  turns={turns}  total={total_ms:.0} ms");
    println!(
        "  per turn   p50 {:>8.2} ms   p95 {:>8.2} ms",
        percentile(&c.turn_ms, 50.0),
        percentile(&c.turn_ms, 95.0)
    );
    println!(
        "  context    p50 {:>8.3} ms   p95 {:>8.3} ms   (last tool -> next model call)",
        percentile(&c.context_ms, 50.0),
        percentile(&c.context_ms, 95.0)
    );
    for (name, xs) in &c.tools {
        let (p50, p95) = (percentile(xs, 50.0), percentile(xs, 95.0));
        println!("  {name:<6} n={:<4} p50 {p50:>8.2} ms   p95 {p95:>8.2} ms", xs.len());
        tools.insert(
            name.clone(),
            serde_json::json!({ "n": xs.len(), "p50_ms": p50, "p95_ms": p95 }),
        );
    }
    if let Some(path) = out {
        let json = serde_json::json!({
            "mode": mode,
            "turns": turns,
            "total_ms": total_ms,
            "turn_p50_ms": percentile(&c.turn_ms, 50.0),
            "turn_p95_ms": percentile(&c.turn_ms, 95.0),
            "context_p50_ms": percentile(&c.context_ms, 50.0),
            "context_p95_ms": percentile(&c.context_ms, 95.0),
            "tools": tools,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    }
}
```

- [x] **Step 5: Run the tests.** Run: `cargo test -p blyte-agent --test test_harness_bench_trajectory`. Expected: 3 passed.

- [x] **Step 6: Run the benchmark locally.** Run: `cargo run --release -p blyte-agent --example harness_bench -- --turns 40`. Expected: it prints a `harness_bench mode=local turns=40 total=...` line, per-turn and context lines, and one line per tool (read, grep, glob, write, edit, bash). Report the output verbatim.

- [x] **Step 7: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [x] **Step 8: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(agent): model-free harness benchmark (harness_bench example)"
```
