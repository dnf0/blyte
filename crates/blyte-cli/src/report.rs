//! The headless solver's machine-readable run report (`--report <path>`).
//!
//! This is the contract between the harness and anything that measures it: superstate-eval
//! reads exactly this shape. Every field is always written, because a zero is a measurement and
//! an absent key is a bug. Schema: docs/superpowers/specs/2026-09-23-native-harness-eval-surface-design.md

use std::path::Path;
use std::time::Duration;

use blyte_agent::run::{Outcome, PhaseResult, RunReport};
use blyte_agent::state::{ActionEntry, State};
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

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Interceptions {
    pub read: u32,
    pub bash: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PhaseRecord {
    pub id: String,
    pub outcome: String,
    pub turns: u32,
    pub gate_attempts: u32,
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
    pub loop_breaker: bool,
    pub gated: bool,
    /// Where the gate came from: `none` (ungated), `user` (`--gate`) or `tdd` (`--tdd`).
    pub gate_source: String,
    /// Per-phase results for a multi-phase run; empty otherwise.
    pub phases: Vec<PhaseRecord>,
    pub container: Option<String>,
    pub container_workdir: String,
    pub duration_seconds: f64,
    pub turns: u32,
    pub empty_turns: u32,
    pub tool_calls: u32,
    pub gate_attempts: u32,
    pub rollbacks: u32,
    pub provider_retries: u32,
    pub reviews: u32,
    pub review_rejections: u32,
    /// List-price cost in USD, when the model's price is known.
    pub cost_usd: Option<f64>,
    /// Fold budget given with `--fold-budget`, if any.
    pub fold_budget: Option<usize>,
    /// Per-call usage (see `blyte_agent::run::TurnUsage`).
    pub turn_usage: Vec<blyte_agent::run::TurnUsage>,
    pub usage: UsageRecord,
    pub interceptions: Interceptions,
    pub insists: u32,
    pub agent_note_keys: Vec<String>,
}

impl RunRecord {
    fn base(config: &TaskConfig, elapsed: Duration) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            outcome: "error".into(),
            error: None,
            model_requested: config.model.clone(),
            model_id: None,
            context: config.context_strategy.clone(),
            loop_breaker: config.loop_breaker,
            gated: config.tdd || config.gate.as_deref().is_some_and(|g| !g.trim().is_empty()),
            gate_source: if config.tdd {
                "tdd"
            } else if config.gate.as_deref().is_some_and(|g| !g.trim().is_empty()) {
                "user"
            } else {
                "none"
            }
            .into(),
            phases: Vec::new(),
            container: config.container.clone(),
            container_workdir: config.container_workdir.clone(),
            duration_seconds: elapsed.as_secs_f64(),
            turns: 0,
            empty_turns: 0,
            tool_calls: 0,
            gate_attempts: 0,
            rollbacks: 0,
            provider_retries: 0,
            reviews: 0,
            review_rejections: 0,
            cost_usd: None,
            fold_budget: config.fold_budget,
            turn_usage: Vec::new(),
            usage: UsageRecord::default(),
            interceptions: Interceptions::default(),
            insists: 0,
            agent_note_keys: Vec::new(),
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
        let mut agent_note_keys: Vec<String> = state.agent.fields.keys().cloned().collect();
        agent_note_keys.sort();
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
            reviews: report.reviews,
            review_rejections: report.review_rejections,
            cost_usd: blyte_provider::pricing(model_id).map(|p| p.cost(&report.usage)),
            turn_usage: report.turn_usage.clone(),
            usage: UsageRecord {
                input: report.usage.input,
                cached: report.usage.cached,
                output: report.usage.output,
                thinking: report.usage.thinking,
            },
            interceptions: count_interceptions(&state.runtime.actions),
            insists: count_insists(&state.runtime.actions),
            agent_note_keys,
            ..Self::base(config, elapsed)
        }
    }

    /// Attach per-phase results, in the order the phases ran.
    pub fn with_phases(mut self, results: &[PhaseResult]) -> Self {
        self.phases = results
            .iter()
            .map(|r| PhaseRecord {
                id: r.id.clone(),
                outcome: outcome_name(&r.outcome).0.into(),
                turns: r.turns,
                gate_attempts: r.gate_attempts,
            })
            .collect();
        self
    }
}

/// Loop-breaker interceptions, by guard.
pub fn count_interceptions(actions: &[ActionEntry]) -> Interceptions {
    let mut counts = Interceptions::default();
    for a in actions {
        match a.summary.as_str() {
            "intercepted: already in working set" => counts.read += 1,
            "intercepted: redundant inspection" => counts.bash += 1,
            _ => {}
        }
    }
    counts
}

/// Calls executed immediately after an interception of the same tool and target: the worker
/// insisted. EXP-081 found 90% of perimeter suppressions were insisted on; this is the number
/// that says whether the native guard fares any better.
pub fn count_insists(actions: &[ActionEntry]) -> u32 {
    actions
        .windows(2)
        .filter(|w| {
            w[0].summary.starts_with("intercepted")
                && !w[1].summary.starts_with("intercepted")
                && w[0].tool == w[1].tool
                && w[0].target == w[1].target
        })
        .count() as u32
}

pub fn write(path: &Path, record: &RunRecord) -> Result<(), String> {
    let json = serde_json::to_string_pretty(record)
        .map_err(|e| format!("report serialisation failed: {e}"))?;
    std::fs::write(path, json + "\n")
        .map_err(|e| format!("cannot write report {}: {e}", path.display()))
}
