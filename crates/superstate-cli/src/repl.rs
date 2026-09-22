use std::io::{BufRead, Write};
use std::path::PathBuf;

/// Configuration options for launching the interactive REPL harness.
#[derive(Debug, Clone)]
pub struct ReplConfig {
    pub repo_root: PathBuf,
    pub model: Option<String>,
    pub gate: Option<String>,
    pub context_strategy: String,
    pub container: Option<String>,
    pub max_turns: usize,
    pub max_gates: usize,
    pub rollback: bool,
}

impl Default for ReplConfig {
    fn default() -> Self {
        Self {
            repo_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            model: None,
            gate: None,
            context_strategy: "state-only".to_string(),
            container: None,
            max_turns: 60,
            max_gates: 5,
            rollback: true,
        }
    }
}

/// Active state and parameters of an ongoing REPL session.
pub struct ReplSession {
    pub repo_root: PathBuf,
    pub gate: Option<String>,
    pub model: String,
    pub context_strategy: String,
    pub state: superstate_agent::state::State,
    pub history: Vec<superstate_provider::Message>,
}

impl ReplSession {
    /// Creates a new `ReplSession` initialized with the given `ReplConfig`.
    pub fn new(config: &ReplConfig) -> Self {
        let gate = config.gate.clone();
        let verification_cmd = gate.clone().unwrap_or_default();
        let goal = "Interactive REPL session".to_string();
        let state = superstate_agent::state::State::new(goal, verification_cmd);
        Self {
            repo_root: config.repo_root.clone(),
            gate,
            model: config
                .model
                .clone()
                .unwrap_or_else(|| "claude-3-5-sonnet".to_string()),
            context_strategy: config.context_strategy.clone(),
            state,
            history: Vec::new(),
        }
    }
}

/// Control flow signal emitted after handling REPL input.
#[derive(Debug, PartialEq, Eq)]
pub enum ReplControl {
    Handled,
    Exit,
    SubmitPrompt(String),
}

/// Formats a runtime event emitted during agent turn or gate execution.
pub fn format_event(event: &superstate_agent::run::Event<'_>) -> String {
    match event {
        superstate_agent::run::Event::Turn { n, text } => {
            let preview = if text.len() > 80 {
                format!("{}...", &text[..80])
            } else {
                text.to_string()
            };
            format!("[Turn {}] {}", n, preview)
        }
        superstate_agent::run::Event::Tool {
            name,
            input,
            summary,
            is_error,
        } => {
            if *is_error {
                format!("\x1b[31m● {} {} -> {}\x1b[0m", name, input, summary)
            } else {
                format!("● {} {} -> {}", name, input, summary)
            }
        }
        superstate_agent::run::Event::Gate {
            attempt,
            passed,
            output,
        } => {
            if *passed {
                format!("✓ Gate passed (attempt {})", attempt)
            } else {
                format!("✗ Gate failed (attempt {}): {}", attempt, output.trim())
            }
        }
        superstate_agent::run::Event::RolledBack { to } => {
            format!("↺ Rolled back working tree to {}", to)
        }
    }
}

