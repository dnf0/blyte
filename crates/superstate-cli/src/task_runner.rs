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
        }
    }
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
) -> Result<
    (
        superstate_agent::run::Outcome,
        superstate_agent::run::RunReport,
    ),
    String,
> {
    let verification_cmd = config.gate.as_deref().unwrap_or("");
    let mut state = superstate_agent::state::State::new(&config.prompt, verification_cmd);
    let mut stdout = std::io::stdout();
    run_task_with_state_and_writer(&config, &mut state, &mut stdout)
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
    state: &mut superstate_agent::state::State,
    writer: &mut W,
) -> Result<
    (
        superstate_agent::run::Outcome,
        superstate_agent::run::RunReport,
    ),
    String,
> {
    // 1. Initialize LLM provider from environment credentials or model specification.
    // If credentials for the detected provider are missing, produce a clear error indicating which key to set.
    let provider = match superstate_provider::resolve_provider(config.model.as_deref()) {
        Ok(p) => p,
        Err(superstate_provider::ProviderError::MissingKey(key)) => {
            return Err(format!(
                "Provider error: no API key: set {key} in environment"
            ));
        }
        Err(e) => return Err(format!("Provider error: {e}")),
    };

    // 2. Select context assembly strategy.
    // "state-only" discards prior conversational turns and keeps only structured state O(1),
    // whereas "conversational" accumulates full turn history.
    let state_only = superstate_agent::context::StateOnly;
    let conversational = superstate_agent::context::Conversational;
    let context_strategy: &dyn superstate_agent::context::ContextStrategy =
        if config.context_strategy == "state-only" {
            &state_only
        } else {
            &conversational
        };

    // 3. Select execution shell and filesystem target.
    // If a container ID is specified, execution and file operations land inside Docker
    // at `/workspace` so host environment is unaffected.
    let local_shell = superstate_agent::shell::LocalShell;
    let local_fs = superstate_agent::fs::LocalFs;
    let container_shell =
        config
            .container
            .as_ref()
            .map(|cid| superstate_agent::shell::ContainerShell {
                container_id: cid.clone(),
                workdir: PathBuf::from("/workspace"),
            });
    let container_fs = config
        .container
        .as_ref()
        .map(|cid| superstate_agent::fs::ContainerFs {
            container_id: cid.clone(),
        });

    let (shell, fs, root): (
        &dyn superstate_agent::shell::Shell,
        &dyn superstate_agent::fs::Fs,
        PathBuf,
    ) = match (&container_shell, &container_fs) {
        (Some(cs), Some(cf)) => (cs, cf, PathBuf::from("/workspace")),
        _ => (&local_shell, &local_fs, config.root.clone()),
    };

    // 4. Wrap writer in a Mutex so the event callback satisfies Fn trait bounds.
    let writer_mutex = Mutex::new(writer);
    let on_event = |event: superstate_agent::run::Event<'_>| {
        if let Ok(mut w) = writer_mutex.lock() {
            let _ = writeln!(w, "{}", crate::repl::format_event(&event));
        }
    };

    // 5. Assemble the autonomous runner and execute until terminal outcome or budget exhaustion.
    let runner = superstate_agent::run::Runner {
        provider: provider.as_ref(),
        shell,
        fs,
        context: context_strategy,
        root,
        budget: superstate_agent::run::Budget {
            max_turns: config.max_turns as u32,
            max_gate_attempts: config.max_gates as u32,
        },
        rollback: config.rollback,
        on_event: Some(&on_event),
    };

    let (outcome, report) = runner.run(state);
    Ok((outcome, report))
}
