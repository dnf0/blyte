//! The headless solver's machine-readable run report (`--report <path>`).
//!
//! This is the contract between the harness and anything that measures it: superstate-eval
//! reads exactly this shape. Every field is always written, because a zero is a measurement and
//! an absent key is a bug. Schema: docs/superpowers/specs/2026-09-23-native-harness-eval-surface-design.md

use std::path::Path;
use std::time::Duration;

use blyte_agent::run::{Outcome, RunReport};
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
        Outcome::BudgetExhausted => ("budget_exhausted", None),
        Outcome::GateFailed => ("gate_failed", None),
        Outcome::Error(e) => ("error", Some(e.clone())),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RunRecord {
    pub schema_version: u32,
    pub outcome: String,
    pub error: Option<String>,
    pub model_requested: Option<String>,
    pub model_id: Option<String>,
    pub context: String,
    pub gated: bool,
    /// Where the gate came from: `none` (ungated) or `user` (`--gate`).
    pub gate_source: String,
    pub container: Option<String>,
    pub container_workdir: String,
    pub duration_seconds: f64,
    pub turns: u32,
    pub empty_turns: u32,
    pub tool_calls: u32,
    pub gate_attempts: u32,
    pub rollbacks: u32,
    pub provider_retries: u32,
    /// List-price cost in USD, when the model's price is known.
    pub cost_usd: Option<f64>,
    /// Fold budget given with `--fold-budget`, if any.
    pub fold_budget: Option<usize>,
    /// Per-call usage (see `blyte_agent::run::TurnUsage`).
    pub turn_usage: Vec<blyte_agent::run::TurnUsage>,
    pub usage: UsageRecord,
}

impl RunRecord {
    fn base(config: &TaskConfig, elapsed: Duration) -> Self {
        let gated = config.gate.as_deref().is_some_and(|g| !g.trim().is_empty());
        Self {
            schema_version: SCHEMA_VERSION,
            outcome: "error".into(),
            error: None,
            model_requested: config.model.clone(),
            model_id: None,
            context: config.context_strategy.clone(),
            gated,
            gate_source: if gated { "user" } else { "none" }.into(),
            container: config.container.clone(),
            container_workdir: config.container_workdir.clone(),
            duration_seconds: elapsed.as_secs_f64(),
            turns: 0,
            empty_turns: 0,
            tool_calls: 0,
            gate_attempts: 0,
            rollbacks: 0,
            provider_retries: 0,
            cost_usd: None,
            fold_budget: config.fold_budget,
            turn_usage: Vec::new(),
            usage: UsageRecord::default(),
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
        Self {
            outcome: outcome_name.into(),
            error,
            model_id: Some(model_id.to_string()),
            turns: report.outcome_turns,
            empty_turns: report.empty_turns,
            tool_calls: report.tool_calls,
            gate_attempts: report.gate_attempts,
            rollbacks: state.runtime.rollbacks,
            provider_retries: report.provider_retries,
            cost_usd: blyte_provider::pricing(model_id).map(|p| p.cost(&report.usage)),
            turn_usage: report.turn_usage.clone(),
            usage: UsageRecord {
                input: report.usage.input,
                cached: report.usage.cached,
                output: report.usage.output,
                thinking: report.usage.thinking,
            },
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
