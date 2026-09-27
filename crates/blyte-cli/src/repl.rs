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
            context_strategy: "folded".to_string(),
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
    pub cached_models: Vec<blyte_provider::ModelInfo>,
    pub context_strategy: String,
    pub container: Option<String>,
    pub max_turns: usize,
    pub max_gates: usize,
    pub rollback: bool,
    pub state: blyte_agent::state::State,
}

impl ReplSession {
    /// Creates a new `ReplSession` initialized with the given `ReplConfig`.
    pub fn new(config: &ReplConfig) -> Self {
        let gate = config.gate.clone();
        let verification_cmd = gate.clone().unwrap_or_default();
        let goal = "Interactive REPL session".to_string();
        let state = blyte_agent::state::State::new(goal, verification_cmd);
        Self {
            repo_root: config.repo_root.clone(),
            gate,
            model: config
                .model
                .clone()
                .unwrap_or_else(|| "gemini-3.8-flash".to_string()),
            cached_models: Vec::new(),
            context_strategy: config.context_strategy.clone(),
            container: config.container.clone(),
            max_turns: config.max_turns,
            max_gates: config.max_gates,
            rollback: config.rollback,
            state,
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
pub fn format_event(event: &blyte_agent::run::Event<'_>) -> String {
    match event {
        blyte_agent::run::Event::Turn { n, text } => {
            let preview = if text.chars().count() > 80 {
                let s: String = text.chars().take(80).collect();
                format!("{s}...")
            } else {
                text.to_string()
            };
            format!("[Turn {}] {}", n, preview)
        }
        blyte_agent::run::Event::Tool {
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
        blyte_agent::run::Event::Gate {
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
        blyte_agent::run::Event::RolledBack { to } => {
            format!("↺ Rolled back working tree to {}", to)
        }
    }
}

/// Renders the Blyte ASCII banner and environment summary to a writer.
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
    writeln!(writer, "║  Blyte Interactive Harness v{:<25}║", version)?;
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

/// Renders the Blyte banner to standard output.
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
                    "  /model [name|idx] - Discover and switch active AI model"
                )
                .map_err(|e| e.to_string())?;
                writeln!(
                    writer,
                    "  /state        - Inspect SKILL.state (Orchestrator, Runtime)"
                )
                .map_err(|e| e.to_string())?;
                writeln!(
                    writer,
                    "  /diff         - Show current git diff in working tree"
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
            "/model" => {
                let args = args.trim();
                if args.is_empty() {
                    // Discover and display all models supported by configured providers
                    writeln!(writer, "Discovering available cloud models...")
                        .map_err(|e| e.to_string())?;
                    session.cached_models.clear();
                    let discovered = blyte_provider::discover_all_models();
                    for (provider_name, res) in discovered {
                        match res {
                            Ok(models) => {
                                let via_info = match provider_name.as_str() {
                                    "Google Gemini" => " (via GEMINI_API_KEY)",
                                    "Anthropic Claude" => " (via ANTHROPIC_API_KEY)",
                                    "Ollama Cloud" => "",
                                    _ => "",
                                };
                                let header = if provider_name == "Ollama Cloud" {
                                    let host = std::env::var("OLLAMA_HOST")
                                        .unwrap_or_else(|_| "https://ollama.com/api".to_string());
                                    format!("[Ollama Cloud] (via OLLAMA_API_KEY at {})", host)
                                } else {
                                    format!("[{}]{}", provider_name, via_info)
                                };
                                writeln!(writer, "\n{}", header).map_err(|e| e.to_string())?;

                                for m in models {
                                    session.cached_models.push(m.clone());
                                    let idx = session.cached_models.len();
                                    let is_active = session.model == m.id
                                        || session.model.ends_with(&format!("/{}", m.id));
                                    let active_tag = if is_active { " (active)" } else { "" };
                                    let id_with_active = format!("{}{}", m.id, active_tag);

                                    if let Some(ref desc) = m.description {
                                        if id_with_active.len() < 25 {
                                            writeln!(
                                                writer,
                                                "  [{}] {:<25} - {}",
                                                idx, id_with_active, desc
                                            )
                                            .map_err(|e| e.to_string())?;
                                        } else {
                                            writeln!(
                                                writer,
                                                "  [{}] {} - {}",
                                                idx, id_with_active, desc
                                            )
                                            .map_err(|e| e.to_string())?;
                                        }
                                    } else {
                                        writeln!(writer, "  [{}] {}", idx, id_with_active)
                                            .map_err(|e| e.to_string())?;
                                    }
                                }
                            }
                            Err(blyte_provider::ProviderError::MissingKey(key)) => {
                                writeln!(
                                    writer,
                                    "\n[{}] Not configured (set {} to discover models)",
                                    provider_name, key
                                )
                                .map_err(|e| e.to_string())?;
                            }
                            Err(err) => {
                                writeln!(writer, "\n[{}] Discovery error: {}", provider_name, err)
                                    .map_err(|e| e.to_string())?;
                            }
                        }
                    }
                    writeln!(writer, "\nActive model: {}", session.model)
                        .map_err(|e| e.to_string())?;
                    writeln!(
                        writer,
                        "To switch: /model <number> or /model <name> (e.g. /model 1 or /model claude-3-7-sonnet-latest)"
                    )
                    .map_err(|e| e.to_string())?;
                } else if let Ok(idx) = args.parse::<usize>() {
                    // Populate model cache on-demand if user immediately specifies numeric index
                    if session.cached_models.is_empty() {
                        let discovered = blyte_provider::discover_all_models();
                        for (_, res) in discovered {
                            if let Ok(models) = res {
                                session.cached_models.extend(models);
                            }
                        }
                    }
                    if idx >= 1 && idx <= session.cached_models.len() {
                        let chosen = &session.cached_models[idx - 1];
                        session.model = chosen.id.clone();
                        writeln!(
                            writer,
                            "Switched active model to: {} ({})",
                            chosen.id, chosen.provider
                        )
                        .map_err(|e| e.to_string())?;
                    } else {
                        writeln!(
                            writer,
                            "Invalid model index: {}. Use /model to view available models.",
                            idx
                        )
                        .map_err(|e| e.to_string())?;
                    }
                } else if let Ok(idx) = args.parse::<i64>() {
                    writeln!(
                        writer,
                        "Invalid model index: {}. Use /model to view available models.",
                        idx
                    )
                    .map_err(|e| e.to_string())?;
                } else {
                    // Switch model by explicit identifier or provider-namespaced tag
                    let kind = blyte_provider::registry::detect_provider_kind(Some(args));
                    session.model = args.to_string();
                    writeln!(writer, "Switched active model to: {} ({:?})", args, kind)
                        .map_err(|e| e.to_string())?;
                }
                Ok(ReplControl::Handled)
            }
            "/state" => {
                writeln!(writer, "=== Blyte SKILL.state ===").map_err(|e| e.to_string())?;
                // Render orchestrator contract: goal specification and verification gate
                let orch = serde_json::to_string_pretty(&session.state.orchestrator)
                    .map_err(|e| e.to_string())?;
                writeln!(writer, "Orchestrator:\n{}", orch).map_err(|e| e.to_string())?;

                // Render runtime state including attempt counts, rollback metrics, and action ledger
                let rt = serde_json::to_string_pretty(&session.state.runtime)
                    .map_err(|e| e.to_string())?;
                writeln!(writer, "Runtime:\n{}", rt).map_err(|e| e.to_string())?;

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
                session.state.runtime = blyte_agent::state::Runtime::default();
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
        write!(writer, "blyte > ").map_err(|e| e.to_string())?;
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
                session.state.orchestrator.goal = prompt.clone();
                if let Some(ref g) = session.gate {
                    session.state.orchestrator.verification_cmd = g.clone();
                }

                // Build task configuration from current session parameters
                let task_config = crate::task_runner::TaskConfig {
                    prompt: prompt.clone(),
                    gate: session.gate.clone(),
                    context_strategy: session.context_strategy.clone(),
                    model: Some(session.model.clone()),
                    container: session.container.clone(),
                    max_turns: session.max_turns,
                    max_gates: session.max_gates,
                    rollback: session.rollback,
                    root: session.repo_root.clone(),
                    container_workdir: "/workspace".into(),
                    max_seconds: None,
                    report: None,
                    fold_budget: None,
                    sandbox: "on".into(),
                };

                // Invoke solver task runner preserving state and outputting directly to REPL writer
                match crate::task_runner::run_task_with_state_and_writer(
                    &task_config,
                    &mut session.state,
                    &mut writer,
                ) {
                    Ok((blyte_agent::run::Outcome::Verified, report)) => {
                        writeln!(
                            writer,
                            "Task verified in {} turns (usage: {:?})",
                            report.outcome_turns, report.usage
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    Ok((outcome, report)) => {
                        writeln!(
                            writer,
                            "Task finished with outcome: {:?} in {} turns",
                            outcome, report.outcome_turns
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    Err(err) => {
                        writeln!(writer, "Task execution error: {}", err)
                            .map_err(|e| e.to_string())?;
                    }
                }
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