/// Renders the Superstate ASCII banner and environment summary to a writer.
pub fn render_banner_to<W: Write>(writer: &mut W, session: &ReplSession) -> std::io::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let repo = session.repo_root.display().to_string();
    let model = &session.model;
    let gate = session
        .gate
        .as_deref()
        .unwrap_or("None (fail-closed disabled)");

    let mut toolchains = Vec::new();
    if session.repo_root.join("Cargo.toml").exists() {
        toolchains.push("Rust (Cargo)");
    }
    if session.repo_root.join("pyproject.toml").exists()
        || session.repo_root.join("requirements.txt").exists()
    {
        toolchains.push("Python");
    }
    if session.repo_root.join("package.json").exists() {
        toolchains.push("Node.js");
    }
    if session.repo_root.join("go.mod").exists() {
        toolchains.push("Go");
    }
    let toolchains_str = if toolchains.is_empty() {
        "Standard POSIX".to_string()
    } else {
        toolchains.join(", ")
    };

    writeln!(
        writer,
        "╔═══════════════════════════════════════════════════════════╗"
    )?;
    writeln!(
        writer,
        "║   ____                                _        _          ║"
    )?;
    writeln!(
        writer,
        "║  / ___| _   _ _ __   ___ _ __ ___   __| |  ___| |_ ___    ║"
    )?;
    writeln!(
        writer,
        "║  \\___ \\| | | | '_ \\ / _ \\ '__/ __| / _` | / __| __/ _ \\   ║"
    )?;
    writeln!(
        writer,
        "║   ___) | |_| | |_) |  __/ |  \\__ \\| (_| || (__| ||  __/   ║"
    )?;
    writeln!(
        writer,
        "║  |____/ \\__,_| .__/ \\___|_|  |___/ \\__,_(_)___|\\__\\___|   ║"
    )?;
    writeln!(
        writer,
        "║              |_|                                          ║"
    )?;
    writeln!(
        writer,
        "║  Superstate Interactive Harness v{:<25}║",
        version
    )?;
    writeln!(
        writer,
        "╚═══════════════════════════════════════════════════════════╝"
    )?;
    writeln!(writer, "  Repository : {}", repo)?;
    writeln!(writer, "  Model      : {}", model)?;
    writeln!(writer, "  Gate       : {}", gate)?;
    writeln!(writer, "  Toolchains : {}", toolchains_str)?;
    writeln!(
        writer,
        "  Type /help for slash commands or enter a prompt to begin."
    )?;
    writeln!(writer)?;
    Ok(())
}

/// Renders the Superstate banner to standard output.
pub fn render_banner(session: &ReplSession) {
    let mut stdout = std::io::stdout();
    let _ = render_banner_to(&mut stdout, session);
}

