//! Headless single-shot solver runner for executing direct agent prompts.
//!
//! Wires together the configured LLM provider, execution shell (local host or container sandbox),
//! filesystem abstraction, context strategy, and verification gate runner into an autonomous loop.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Fold budget for research runs, which quote code read many turns before they write it up.
/// High enough that folding is only a safety net: unfolded research runs were cheaper, shorter and
/// invented fewer names than runs folded at 96k (`docs/specs/2026-10-19-claim-check.md`, FX).
pub const RESEARCH_FOLD_BUDGET: usize = 180_000;

/// Tracks turn and gate progress across the run, for the deadline watchdog.
#[derive(Debug, Default, Clone, Copy)]
pub struct Progress {
    pub turns: u32,
    pub gate_attempts: u32,
    pub in_gate: bool,
    pub gate_wait_ms: u64,
    pub usage: blyte_provider::Usage,
}

/// Execution parameters for running an autonomous solver task to completion.
#[derive(Debug, Clone)]
pub struct TaskConfig {
    /// Goal or task prompt for the agent to resolve.
    pub prompt: String,
    /// Optional command string executed to determine task success (exit 0).
    pub gate: Option<String>,
    pub context_strategy: String,
    /// Model identifier override passed to the provider.
    pub model: Option<String>,
    /// Container identifier if execution is sandboxed inside a Docker container.
    pub container: Option<String>,
    /// Maximum allowed turn iterations before exhausting budget.
    pub max_turns: usize,
    /// Maximum verification gate retry attempts.
    pub max_gates: usize,
    /// Whether to roll back working tree on gate failure.
    pub rollback: bool,
    /// Target repository or workspace root on the local filesystem.
    pub root: PathBuf,
    /// Working directory inside the container (`--container-workdir`).
    pub container_workdir: String,
    /// Wall-clock budget in seconds (`--max-seconds`).
    pub max_seconds: Option<u64>,
    /// Maximum spend in USD (`--max-cost`).
    pub max_cost_usd: Option<f64>,
    /// Where to write the JSON run report, if anywhere (`--report`).
    pub report: Option<PathBuf>,
    /// Fold budget in approximate tokens (`--fold-budget`).
    pub fold_budget: Option<usize>,
    /// Sandbox local command execution mode (`--sandbox`).
    pub sandbox: String,
    /// Network outbound mode for commands (`--network`).
    pub network: String,
    /// Paths permitted for reading despite credential deny-list (`--allow-read`).
    pub allow_read: Vec<PathBuf>,
    /// Environment variables permitted to pass to commands despite looking like credentials (`--pass-env`).
    pub pass_env: Vec<String>,
    /// Whether this is a research run: periodically remind the model to keep report.md updated.
    pub research: bool,
    /// Research run: check report.md's claims against their cited lines.
    pub claim_check: bool,
    /// Whether this run is in orchestrate mode (`--orchestrate`).
    pub orchestrate: bool,
    /// Command to run on the host when the orchestrator delegates work via the `dispatch` tool (`--dispatch-cmd`).
    pub dispatch_cmd: Option<String>,
}

impl From<&crate::cli::Cli> for TaskConfig {
    /// Maps parsed CLI flags into a concrete `TaskConfig` configuration.
    fn from(cli: &crate::cli::Cli) -> Self {
        blyte_agent::tools::set_read_roots(cli.read_roots.clone());
        // Fall back to current working directory if not running in container mode
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Self {
            prompt: cli.prompt.clone().unwrap_or_default(),
            gate: cli.gate.clone(),
            context_strategy: cli.context.clone(),
            model: cli.model.clone(),
            container: cli.container.clone(),
            max_turns: cli.max_turns,
            max_gates: cli.max_gates,
            rollback: cli.rollback,
            root,
            container_workdir: cli.container_workdir.clone(),
            max_seconds: cli.max_seconds,
            max_cost_usd: cli.max_cost_usd,
            report: cli.report.clone(),
            fold_budget: cli.fold_budget,
            sandbox: if cli.orchestrate {
                "off".into()
            } else {
                cli.sandbox.clone()
            },
            network: cli.network.clone(),
            allow_read: cli.allow_read.clone(),
            pass_env: cli.pass_env.clone(),
            research: cli.research,
            claim_check: cli.claim_check,
            orchestrate: cli.orchestrate,
            dispatch_cmd: cli.dispatch_cmd.clone(),
        }
    }
}

