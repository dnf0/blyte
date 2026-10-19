//! The headless solver's machine-readable run report (`--report <path>`).
//!
//! This is the contract between the harness and anything that measures it: blyte-eval
//! reads exactly this shape. Every field is always written, because a zero is a measurement and
//! an absent key is a bug. Schema: docs/superpowers/specs/2026-09-23-native-harness-eval-surface-design.md

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use blyte_agent::run::{Outcome, RunReport};
use blyte_agent::sandbox::Profile;
use blyte_agent::state::State;
use serde::Serialize;

use crate::task_runner::TaskConfig;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct UsageRecord {
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub thinking: u64,
}

/// The report's name for an outcome, and its error message if it carries one.
fn outcome_name(outcome: &Outcome) -> (&'static str, Option<String>) {
    match outcome {
        Outcome::Verified => ("verified", None),
        Outcome::Finished => ("finished", None),
        Outcome::BudgetExhausted => ("budget_exhausted", None),
        Outcome::GateFailed => ("gate_failed", None),
        Outcome::Blocked(why) => ("blocked", Some(why.clone())),
        Outcome::Error(e) => ("error", Some(e.clone())),
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct RunRecord {
    pub schema_version: u32,
    /// The commit this blyte was built from (`build.rs`), or `unknown`.
    pub harness_build: String,
    pub outcome: String,
    pub error: Option<String>,
    pub final_message: Option<String>,
    pub model_requested: Option<String>,
    pub model_id: Option<String>,
    pub effort: Option<String>,
    pub context: String,
    pub gated: bool,
    /// Where the gate came from: `none` (ungated) or `user` (`--gate`).
    pub gate_source: String,
    pub container: Option<String>,
    pub container_workdir: String,
    /// Whether local shell commands ran under macOS seatbelt sandbox isolation.
    pub sandboxed: bool,
    /// The sandbox mechanism active for the run (`container`, `seatbelt`, `landlock`, `landlock-offline`, `off`).
    pub sandbox: String,
    /// Outbound network mode for commands (`on` or `off`).
    pub network: String,
    /// Filesystem paths permitted for reading despite credential deny-lists.
    pub allow_read: Vec<String>,
    /// Environment variables permitted to pass to commands despite looking like credentials.
    pub pass_env: Vec<String>,
    /// Names of environment variables withheld from commands by the credential filter.
    pub withheld_env: Vec<String>,
    pub duration_seconds: f64,
    pub turns: u32,
    pub empty_turns: u32,
    #[serde(default)]
    pub truncated_turns: u32,
    #[serde(default)]
    pub read_loop: bool,
    #[serde(default)]
    pub peak_read_repeats: u32,
    #[serde(default)]
    pub fold_thrash: bool,
    #[serde(default)]
    pub big_read_turns: u32,
    pub tool_calls: u32,
    pub gate_attempts: u32,
    pub rollbacks: u32,
    pub provider_retries: u32,
    /// List-price cost in USD, when the model's price is known.
    pub cost_usd: Option<f64>,
    /// Spending cap in USD given with `--max-cost`, if any.
    pub max_cost_usd: Option<f64>,
    /// Whether the run stopped because spending reached `--max-cost`.
    pub cost_capped: bool,
    /// Fold budget given with `--fold-budget`, if any.
    pub fold_budget: Option<usize>,
    /// Per-call usage (see `blyte_agent::run::TurnUsage`).
    pub turn_usage: Vec<blyte_agent::run::TurnUsage>,
    pub usage: UsageRecord,
    pub gate_ms: Vec<u64>,
    #[serde(default)]
    pub gate_turns: Vec<u32>,
    pub gate_wait_ms: Vec<u64>,
    pub gate_failures: Vec<String>,
    pub tool_counts: BTreeMap<String, u32>,
    pub model_seconds: f64,
    pub tools_seconds: f64,
    pub gate_seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_problems: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_check: Option<blyte_agent::claim_check::ClaimCheck>,
}

impl RunRecord {
    fn base(config: &TaskConfig, elapsed: Duration) -> Self {
        let gated = config.gate.as_deref().is_some_and(|g| !g.trim().is_empty());
        let sandboxed = config.container.is_none()
            && config.sandbox == "on"
            && blyte_agent::sandbox::available();
        let withheld_env = if config.container.is_some() {
            Vec::new()
        } else {
            blyte_agent::shell::withheld_env()
        };
        Self {
            schema_version: SCHEMA_VERSION,
            harness_build: env!("BLYTE_BUILD").into(),
            outcome: "error".into(),
            error: None,
            final_message: None,
            model_requested: config.model.clone(),
            model_id: None,
            effort: None,
            context: config.context_strategy.clone(),
            gated,
            gate_source: if gated { "user" } else { "none" }.into(),
            container: config.container.clone(),
            container_workdir: config.container_workdir.clone(),
            sandboxed,
            sandbox: sandbox_kind(config),
            network: config.network.clone(),
            allow_read: config
                .allow_read
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            pass_env: config.pass_env.clone(),
            withheld_env,
            duration_seconds: elapsed.as_secs_f64(),
            turns: 0,
            empty_turns: 0,
            truncated_turns: 0,
            read_loop: false,
            peak_read_repeats: 0,
            fold_thrash: false,
            big_read_turns: 0,
            tool_calls: 0,
            gate_attempts: 0,
            rollbacks: 0,
            provider_retries: 0,
            cost_usd: None,
            max_cost_usd: config.max_cost_usd,
            cost_capped: false,
            fold_budget: config.fold_budget,
            turn_usage: Vec::new(),
            usage: UsageRecord::default(),
            gate_ms: Vec::new(),
            gate_turns: Vec::new(),
            gate_wait_ms: Vec::new(),
            gate_failures: Vec::new(),
            tool_counts: BTreeMap::new(),
            model_seconds: 0.0,
            tools_seconds: 0.0,
            gate_seconds: 0.0,
            quote_problems: None,
            claim_check: None,
        }
    }

    /// A run that never reached the loop, e.g. a missing API key.
    pub fn from_error(config: &TaskConfig, error: &str, elapsed: Duration) -> Self {
        Self {
            error: Some(error.to_string()),
            ..Self::base(config, elapsed)
        }
    }

    /// A run the loop completed, whatever its outcome.
    pub fn from_run(
        config: &TaskConfig,
        model_id: &str,
        outcome: &Outcome,
        report: &RunReport,
        state: &State,
        elapsed: Duration,
    ) -> Self {
        let (outcome_name, error) = outcome_name(outcome);
        let model_ms: u64 = report.turn_usage.iter().map(|t| t.model_ms).sum();
        let tools_ms: u64 = report.turn_usage.iter().map(|t| t.tools_ms).sum();
        let gate_ms_sum: u64 = report.gate_ms.iter().sum();
        Self {
            outcome: outcome_name.into(),
            error,
            final_message: report.final_message.clone(),
            model_id: Some(model_id.to_string()),
            turns: report.outcome_turns,
            empty_turns: report.empty_turns,
            truncated_turns: report.truncated_turns,
            read_loop: report.read_loop,
            peak_read_repeats: report.peak_read_repeats,
            fold_thrash: report.fold_thrash,
            big_read_turns: report.big_read_turns,
            tool_calls: report.tool_calls,
            gate_attempts: report.gate_attempts,
            rollbacks: state.runtime.rollbacks,
            provider_retries: report.provider_retries,
            cost_usd: blyte_provider::pricing(model_id).map(|p| p.cost(&report.usage)),
            cost_capped: report.cost_capped,
            turn_usage: report.turn_usage.clone(),
            usage: UsageRecord {
                input: report.usage.input,
                cached: report.usage.cached,
                output: report.usage.output,
                thinking: report.usage.thinking,
            },
            gate_ms: report.gate_ms.clone(),
            gate_turns: report.gate_turns.clone(),
            gate_wait_ms: report.gate_wait_ms.clone(),
            gate_failures: report.gate_failures.clone(),
            tool_counts: report.tool_counts.clone(),
            model_seconds: model_ms as f64 / 1000.0,
            tools_seconds: tools_ms as f64 / 1000.0,
            gate_seconds: gate_ms_sum as f64 / 1000.0,
            quote_problems: report.quote_problems.clone(),
            claim_check: report.claim_check.clone(),
            ..Self::base(config, elapsed)
        }
    }
}

pub fn write(path: &Path, record: &RunRecord) -> Result<(), String> {
    let json = serde_json::to_string_pretty(record)
        .map_err(|e| format!("report serialisation failed: {e}"))?;
    std::fs::write(path, json + "\n")
        .map_err(|e| format!("cannot write report {}: {e}", path.display()))
}

/// Returns the name of the sandbox mechanism active for the task configuration.
pub fn sandbox_kind(config: &TaskConfig) -> String {
    if config.container.is_some() {
        return "container".into();
    }
    if config.sandbox != "on" || !blyte_agent::sandbox::available() {
        return "off".into();
    }
    match blyte_agent::sandbox::default_profile_net(
        &config.root,
        &config.allow_read,
        config.network == "on",
    ) {
        Profile::Seatbelt(_) => "seatbelt".into(),
        Profile::Landlock(_) => "landlock".into(),
        Profile::LandlockOffline(_) => "landlock-offline".into(),
        Profile::LandlockConfined { block_tcp, .. } => {
            if block_tcp {
                "landlock-offline".into()
            } else {
                "landlock".into()
            }
        }
    }
}
