use blyte_cli::cli::{Cli, Commands};
use blyte_cli::commands;
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

    let result = match cli.command {
        Some(Commands::Doctor) => commands::doctor::run_doctor(),
        Some(Commands::Mcp) => blyte_mcp::server::run_stdio_mcp_server().map_err(|e| e.to_string()),
        Some(Commands::Review {
            commit_range,
            repo,
            driver,
            state_path,
            record,
            json,
            fail_on_blocking,
        }) => commands::review::cmd_review(commands::review::ReviewArgs {
            commit_range,
            repo,
            driver,
            state_path,
            record,
            json,
            fail_on_blocking,
        }),
        Some(Commands::Init {
            dest,
            language,
            strictness,
        }) => commands::init::cmd_init_rules(&dest, &language, &strictness),
        Some(Commands::State { cmd }) => blyte_cli::state_cli::dispatch(cmd),
        Some(Commands::Worktree { cmd }) => commands::worktree::dispatch_worktree(cmd),
        Some(Commands::Daemon(args)) => match args.command {
            Some(blyte_cli::state_cli::DaemonSubcommands::List { repo, json }) => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let repo_root = repo.unwrap_or_else(|| {
                    let mut cur = Some(cwd.as_path());
                    while let Some(d) = cur {
                        if d.join(".agent-state").exists() || d.join(".git").exists() {
                            return d.to_path_buf();
                        }
                        cur = d.parent();
                    }
                    cwd
                });
                commands::daemon::cmd_daemon_list(&repo_root, json)
            }
            None => commands::daemon::cmd_daemon(
                &args.state,
                args.driver.as_deref(),
                args.detach,
                args.max_retries,
                args.timeout,
                args.advisory_review,
                args.review_blocking,
                args.use_auto_rollback(),
                match args.rollback_policy.as_deref() {
                    // An unrecognised policy must abort rather than silently defaulting: a run
                    // that thinks it measured on-stall and actually measured always is worse
                    // than a run that did not start.
                    Some(v) => match blyte_core::daemon::RollbackPolicy::parse(v) {
                        Ok(policy) => policy,
                        Err(err) => {
                            eprintln!("{err}");
                            std::process::exit(2);
                        }
                    },
                    None => blyte_core::daemon::RollbackPolicy::default(),
                },
                args.use_shunt(),
                args.use_guard(),
                args.read_threshold_lines,
                args.use_read_cache(),
                args.use_command_guard(),
                args.impact_slicing,
            ),
        },
        Some(Commands::Cancel(args)) => {
            commands::daemon::cmd_cancel(args.state.as_deref(), args.branch.as_deref(), args.all)
        }
        Some(Commands::Logs(args)) => {
            commands::daemon::cmd_logs(args.state.as_deref(), args.branch.as_deref(), args.tail)
                .map(|_| ())
        }
        Some(Commands::Handoff(args)) => commands::handoff::cmd_handoff(&args),
        Some(Commands::Inspect(args)) => commands::inspect::cmd_inspect(&args),
        Some(Commands::Guard { cmd }) => commands::guard::dispatch_guard(cmd),
        Some(Commands::Shunt {
            threshold_lines,
            logs_dir,
            cmd,
        }) => match commands::shunt::execute_shunt(&cmd, threshold_lines, logs_dir) {
            Ok(code) => std::process::exit(code),
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        },
        None => {
            if cli.prompt.is_some() || cli.resume.is_some() {
                if cli.resume.is_none() && cli.prompt.as_deref().unwrap_or("").trim().is_empty() {
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
            Ok(())
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