/// Processes a single line of REPL input, dispatching slash commands or submitting prompts.
pub fn handle_repl_command<W: Write>(
    session: &mut ReplSession,
    line: &str,
    writer: &mut W,
) -> Result<ReplControl, String> {
    let trimmed = line.trim();

    // Check if input is a slash command
    if trimmed.starts_with('/') {
        let (cmd, args) = match trimmed.split_once(char::is_whitespace) {
            Some((c, a)) => (c, a.trim()),
            None => (trimmed, ""),
        };

        match cmd {
            "/help" => {
                writeln!(writer, "Available Slash Commands:").map_err(|e| e.to_string())?;
                writeln!(writer, "  /help         - Show this command reference")
                    .map_err(|e| e.to_string())?;
                writeln!(
                    writer,
                    "  /state        - Inspect SKILL.state (Orchestrator, Runtime, Agent)"
                )
                .map_err(|e| e.to_string())?;
                writeln!(
                    writer,
                    "  /diff         - Show current git diff in working tree"
                )
                .map_err(|e| e.to_string())?;
                writeln!(
                    writer,
                    "  /review       - Run code review engine on uncommitted changes"
                )
                .map_err(|e| e.to_string())?;
                writeln!(
                    writer,
                    "  /doctor       - Run 5-step system verification handshake"
                )
                .map_err(|e| e.to_string())?;
                writeln!(
                    writer,
                    "  /gate [cmd]   - Inspect or update the verification gate command"
                )
                .map_err(|e| e.to_string())?;
                writeln!(
                    writer,
                    "  /clear        - Clear session history and reset working state"
                )
                .map_err(|e| e.to_string())?;
                writeln!(writer, "  /exit, /quit  - Exit the interactive REPL")
                    .map_err(|e| e.to_string())?;
                Ok(ReplControl::Handled)
            }
            "/state" => {
                writeln!(writer, "=== Superstate SKILL.state ===").map_err(|e| e.to_string())?;
                let orch = serde_json::to_string_pretty(&session.state.orchestrator)
                    .map_err(|e| e.to_string())?;
                writeln!(writer, "Orchestrator:\n{}", orch).map_err(|e| e.to_string())?;

                let rt = serde_json::to_string_pretty(&session.state.runtime)
                    .map_err(|e| e.to_string())?;
                writeln!(writer, "Runtime:\n{}", rt).map_err(|e| e.to_string())?;

                let agent = serde_json::to_string_pretty(&session.state.agent)
                    .map_err(|e| e.to_string())?;
                writeln!(writer, "Agent:\n{}", agent).map_err(|e| e.to_string())?;
                Ok(ReplControl::Handled)
            }
            "/diff" => {
                let out = std::process::Command::new("git")
                    .arg("diff")
                    .current_dir(&session.repo_root)
                    .output();
                match out {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        if stdout.trim().is_empty() {
                            writeln!(writer, "No uncommitted diff in repository.")
                                .map_err(|e| e.to_string())?;
                        } else {
                            write!(writer, "{}", stdout).map_err(|e| e.to_string())?;
                        }
                    }
                    Err(e) => {
                        writeln!(writer, "Failed to run git diff: {}", e)
                            .map_err(|e| e.to_string())?;
                    }
                }
                Ok(ReplControl::Handled)
            }
            "/review" => {
                writeln!(
                    writer,
                    "Running review engine on {}...",
                    session.repo_root.display()
                )
                .map_err(|e| e.to_string())?;
                match crate::commands::review::cmd_review(crate::commands::review::ReviewArgs {
                    commit_range: None,
                    repo: session.repo_root.clone(),
                    driver: None,
                    state_path: None,
                    record: false,
                    json: false,
                    fail_on_blocking: false,
                }) {
                    Ok(()) => {}
                    Err(e) => {
                        writeln!(writer, "Review advisory: {}", e).map_err(|e| e.to_string())?;
                    }
                }
                Ok(ReplControl::Handled)
            }
            "/doctor" => {
                writeln!(writer, "Running system verification handshake...")
                    .map_err(|e| e.to_string())?;
                match crate::commands::doctor::run_doctor() {
                    Ok(()) => {}
                    Err(e) => {
                        writeln!(writer, "Doctor check failed: {}", e)
                            .map_err(|e| e.to_string())?;
                    }
                }
                Ok(ReplControl::Handled)
            }
            "/gate" => {
                if args.is_empty() {
                    match &session.gate {
                        Some(g) => {
                            writeln!(writer, "Current gate: {}", g).map_err(|e| e.to_string())?
                        }
                        None => {
                            writeln!(writer, "No gate configured.").map_err(|e| e.to_string())?
                        }
                    }
                } else {
                    session.gate = Some(args.to_string());
                    session.state.orchestrator.verification_cmd = args.to_string();
                    writeln!(writer, "Gate set to: {}", args).map_err(|e| e.to_string())?;
                }
                Ok(ReplControl::Handled)
            }
            "/clear" => {
                session.history.clear();
                session.state.runtime = superstate_agent::state::Runtime::default();
                session.state.agent = superstate_agent::state::Agent::default();
                writeln!(writer, "Session history cleared and working state reset.")
                    .map_err(|e| e.to_string())?;
                Ok(ReplControl::Handled)
            }
            "/exit" | "/quit" => Ok(ReplControl::Exit),
            unknown => {
                writeln!(
                    writer,
                    "Unknown command: {}. Type /help for available commands.",
                    unknown
                )
                .map_err(|e| e.to_string())?;
                Ok(ReplControl::Handled)
            }
        }
    } else if trimmed.is_empty() {
        Ok(ReplControl::Handled)
    } else {
        Ok(ReplControl::SubmitPrompt(line.to_string()))
    }
}

/// Runs the interactive REPL event loop over generic input and output streams.
pub fn run_repl_stream<R: BufRead, W: Write>(
    mut reader: R,
    mut writer: W,
    mut session: ReplSession,
) -> Result<(), String> {
    loop {
        write!(writer, "superstate > ").map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;

        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if bytes_read == 0 {
            // EOF reached
            break;
        }

        let trimmed = line.trim_end();
        match handle_repl_command(&mut session, trimmed, &mut writer)? {
            ReplControl::Exit => break,
            ReplControl::Handled => {}
            ReplControl::SubmitPrompt(prompt) => {
                writeln!(writer, "Dispatching turn for prompt: {}", prompt)
                    .map_err(|e| e.to_string())?;
                session
                    .history
                    .push(superstate_provider::Message::User(prompt.clone()));
                session.state.runtime.attempts += 1;
            }
        }
    }
    Ok(())
}

/// Initializes a REPL session and runs the interactive terminal loop.
pub fn run_repl(config: ReplConfig) -> Result<(), String> {
    let session = ReplSession::new(&config);
    render_banner(&session);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    run_repl_stream(stdin.lock(), stdout.lock(), session)
}
