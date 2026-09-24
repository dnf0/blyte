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
        "-c",
        "user.name=bench",
        "-c",
        "user.email=bench@localhost",
        "commit",
        "-q",
        "--no-gpg-sign",
        "-m",
        "fixture",
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
        assert!(
            o.status.success(),
            "docker run failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );
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
        println!(
            "  {name:<6} n={:<4} p50 {p50:>8.2} ms   p95 {p95:>8.2} ms",
            xs.len()
        );
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
