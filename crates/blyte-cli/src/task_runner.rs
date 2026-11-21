//! Headless single-shot solver runner for executing direct agent prompts.
//!
//! Wires together the configured LLM provider, execution shell (local host or container sandbox),
//! filesystem abstraction, context strategy, and verification gate runner into an autonomous loop.

use std::io::Write;
use std::path::{Path, PathBuf};
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
    /// Run the verification gate once before the first model call and stop as blocked if it fails.
    pub base_check: bool,
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

        let network = match &cli.network {
            Some(v) => v.clone(),
            None if cli.orchestrate && cli.sandbox == "on" && cli.container.is_none() => {
                "off".into()
            }
            None => "on".into(),
        };

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
            sandbox: cli.sandbox.clone(),
            network,
            allow_read: cli.allow_read.clone(),
            pass_env: cli.pass_env.clone(),
            research: cli.research,
            claim_check: cli.claim_check,
            base_check: cli.base_check,
            orchestrate: cli.orchestrate,
            dispatch_cmd: cli.dispatch_cmd.clone(),
        }
    }
}

fn orchestrator_extra_writes(root: &Path) -> Vec<PathBuf> {
    let output = match std::process::Command::new("git")
        .args(["-C"])
        .arg(root)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines().map(str::trim).filter(|s| !s.is_empty());
    let (Some(git_dir), Some(common_dir)) = (lines.next(), lines.next()) else {
        return Vec::new();
    };
    let git_dir_path = PathBuf::from(git_dir);
    let common_dir_path = PathBuf::from(common_dir);
    let git_canon = git_dir_path.canonicalize().unwrap_or(git_dir_path);
    let common_canon = common_dir_path
        .canonicalize()
        .unwrap_or_else(|_| common_dir_path.clone());
    if git_canon == common_canon {
        return Vec::new();
    }
    let mut extra = vec![common_canon.clone()];
    if common_dir_path != common_canon {
        extra.push(common_dir_path.clone());
    }
    if common_dir_path.file_name() == Some(std::ffi::OsStr::new(".git")) {
        if let Some(main_root) = common_dir_path.parent() {
            let main_canon = main_root
                .canonicalize()
                .unwrap_or_else(|_| main_root.to_path_buf());
            extra.push(main_canon.join(".wt"));
            if main_canon != main_root {
                extra.push(main_root.join(".wt"));
            }
        }
    }
    extra
}

