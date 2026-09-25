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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Orchestrator {
    /// What success means, in prose, for the worker.
    pub goal: String,
    /// The shell command whose exit code decides completion. Exit 0 and only exit 0.
    pub verification_cmd: String,
    /// Before a passing verification is accepted, an independent review of the change against the
    /// goal may send the worker back (at most `run::MAX_REVIEW_ROUNDS` times). Part of what "done"
    /// means, so it is set by the caller, never by the worker.
    #[serde(default)]
    pub review: bool,
}

/// Facts the harness observed. Not opinions, and not reported by the worker.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Runtime {
    pub attempts: u32,
    /// Verification failures, most recent last. The text the worker is shown on retry.
    pub failures: Vec<String>,
    pub rollbacks: u32,
    /// Files the harness saw written, in order of first write.
    pub files_written: Vec<String>,
}

/// Execution state across two regions: [`Orchestrator`] and [`Runtime`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
                review: false,
            },
            runtime: Runtime::default(),
        }
    }

    /// The failure text a retry should carry, if any.
    ///
    /// Invariant III: the worker restarts from a clean tree "armed with the exact failure
    /// telemetry of the previous attempt", so the last failure is surfaced and the rest are
    /// kept for the record rather than replayed into context.
    pub fn last_failure(&self) -> Option<&str> {
        self.runtime.failures.last().map(String::as_str)
    }
}
