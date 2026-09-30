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

    // Before anything else reads the environment, and before any threads start.
    if let Some(path) = blyte_cli::env_file::default_path() {
        if let Err(e) = blyte_cli::env_file::load(&path) {
            eprintln!("warning: {e}");
        }
    }

    let cli = Cli::parse();

    if let Some(cmd) = cli.command {
        match cmd {
            blyte_cli::cli::Command::Init { force, print } => {
                let out = std::process::Command::new("git")
                    .args(["rev-parse", "--show-toplevel"])
                    .output();
                let Ok(out) = out else {
                    eprintln!("error: git not found");
                    std::process::exit(2);
                };
                if !out.status.success() {
                    eprintln!("error: not a git repository");
                    std::process::exit(2);
                }
                let root = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
                let detected = blyte_cli::config::detect(&root);

                if print {
                    print!("{}", blyte_cli::config::render(&detected));
                    std::process::exit(0);
                }

                if detected.is_empty() {
                    eprintln!(
                        "no toolchain detected: write blyte.toml by hand (see blyte init --print for the format)"
                    );
                    std::process::exit(1);
                }

                let toml_path = root.join("blyte.toml");
                if toml_path.exists() && !force {
                    eprintln!("blyte.toml already exists; pass --force to overwrite");
                    std::process::exit(1);
                }

                if let Err(e) = std::fs::write(&toml_path, blyte_cli::config::render(&detected)) {
                    eprintln!("error writing {}: {e}", toml_path.display());
                    std::process::exit(1);
                }

                println!("wrote {}", toml_path.display());
                for step in &detected {
                    println!("{step}");
                }
                std::process::exit(0);
            }
            blyte_cli::cli::Command::Dispatch(args) => {
                std::process::exit(blyte_cli::dispatch::run_dispatch(&args));
            }
            blyte_cli::cli::Command::Review(args) => {
                std::process::exit(blyte_cli::dispatch::run_review(&args));
            }
            blyte_cli::cli::Command::Status { id, json } => {
                std::process::exit(blyte_cli::dispatch::run_status(&id, json));
            }
            blyte_cli::cli::Command::Update { check, rollback } => {
                std::process::exit(blyte_cli::update::run(check, rollback));
            }
            blyte_cli::cli::Command::Skill(args) => match args.action {
                blyte_cli::cli::SkillAction::Install {
                    project,
                    force,
                    default: _,
                    no_default,
                    harness,
                } => {
                    let targets = match blyte_cli::skill::Harness::parse_all(&harness) {
                        Ok(t) => t,
                        Err(e) => {
                            eprintln!("{e}");
                            std::process::exit(1);
                        }
                    };
                    for &target in &targets {
                        let written =
                            match blyte_cli::skill::install_for(target, project, force, no_default)
                            {
                                Ok(w) => w,
                                Err(e) => {
                                    eprintln!("{e}");
                                    std::process::exit(1);
                                }
                            };
                        for path in written {
                            if path.extension().is_some_and(|ext| ext == "rules") {
                                println!("wrote {}", path.display());
                            } else if path.file_name().is_some_and(|name| name == "SKILL.md") {
                                println!("installed {}", path.display());
                            } else {
                                println!("updated {}", path.display());
                            }
                        }
                    }
                    for target in targets {
                        match target {
                            blyte_cli::skill::Harness::Claude => {
                                if no_default {
                                    println!(
                                        "to use it, ask Claude Code: \"Plan this change, then use blyte to implement each task and review the results.\""
                                    );
                                } else {
                                    println!(
                                        "Claude Code now writes the plan and delegates implementation to blyte, in parallel, by default"
                                    );
                                }
                            }
                            blyte_cli::skill::Harness::Codex => {
                                if no_default {
                                    println!(
                                        "to use it, ask Codex: \"use blyte to implement this\""
                                    );
                                } else {
                                    println!(
                                        "Codex now writes the plan and delegates implementation to blyte, in parallel, by default"
                                    );
                                }
                            }
                        }
                    }
                    std::process::exit(0);
                }
                blyte_cli::cli::SkillAction::Uninstall { project, harness } => {
                    let targets = match blyte_cli::skill::Harness::parse_all(&harness) {
                        Ok(t) => t,
                        Err(e) => {
                            eprintln!("{e}");
                            std::process::exit(1);
                        }
                    };
                    let mut all_paths = Vec::new();
                    for target in targets {
                        match blyte_cli::skill::uninstall_for(target, project) {
                            Ok(paths) => all_paths.extend(paths),
                            Err(e) => {
                                eprintln!("{e}");
                                std::process::exit(1);
                            }
                        }
                    }
                    if all_paths.is_empty() {
                        println!("nothing to remove");
                    } else {
                        for path in all_paths {
                            println!("removed {}", path.display());
                        }
                    }
                    std::process::exit(0);
                }
                blyte_cli::cli::SkillAction::Print => {
                    print!("{}", blyte_cli::skill::SKILL_MD);
                    std::process::exit(0);
                }
            },
        }
    }

    blyte_agent::gate_slots::enable();

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