pub fn build_sandbox_profile(config: &TaskConfig) -> blyte_agent::sandbox::Profile {
    let extra = if config.orchestrate {
        orchestrator_extra_writes(&config.root)
    } else {
        Vec::new()
    };
    if extra.is_empty() {
        return blyte_agent::sandbox::default_profile_net(
            &config.root,
            &config.allow_read,
            config.network == "on",
        );
    }
    #[cfg(target_os = "macos")]
    {
        let mut allowed = blyte_agent::sandbox::default_allowed(&config.root);
        allowed.extend(extra);
        let deny_read = match std::env::var_os("HOME") {
            Some(home) if !home.is_empty() => blyte_agent::sandbox::read_denied(Path::new(&home)),
            _ => Vec::new(),
        };
        let base = blyte_agent::sandbox::profile_with(
            &config.root,
            &allowed,
            &deny_read,
            &config.allow_read,
        );
        if config.network == "on" {
            blyte_agent::sandbox::Profile::Seatbelt(base)
        } else {
            let socks = match std::env::var_os("HOME") {
                Some(home) if !home.is_empty() => {
                    blyte_agent::sandbox::docker_sockets(Path::new(&home))
                }
                _ => Vec::new(),
            };
            blyte_agent::sandbox::Profile::Seatbelt(blyte_agent::sandbox::offline(base, &socks))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut profile = blyte_agent::sandbox::default_profile_net(
            &config.root,
            &config.allow_read,
            config.network == "on",
        );
        match &mut profile {
            blyte_agent::sandbox::Profile::Landlock(write)
            | blyte_agent::sandbox::Profile::LandlockOffline(write)
            | blyte_agent::sandbox::Profile::LandlockConfined { write, .. } => {
                write.extend(extra);
            }
            blyte_agent::sandbox::Profile::Seatbelt(_) => {}
        }
        profile
    }
}

pub fn build_local_shell(config: &TaskConfig) -> blyte_agent::shell::PersistentShell {
    if config.sandbox == "on" {
        if blyte_agent::sandbox::available() {
            let profile = build_sandbox_profile(config);
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
            if let blyte_agent::sandbox::Profile::LandlockConfined { block_tcp, .. } = &profile {
                eprintln!(
                    "blyte: on Linux, commands can read only system directories, the workspace, tool caches and --allow-read paths"
                );
                if *block_tcp {
                    eprintln!(
                        "blyte: on Linux --network=off blocks TCP connections only (localhost included); UDP and the Docker socket are not blocked"
                    );
                }
            }
            blyte_agent::shell::PersistentShell::local_sandboxed(config.root.clone(), profile)
        } else {
            eprintln!("warning: sandbox unavailable; running unsandboxed");
            blyte_agent::shell::PersistentShell::local(config.root.clone())
        }
    } else {
        blyte_agent::shell::PersistentShell::local(config.root.clone())
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

/// Executes an autonomous solver run against an existing mutable `State`, passing every raw event
/// directly to `on_event` without formatting lines to a writer.
pub fn run_task_with_state_and_events(
    config: &TaskConfig,
    state: &mut blyte_agent::state::State,
    on_event: &(dyn Fn(&blyte_agent::run::Event<'_>) + Sync),
) -> Result<(blyte_agent::run::Outcome, blyte_agent::run::RunReport), String> {
    run_task_with_event_callback(config, state, &|event| on_event(event))
}

/// Executes an autonomous solver run against an existing mutable `State`, streaming events to `writer`
/// and updating the shared `progress` record.
pub fn run_task_with_state_writer_and_progress<W: Write>(
    config: &TaskConfig,
    state: &mut blyte_agent::state::State,
    writer: &mut W,
    progress: Arc<Mutex<Progress>>,
) -> Result<(blyte_agent::run::Outcome, blyte_agent::run::RunReport), String> {
    let writer_mutex = Mutex::new(writer);
    let on_event = |event: &blyte_agent::run::Event<'_>| {
        {
            let mut p = match progress.lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            match event {
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
        let line = crate::repl::format_event(event);
        if !line.is_empty() {
            if let Ok(mut w) = writer_mutex.lock() {
                let _ = writeln!(w, "{line}");
            }
        }
    };
    run_task_with_event_callback(config, state, &on_event)
}

fn build_usage_entry(
    config: &TaskConfig,
    model_id: &str,
    outcome: String,
    turns: u32,
    usage: &blyte_provider::Usage,
) -> crate::usage_log::Entry {
    crate::usage_log::Entry {
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
        model: model_id.to_string(),
        outcome,
        turns,
        input: usage.input,
        cached: usage.cached,
        output: usage.output,
        thinking: usage.thinking,
        cost_usd: blyte_provider::pricing(model_id).map(|p| p.cost(usage)),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ExceededField {
    field: &'static str,
    written: u64,
    total: u64,
}

fn usage_remainder(
    total_turns: u32,
    total_usage: &blyte_provider::Usage,
    written_turns: u32,
    written_usage: &blyte_provider::Usage,
) -> Result<(u32, blyte_provider::Usage), Vec<ExceededField>> {
    let mut exceeded = Vec::new();
    if written_turns > total_turns {
        exceeded.push(ExceededField {
            field: "turns",
            written: u64::from(written_turns),
            total: u64::from(total_turns),
        });
    }
    if written_usage.input > total_usage.input {
        exceeded.push(ExceededField {
            field: "input",
            written: written_usage.input,
            total: total_usage.input,
        });
    }
    if written_usage.cached > total_usage.cached {
        exceeded.push(ExceededField {
            field: "cached",
            written: written_usage.cached,
            total: total_usage.cached,
        });
    }
    if written_usage.output > total_usage.output {
        exceeded.push(ExceededField {
            field: "output",
            written: written_usage.output,
            total: total_usage.output,
        });
    }
    if written_usage.thinking > total_usage.thinking {
        exceeded.push(ExceededField {
            field: "thinking",
            written: written_usage.thinking,
            total: total_usage.thinking,
        });
    }
    if !exceeded.is_empty() {
        return Err(exceeded);
    }
    Ok((
        total_turns - written_turns,
        blyte_provider::Usage {
            input: total_usage.input - written_usage.input,
            cached: total_usage.cached - written_usage.cached,
            output: total_usage.output - written_usage.output,
            thinking: total_usage.thinking - written_usage.thinking,
        },
    ))
}

fn run_task_with_event_callback(
    config: &TaskConfig,
    state: &mut blyte_agent::state::State,
    on_event: &dyn Fn(&blyte_agent::run::Event<'_>),
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
        let sandbox_on = config.sandbox == "on";
        let allowed_count = if sandbox_on {
            let rules = crate::approval_file::load()?;
            let count = rules.len();
            let approver: Arc<dyn blyte_agent::approval::Approver> =
                if state.runtime.conversation.is_some() {
                    Arc::new(crate::tty_approver::TtyApprover)
                } else {
                    Arc::new(blyte_agent::approval::NoApprover)
                };
            blyte_agent::tools::set_approval(Some(blyte_agent::approval::ApprovalConfig {
                rules,
                approver,
            }));
            count
        } else {
            blyte_agent::tools::set_approval(None);
            0
        };
        if state.orchestrator.instructions.is_none() {
            let lines = crate::orchestrator_text::set_state_instructions(
                state,
                config.dispatch_cmd.is_some(),
                &config.root,
                sandbox_on,
                allowed_count,
            )?;
            if state.runtime.conversation.is_none() {
                for line in &lines {
                    eprintln!("blyte: {line}");
                }
            }
        }
        let exe = std::env::current_exe()
            .map_err(|e| format!("cannot locate blyte executable for the blyte tool: {e}"))?;
        blyte_agent::tools::set_blyte_tool(Some(exe));
    } else {
        blyte_agent::tools::set_approval(None);
        blyte_agent::tools::set_blyte_tool(None);
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
            if !config.orchestrate {
                if let Some(r) = blyte_agent::rules::load(&config.root) {
                    eprintln!("blyte: repository rules from {}", r.source.display());
                }
            }
            let withheld = blyte_agent::shell::withheld_env();
            if !withheld.is_empty() {
                eprintln!(
                    "blyte: withholding from commands: {} (keep one with --pass-env NAME)",
                    withheld.join(", ")
                );
            }
            let shell = build_local_shell(config);
            (shell, None, config.root.clone())
        }
    };
    let shell: &dyn blyte_agent::shell::Shell = &persistent;
    let fs: &dyn blyte_agent::fs::Fs = match &session_fs {
        Some(f) => f,
        None => &local_fs,
    };

    let usage_path = crate::usage_log::default_path();
    let written = Mutex::new((0u32, blyte_provider::Usage::default()));
    let runner_on_event = |event: blyte_agent::run::Event<'_>| {
        if let blyte_agent::run::Event::Turn { usage, .. } = &event {
            if let Some(path) = &usage_path {
                let entry = build_usage_entry(config, &model_id, "turn".to_string(), 1, usage);
                match crate::usage_log::append(path, &entry) {
                    Ok(()) => {
                        let mut w = match written.lock() {
                            Ok(g) => g,
                            Err(e) => e.into_inner(),
                        };
                        w.0 += 1;
                        w.1.input += usage.input;
                        w.1.cached += usage.cached;
                        w.1.output += usage.output;
                        w.1.thinking += usage.thinking;
                    }
                    Err(err) => eprintln!("warning: usage log: {err}"),
                }
            }
        }
        on_event(&event);
    };

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
        on_event: Some(&runner_on_event),
        research: config.research,
        claim_check: config.claim_check,
        base_check: config.base_check,
    };

    let (outcome, report) = runner.run(state);

    if let Some(path) = &usage_path {
        let (written_turns, written_usage) = match written.lock() {
            Ok(g) => *g,
            Err(e) => *e.into_inner(),
        };
        let (closing_turns, closing_usage) = match usage_remainder(
            report.outcome_turns,
            &report.usage,
            written_turns,
            &written_usage,
        ) {
            Ok(rem) => rem,
            Err(exceeded) => {
                for ExceededField {
                    field,
                    written,
                    total,
                } in exceeded
                {
                    eprintln!(
                        "error: usage log: turn lines exceed the run's total ({field}: {written} > {total})"
                    );
                }
                (0, blyte_provider::Usage::default())
            }
        };
        let entry = build_usage_entry(
            config,
            &model_id,
            format!("{outcome:?}").to_lowercase(),
            closing_turns,
            &closing_usage,
        );
        if let Err(err) = crate::usage_log::append(path, &entry) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_remainder_computes_exact_and_partial_remainders() {
        let total = blyte_provider::Usage {
            input: 600,
            cached: 150,
            output: 90,
            thinking: 75,
        };
        let exact = usage_remainder(3, &total, 3, &total).unwrap();
        assert_eq!(exact, (0, blyte_provider::Usage::default()));

        let written = blyte_provider::Usage {
            input: 500,
            cached: 100,
            output: 80,
            thinking: 50,
        };
        let partial = usage_remainder(3, &total, 2, &written).unwrap();
        assert_eq!(
            partial,
            (
                1,
                blyte_provider::Usage {
                    input: 100,
                    cached: 50,
                    output: 10,
                    thinking: 25,
                }
            )
        );
    }

    #[test]
    fn usage_remainder_reports_exceeding_fields() {
        let total = blyte_provider::Usage {
            input: 100,
            cached: 20,
            output: 30,
            thinking: 10,
        };
        let written = blyte_provider::Usage {
            input: 120,
            cached: 20,
            output: 35,
            thinking: 10,
        };
        let err = usage_remainder(2, &total, 3, &written).unwrap_err();
        assert_eq!(
            err,
            vec![
                ExceededField {
                    field: "turns",
                    written: 3,
                    total: 2,
                },
                ExceededField {
                    field: "input",
                    written: 120,
                    total: 100,
                },
                ExceededField {
                    field: "output",
                    written: 35,
                    total: 30,
                },
            ]
        );
    }
}
