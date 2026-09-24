//! Headless single-shot solver runner for executing direct agent prompts.
//!
//! Wires together the configured LLM provider, execution shell (local host or container sandbox),
//! filesystem abstraction, context strategy, and verification gate runner into an autonomous loop.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Execution parameters for running an autonomous solver task to completion.
#[derive(Debug, Clone)]
pub struct TaskConfig {
    /// Goal or task prompt for the agent to resolve.
    pub prompt: String,
    /// Optional command string executed to determine task success (exit 0).
    pub gate: Option<String>,
    /// Context assembly strategy ("state-only" or "conversational").
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
    /// Run the loop-breaker guards. False is the ablation control (`--no-loop-breaker`).
    pub loop_breaker: bool,
    /// Working directory inside the container (`--container-workdir`).
    pub container_workdir: String,
    /// Wall-clock budget in seconds (`--max-seconds`).
    pub max_seconds: Option<u64>,
    /// Where to write the JSON run report, if anywhere (`--report`).
    pub report: Option<PathBuf>,
    /// Gate by autonomous TDD phases instead of a single command (`--tdd`).
    pub tdd: bool,
    /// Completeness review before a passing verification is accepted (`--review`).
    pub review: bool,
    /// Fold notes (`--fold-notes`).
    pub fold_notes: bool,
    /// Dollar limit (`--max-cost-usd`).
    pub max_cost_usd: Option<f64>,
    /// Where to save the run before every model call (`--journal`).
    pub journal: Option<PathBuf>,
    /// A journal to resume from (`--resume`).
    pub resume: Option<PathBuf>,
    /// Fold budget in approximate tokens (`--fold-budget`).
    pub fold_budget: Option<usize>,
}

impl From<&crate::cli::Cli> for TaskConfig {
    /// Maps parsed CLI flags into a concrete `TaskConfig` configuration.
    fn from(cli: &crate::cli::Cli) -> Self {
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
            loop_breaker: !cli.no_loop_breaker,
            container_workdir: cli.container_workdir.clone(),
            max_seconds: cli.max_seconds,
            report: cli.report.clone(),
            tdd: cli.tdd,
            review: cli.review,
            fold_notes: cli.fold_notes,
            max_cost_usd: cli.max_cost_usd,
            journal: cli.journal.clone(),
            resume: cli.resume.clone(),
            fold_budget: cli.fold_budget,
        }
    }
}

/// Autonomous TDD's RED/GREEN milestones as harness phases. The wording, commands and
/// reproduction path come from `blyte_core::tdd` unchanged, so native and daemon runs are
/// given identical instructions. RED may change only the reproduction test; GREEN may not
/// change it at all.
pub fn tdd_phases(repo_root: &std::path::Path) -> Result<Vec<blyte_agent::run::Phase>, String> {
    let milestones =
        blyte_core::tdd::tdd_milestones(repo_root).map_err(|e| format!("autonomous TDD: {e}"))?;
    let repro = milestones
        .first()
        .and_then(|m| m.files.first())
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| "autonomous TDD: no reproduction path".to_string())?;
    Ok(milestones
        .into_iter()
        .enumerate()
        .map(|(i, m)| blyte_agent::run::Phase {
            id: m.id.unwrap_or_else(|| format!("phase-{}", i + 1)),
            instruction: m.title,
            verify_cmd: m.verify_cmd,
            scope_only: (i == 0).then(|| repro.clone()),
            protect: (i > 0).then(|| repro.clone()),
        })
        .collect())
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
    let verification_cmd = config.gate.as_deref().unwrap_or("");
    let mut state = blyte_agent::state::State::new(&config.prompt, verification_cmd);
    let mut stdout = std::io::stdout();
    let result = run_task_with_state_and_writer(&config, &mut state, &mut stdout);
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
    // A resumed run takes its goal, gate, ledger and fold notes from the journal.
    let resumed = match &config.resume {
        Some(path) => {
            let j = crate::journal::read(path)?;
            *state = j.state;
            Some(j.history)
        }
        None => None,
    };
    state.orchestrator.review = config.review;
    state.orchestrator.fold_notes = config.fold_notes;
    let started = std::time::Instant::now();
    // Derived before the provider, so an unrecognised repository fails closed without an API call.
    let phases = if config.tdd {
        Some(tdd_phases(&config.root)?)
    } else {
        None
    };
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

    // 2. Select context assembly strategy.
    // "state-only" discards prior conversational turns and keeps only structured state O(1),
    // whereas "conversational" accumulates full turn history.
    let state_only = blyte_agent::context::StateOnly;
    let state_window = blyte_agent::context::StateWindow::default();
    let folded = blyte_agent::context::Folded {
        budget_tokens: config
            .fold_budget
            .unwrap_or(blyte_agent::context::Folded::default().budget_tokens),
    };
    let compact = blyte_agent::context::Compact {
        budget_tokens: config
            .fold_budget
            .unwrap_or(blyte_agent::context::Compact::default().budget_tokens),
        ..blyte_agent::context::Compact::default()
    };
    let adaptive = blyte_agent::context::Adaptive {
        min_tokens: config
            .fold_budget
            .unwrap_or(blyte_agent::context::Adaptive::default().min_tokens),
        ..blyte_agent::context::Adaptive::default()
    };
    let conversational = blyte_agent::context::Conversational;
    let context_strategy: &dyn blyte_agent::context::ContextStrategy =
        match config.context_strategy.as_str() {
            "state-only" => &state_only,
            "state-window" => &state_window,
            "compact" => &compact,
            "adaptive" => &adaptive,
            "folded" => &folded,
            _ => &conversational,
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
        None => (
            blyte_agent::shell::PersistentShell::local(config.root.clone()),
            None,
            config.root.clone(),
        ),
    };
    let shell: &dyn blyte_agent::shell::Shell = &persistent;
    let fs: &dyn blyte_agent::fs::Fs = match &session_fs {
        Some(f) => f,
        None => &local_fs,
    };

    // 4. Wrap writer in a Mutex so the event callback satisfies Fn trait bounds.
    let writer_mutex = Mutex::new(writer);
    let on_event = |event: blyte_agent::run::Event<'_>| {
        if let blyte_agent::run::Event::Checkpoint { history, state } = &event {
            if let Some(path) = &config.journal {
                if let Err(e) = crate::journal::write(path, state, history) {
                    eprintln!("{e}");
                }
            }
            return;
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
            max_cost_usd: config.max_cost_usd,
        },
        rollback: config.rollback,
        loop_breaker: config.loop_breaker,
        on_event: Some(&on_event),
    };

    let (outcome, report, phase_results) = match &phases {
        Some(p) => runner.run_phases(state, p),
        None => {
            let (o, r) = match resumed {
                Some(history) => runner.resume(state, history),
                None => runner.run(state),
            };
            (o, r, Vec::new())
        }
    };
    if let Some(path) = &config.report {
        let model_id = blyte_provider::Provider::model_id(provider.as_ref());
        let record = crate::report::RunRecord::from_run(
            config,
            &model_id,
            &outcome,
            &report,
            state,
            started.elapsed(),
        )
        .with_phases(&phase_results);
        crate::report::write(path, &record)?;
    }
    Ok((outcome, report))
}
