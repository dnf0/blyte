//! End-to-end: a real repository, a real gate, a real model.
//! `cargo run -p blyte-agent --release --example solve`
use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Runner};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::state::State;
use blyte_provider::{gemini::Gemini, Provider};
use std::path::PathBuf;

fn main() {
    let dir = tempfile::tempdir().unwrap();
    let root: PathBuf = dir.path().into();
    let sh = LocalShell;
    let t = std::time::Duration::from_secs(60);

    // A tiny Python project with one failing test. The bug is real: sum of an empty list.
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("stats.py"),
        "def mean(xs):\n    return sum(xs) / len(xs)\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/test_stats.py"),
        "from stats import mean\n\n\
         def test_mean():\n    assert mean([1, 2, 3]) == 2\n\n\
         def test_mean_of_empty_is_zero():\n    assert mean([]) == 0\n",
    )
    .unwrap();
    sh.exec(
        "git init -q && git config user.email a@b && git config user.name c \
             && git add -A && git commit -qm base",
        &root,
        t,
    )
    .unwrap();

    let provider = match Gemini::from_env() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("model     : {}", provider.model_id());

    let mut state = State::new(
        "The test suite fails. Find out why and fix the source so every test passes. \
         Do not change the tests.",
        "python3 -m pytest tests/ -q",
    );

    // Printed as it happens. A run that is working and a run stuck on `find /` must not look
    // the same for three minutes.
    let log = |e: blyte_agent::run::Event<'_>| {
        use blyte_agent::run::Event::*;
        match e {
            Turn { n, text, .. } if !text.is_empty() => {
                println!("  turn {n}: {}", text.lines().next().unwrap_or(""))
            }
            Turn { n, .. } => println!("  turn {n}:"),
            Tool {
                name,
                input,
                summary,
                is_error,
            } => println!(
                "    {} {name:<5} {input}\n           -> {summary}",
                if is_error { "!" } else { "-" }
            ),
            GateStart { attempt } => println!("    GATE #{attempt} START"),
            GateSlotWait { ms } => println!("    GATE WAIT {ms}ms"),
            Gate {
                attempt,
                passed,
                output,
            } => println!(
                "    GATE #{attempt} {} | {output}",
                if passed { "PASS" } else { "FAIL" }
            ),
            RolledBack { to } => println!("    reset to {to}"),
        }
    };

    let runner = Runner {
        provider: &provider,
        shell: &sh,
        fs: &LocalFs,
        context: &Conversational,
        root: root.clone(),
        budget: Budget {
            max_turns: 20,
            max_gate_attempts: 4,
            max_duration: None,
        },
        rollback: true,
        on_event: Some(&log),
        research: false,
        claim_check: false,
    };

    let started = std::time::Instant::now();
    let (outcome, report) = runner.run(&mut state);
    println!("outcome   : {outcome:?}");
    println!(
        "turns     : {}  tool calls: {}  gate attempts: {}",
        report.outcome_turns, report.tool_calls, report.gate_attempts
    );
    println!(
        "empty     : {} (turns that did nothing)",
        report.empty_turns
    );
    println!("usage     : {:?}", report.usage);
    println!("elapsed   : {:.0}s", started.elapsed().as_secs_f64());
    println!(
        "\n--- stats.py as left ---\n{}",
        std::fs::read_to_string(root.join("stats.py")).unwrap_or_default()
    );
}
