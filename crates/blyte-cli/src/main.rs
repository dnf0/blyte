use blyte_cli::cli::Cli;
use clap::Parser;
use std::path::PathBuf;

/// Restore the default SIGPIPE behaviour.
///
/// Rust disables SIGPIPE at start-up, so writing to a closed pipe returns EPIPE
/// and `println!` panics on it. That turns the ordinary `… | head` or quitting
/// out of a pager into a panic and a backtrace note. Restoring the default makes
/// the process exit quietly, as every other Unix command line tool does.
fn restore_sigpipe() {
    // Safety: setting a signal disposition to the default is always sound, and
    // this runs before any threads are spawned.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

fn main() {
    restore_sigpipe();
    let cli = Cli::parse();

    if let Some(ref prompt) = cli.prompt {
        if prompt.trim().is_empty() {
            eprintln!("Error: prompt cannot be empty");
            std::process::exit(1);
        }
        let config = blyte_cli::task_runner::TaskConfig::from(&cli);
        match blyte_cli::task_runner::run_task(config) {
            Ok((blyte_agent::run::Outcome::Verified, report)) => {
                println!(
                    "Task verified in {} turns (usage: {:?})",
                    report.outcome_turns, report.usage
                );
                std::process::exit(0);
            }
            Ok((outcome, report)) => {
                eprintln!(
                    "Task finished with outcome: {outcome:?} in {} turns",
                    report.outcome_turns
                );
                std::process::exit(1);
            }
            Err(err) => {
                eprintln!("Error executing task: {err}");
                std::process::exit(1);
            }
        }
    }

    let repl_config = blyte_cli::repl::ReplConfig {
        repo_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        model: cli.model,
        gate: cli.gate,
        context_strategy: cli.context,
        container: cli.container,
        max_turns: cli.max_turns,
        max_gates: cli.max_gates,
        rollback: cli.rollback,
    };
    if let Err(e) = blyte_cli::repl::run_repl(repl_config) {
        eprintln!("REPL error: {e}");
        std::process::exit(1);
    }
}
