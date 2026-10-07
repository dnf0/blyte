//! Execution state, split by authority rather than by who happens to write.
//!
//! The split that follows:
//!
//! | region | written by | agent may mutate |
//! |---|---|---|
//! | [`Orchestrator`] -- goal, gate | human or calling agent | **never** |
//! | [`Runtime`] -- observed facts | the harness | no |
//!
//! The first is the load-bearing one. A control plane whose worker can author its own
//! definition of done is self-grading with extra steps, which is the failure this project spent
//! a week establishing. Enforcing it in the type system means it cannot be undone by a prompt.

use serde::{Deserialize, Serialize};

/// What the orchestrator decided. The worker reads this and can never change it.
///
/// There is deliberately no `&mut` accessor and no setter. Blyte previously asserted
/// "the orchestrator states the verification command" as doctrine while the code happily let
/// the worker's own test become the gate; making it structurally impossible is the difference
/// between a rule and a property.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Orchestrator {
    /// What success means, in prose, for the worker.
    pub goal: String,
    /// The shell command whose exit code decides completion. Exit 0 and only exit 0.
    pub verification_cmd: String,
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
    /// Set for `blyte --orchestrate`: the orchestrator's instructions, which replace the implementer's
    /// framing in the system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Facts the harness observed. Not opinions, and not reported by the worker.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Runtime {
    pub rollbacks: u32,
    /// The conversation so far, kept across prompts in an interactive session. `None`: each run starts
    /// fresh (one-shot runs and dispatches). `Some`: the run continues it and stores it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<Vec<blyte_provider::Message>>,
    /// The goal of the conversation's first prompt; the system prompt is built from it for the whole
    /// conversation, so it stays identical and its cache holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_goal: Option<String>,
}

/// Execution state across two regions: [`Orchestrator`] and [`Runtime`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub orchestrator: Orchestrator,
    pub runtime: Runtime,
}

impl State {
    pub fn new(goal: impl Into<String>, verification_cmd: impl Into<String>) -> Self {
        Self {
            orchestrator: Orchestrator {
                goal: goal.into(),
                verification_cmd: verification_cmd.into(),
                max_cost_usd: None,
                instructions: None,
            },
            runtime: Runtime::default(),
        }
    }
}