/// Validates network configuration against the environment and active flags.
///
/// Refuses `--network=off` when running with `--container`, when `--sandbox=off`,
/// when the sandbox is unavailable, or on Linux when `landlock_abi() < 4`.
pub fn check_network_mode(config: &TaskConfig) -> Result<(), String> {
    if config.network == "off" {
        if config.container.is_some() {
            return Err(
                "--network=off is not supported with --container; set the container's network instead"
                    .into(),
            );
        }
        if config.sandbox != "on" {
            return Err("--network=off needs the sandbox".into());
        }
        #[cfg(target_os = "linux")]
        if blyte_agent::sandbox::landlock_abi() < 4 {
            return Err("--network=off on Linux needs Linux 6.7 or later".into());
        }
        if !blyte_agent::sandbox::available() {
            return Err("--network=off needs the sandbox, but the sandbox is unavailable".into());
        }
    }
    Ok(())
}

/// Executes an autonomous solver run using the provided task configuration and live stdout.
///
/// Configures the AI provider, context strategy, execution environment (local or container),
/// and deterministic gate loop, printing live event progress as turns execute.
///
/// # Arguments
///
/// * `config` - Parameters governing prompt, budgets, gate verification, and environment.
///
/// # Returns
///
/// Returns `Ok((Outcome, RunReport))` containing final completion status and metrics,
/// or an `Err(String)` describing initialization failure.
pub fn run_task(
    config: TaskConfig,
) -> Result<(blyte_agent::run::Outcome, blyte_agent::run::RunReport), String> {
    let started = std::time::Instant::now();
    let progress = Arc::new(Mutex::new(Progress::default()));
    if let Some(s) = config.max_seconds {
        // BLYTE_WATCHDOG_GRACE_SECS exists for tests.
        let grace_secs: u64 = std::env::var("BLYTE_WATCHDOG_GRACE_SECS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(30);
        let watchdog_config = config.clone();
        let watchdog_progress = Arc::clone(&progress);
        std::thread::spawn(move || {
            loop {
                let wait_ms = match watchdog_progress.lock() {
                    Ok(g) => g.gate_wait_ms,
                    Err(e) => e.into_inner().gate_wait_ms,
                };
                let deadline = started
                    + std::time::Duration::from_secs(s.saturating_add(grace_secs))
                    + std::time::Duration::from_millis(wait_ms);
                match deadline.checked_duration_since(std::time::Instant::now()) {
                    Some(remaining) if !remaining.is_zero() => {
                        std::thread::sleep(remaining.min(std::time::Duration::from_millis(500)));
                    }
                    _ => break,
                }
            }
            let p = match watchdog_progress.lock() {
                Ok(g) => *g,
                Err(e) => *e.into_inner(),
            };
            let msg = if p.in_gate {
                format!(
                    "stopped {grace_secs} s past --max-seconds {s}: gate attempt {} was still running",
                    p.gate_attempts
                )
            } else {
                format!(
                    "stopped {grace_secs} s past --max-seconds {s} during turn {}: a model call or command never returned",
                    p.turns
                )
            };
            if let Some(path) = &watchdog_config.report {
                let mut record =
                    crate::report::RunRecord::from_error(&watchdog_config, &msg, started.elapsed());
                record.turns = p.turns;
                record.gate_attempts = p.gate_attempts;
                record.usage = crate::report::UsageRecord {
                    input: p.usage.input,
                    cached: p.usage.cached,
                    output: p.usage.output,
                    thinking: p.usage.thinking,
                };
                let model_id = blyte_provider::resolve_provider(watchdog_config.model.as_deref())
                    .map(|pr| pr.model_id())
                    .ok()
                    .or_else(|| watchdog_config.model.clone())
                    .unwrap_or_default();
                record.cost_usd = blyte_provider::pricing(&model_id).map(|pr| pr.cost(&p.usage));
                let _ = crate::report::write(path, &record);
            }
            eprintln!("blyte: {msg}");
            std::process::exit(1);
        });
    }
    let verification_cmd = config.gate.as_deref().unwrap_or("");
    let mut state = blyte_agent::state::State::new(&config.prompt, verification_cmd);
    state.orchestrator.max_cost_usd = config.max_cost_usd;
    if config.orchestrate {
        let rules_path = blyte_agent::orchestrator::user_rules_path();
        let loaded_rules = rules_path
            .as_deref()
            .and_then(|p| blyte_agent::orchestrator::load_user_rules(p).map(|text| (p, text)));
        let rules_ref = loaded_rules.as_ref().map(|(p, t)| (*p, t.as_str()));
        state.orchestrator.instructions =
            Some(blyte_agent::orchestrator::prompt_sections_with_dispatch(
                crate::skill::SKILL_MD,
                rules_ref,
                config.dispatch_cmd.is_some(),
            ));
    }
    let mut stdout = std::io::stdout();
    let result =
        run_task_with_state_writer_and_progress(&config, &mut state, &mut stdout, progress);
    // The loop writes its own report; this covers a run that never reached it (e.g. no API
    // key), so a caller always finds one. An absent report would read as a crash.
    if let (Err(e), Some(path)) = (&result, &config.report) {
        crate::report::write(
            path,
            &crate::report::RunRecord::from_error(&config, e, started.elapsed()),
        )?;
    }
    result
}

/// Executes an autonomous solver run against an existing mutable `State`, streaming events to `writer`.
///
/// Preserves session mutations (epistemic ledger, patches, attempts) directly in the provided `state`.
///
/// # Arguments
///
/// * `config` - Execution parameters.
/// * `state` - The mutable session state to advance.
/// * `writer` - Stream writer sink for formatted events.
///
/// # Returns
///
/// Returns `Ok((Outcome, RunReport))` or an `Err(String)` describing failure.
pub fn run_task_with_state_and_writer<W: Write>(
    config: &TaskConfig,
    state: &mut blyte_agent::state::State,
    writer: &mut W,
) -> Result<(blyte_agent::run::Outcome, blyte_agent::run::RunReport), String> {
    let progress = Arc::new(Mutex::new(Progress::default()));
    run_task_with_state_writer_and_progress(config, state, writer, progress)
}

/// Executes an autonomous solver run against an existing mutable `State`, streaming events to `writer`
/// and updating the shared `progress` record.
pub fn run_task_with_state_writer_and_progress<W: Write>(
    config: &TaskConfig,
    state: &mut blyte_agent::state::State,
    writer: &mut W,
    progress: Arc<Mutex<Progress>>,
) -> Result<(blyte_agent::run::Outcome, blyte_agent::run::RunReport), String> {
    check_network_mode(config)?;
    let started = std::time::Instant::now();
    if config.orchestrate {
        if std::env::var("BLYTE_SESSION")
            .map(|v| v.is_empty())
            .unwrap_or(true)
        {
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let id = crate::repl::new_session_id(now_secs, std::process::id());
            std::env::set_var("BLYTE_SESSION", &id);
        }
        if state.orchestrator.instructions.is_none() {
            let rules_path = blyte_agent::orchestrator::user_rules_path();
            let loaded_rules = rules_path
                .as_deref()
                .and_then(|p| blyte_agent::orchestrator::load_user_rules(p).map(|text| (p, text)));
            let rules_ref = loaded_rules.as_ref().map(|(p, t)| (*p, t.as_str()));
            state.orchestrator.instructions =
                Some(blyte_agent::orchestrator::prompt_sections_with_dispatch(
                    crate::skill::SKILL_MD,
                    rules_ref,
                    config.dispatch_cmd.is_some(),
                ));
        }
    }
    blyte_agent::tools::set_dispatch_cmd(config.dispatch_cmd.clone());
    // 1. Initialize LLM provider from environment credentials or model specification.
    // If credentials for the detected provider are missing, produce a clear error indicating which key to set.
    let provider = match blyte_provider::resolve_provider(config.model.as_deref()) {
        Ok(p) => p,
        Err(blyte_provider::ProviderError::MissingKey(key)) => {
            return Err(format!(
                "Provider error: no API key: set {key} in environment"
            ));
        }
        Err(e) => return Err(format!("Provider error: {e}")),
    };

    let model_id = provider.model_id();
    if config.max_cost_usd.is_some() && blyte_provider::pricing(&model_id).is_none() {
        return Err(format!(
            "--max-cost needs a known price for {model_id}; none is known"
        ));
    }
    state.orchestrator.max_cost_usd = config.max_cost_usd;

    // 2. Select context assembly strategy.
    let folded = blyte_agent::context::Folded {
        // Research quotes code read many turns earlier, so it keeps more before folding.
        budget_tokens: config.fold_budget.unwrap_or(if config.research {
            RESEARCH_FOLD_BUDGET
        } else {
            blyte_agent::context::Folded::default().budget_tokens
        }),
    };
    let conversational = blyte_agent::context::Conversational;
    let context_strategy: &dyn blyte_agent::context::ContextStrategy =
        match config.context_strategy.as_str() {
            "conversational" => &conversational,
            _ => &folded,
        };

    // Persistent sessions: one for the harness's commands, one for the model's `bash` (its `cd`
    // and `export` carry over). Container files go through a session too. Each falls back to a
    // process per command if a session cannot start.
    let local_fs = blyte_agent::fs::LocalFs;
    let (persistent, session_fs, root) = match &config.container {
        Some(cid) => (
            blyte_agent::shell::PersistentShell::container(
                cid.clone(),
                PathBuf::from(&config.container_workdir),
            ),
            Some(blyte_agent::fs::SessionFs::container(
                cid.clone(),
                PathBuf::from(&config.container_workdir),
            )),
            PathBuf::from(&config.container_workdir),
        ),
        None => {
            blyte_agent::shell::set_pass_env(config.pass_env.clone());
            if let Some(r) = blyte_agent::rules::load(&config.root) {
                eprintln!("blyte: repository rules from {}", r.source.display());
            }
            let withheld = blyte_agent::shell::withheld_env();
            if !withheld.is_empty() {
                eprintln!(
                    "blyte: withholding from commands: {} (keep one with --pass-env NAME)",
                    withheld.join(", ")
                );
            }
            let shell = if config.sandbox == "on" {
                if blyte_agent::sandbox::available() {
                    let profile = blyte_agent::sandbox::default_profile_net(
                        &config.root,
                        &config.allow_read,
                        config.network == "on",
                    );
                    if matches!(profile, blyte_agent::sandbox::Profile::Landlock(_)) {
                        eprintln!(
                            "blyte: on Linux the sandbox restricts writes only; credential reads are not blocked"
                        );
                    }
                    if matches!(profile, blyte_agent::sandbox::Profile::LandlockOffline(_)) {
                        eprintln!(
                            "blyte: on Linux --network=off blocks TCP connections only (localhost included); UDP and the Docker socket are not blocked"
                        );
                    }
                    if let blyte_agent::sandbox::Profile::LandlockConfined { block_tcp, .. } =
                        &profile
                    {
                        eprintln!(
                            "blyte: on Linux, commands can read only system directories, the workspace, tool caches and --allow-read paths"
                        );
                        if *block_tcp {
                            eprintln!(
                                "blyte: on Linux --network=off blocks TCP connections only (localhost included); UDP and the Docker socket are not blocked"
                            );
                        }
                    }
                    blyte_agent::shell::PersistentShell::local_sandboxed(
                        config.root.clone(),
                        profile,
                    )
                } else {
                    eprintln!("warning: sandbox unavailable; running unsandboxed");
                    blyte_agent::shell::PersistentShell::local(config.root.clone())
                }
            } else {
                blyte_agent::shell::PersistentShell::local(config.root.clone())
            };
            (shell, None, config.root.clone())
        }
    };
    let shell: &dyn blyte_agent::shell::Shell = &persistent;
    let fs: &dyn blyte_agent::fs::Fs = match &session_fs {
        Some(f) => f,
        None => &local_fs,
    };

    // 4. Wrap writer in a Mutex so the event callback satisfies Fn trait bounds.
    let writer_mutex = Mutex::new(writer);
    let on_event = |event: blyte_agent::run::Event<'_>| {
        {
            let mut p = match progress.lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            match &event {
                blyte_agent::run::Event::Turn { n, usage, .. } => {
                    p.turns = *n;
                    p.usage.input += usage.input;
                    p.usage.cached += usage.cached;
                    p.usage.output += usage.output;
                    p.usage.thinking += usage.thinking;
                    p.in_gate = false;
                }
                blyte_agent::run::Event::GateStart { attempt } => {
                    p.gate_attempts = *attempt;
                    p.in_gate = true;
                }
                blyte_agent::run::Event::GateSlotWait { ms } => {
                    p.gate_wait_ms += ms;
                }
                blyte_agent::run::Event::Gate { .. } => {
                    p.in_gate = false;
                }
                _ => {}
            }
        }
        if let Ok(mut w) = writer_mutex.lock() {
            let _ = writeln!(w, "{}", crate::repl::format_event(&event));
        }
    };

    // 5. Assemble the autonomous runner and execute until terminal outcome or budget exhaustion.
    let runner = blyte_agent::run::Runner {
        provider: provider.as_ref(),
        shell,
        fs,
        context: context_strategy,
        root,
        budget: blyte_agent::run::Budget {
            max_turns: config.max_turns as u32,
            max_gate_attempts: config.max_gates as u32,
            max_duration: config.max_seconds.map(std::time::Duration::from_secs),
        },
        rollback: config.rollback,
        on_event: Some(&on_event),
        research: config.research,
        claim_check: config.claim_check,
    };

    let (outcome, report) = runner.run(state);
    let model_id = provider.model_id();

    let entry = crate::usage_log::Entry {
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        repo: crate::usage_log::repo_of(&config.root),
        session: crate::usage_log::session_from(
            std::env::var("BLYTE_SESSION").ok(),
            std::env::var("CLAUDE_CODE_SESSION_ID").ok(),
        ),
        dispatch: std::env::var("BLYTE_DISPATCH_ID")
            .ok()
            .filter(|s| !s.is_empty()),
        research: config.research,
        model: model_id.clone(),
        outcome: format!("{outcome:?}").to_lowercase(),
        turns: report.outcome_turns,
        input: report.usage.input,
        cached: report.usage.cached,
        output: report.usage.output,
        thinking: report.usage.thinking,
        cost_usd: blyte_provider::pricing(&model_id).map(|p| p.cost(&report.usage)),
    };
    if let Some(path) = crate::usage_log::default_path() {
        if let Err(err) = crate::usage_log::append(&path, &entry) {
            eprintln!("warning: usage log: {err}");
        }
    }

    if let Some(path) = &config.report {
        let mut record = crate::report::RunRecord::from_run(
            config,
            &model_id,
            &outcome,
            &report,
            state,
            started.elapsed(),
        );
        record.effort = provider.effort();
        crate::report::write(path, &record)?;
    }
    Ok((outcome, report))
}
