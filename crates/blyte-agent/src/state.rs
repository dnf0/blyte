//! Execution state, split by authority rather than by who happens to write.
//!
//! Measured basis (FINDINGS 13.30): on a full katex task the state file accumulated 15 notes,
//! **all 15 written by the runtime and none by the agent**, despite the agent having a `note`
//! verb available throughout. The state that proved useful was observed -- what failed, what
//! was rolled back -- not self-reported.
//!
//! The split that follows:
//!
//! | region | written by | agent may mutate |
//! |---|---|---|
//! | [`Orchestrator`] -- goal, gate | human or calling agent | **never** |
//! | [`Runtime`] -- observed facts | the harness | no |
//! | [`Agent`] -- working notes | the worker | yes |
//!
//! The first is the load-bearing one. A control plane whose worker can author its own
//! definition of done is self-grading with extra steps, which is the failure this project spent
//! a week establishing. Enforcing it in the type system means it cannot be undone by a prompt.

use serde::{Deserialize, Serialize};

use crate::working_set::WorkingSet;

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

/// A single action observed and summarized objectively by the harness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionEntry {
    /// Turn number when this action occurred.
    pub turn: u32,
    /// Name of the tool invoked (e.g. "read", "write", "edit", "bash", "glob", "grep", "note", "rollback").
    pub tool: String,
    /// Canonical target or command (e.g. "stats.py (lines 1-30)", "cargo test", "pattern 'mean'").
    pub target: String,
    /// Short 1-line summary of the outcome (e.g. "22 lines read", "exit 1: ZeroDivisionError", "replaced 3 lines").
    pub summary: String,
    /// Whether the action resulted in an error or non-zero exit code.
    pub is_error: bool,
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
    /// Chronological log of all actions executed in this session.
    #[serde(default)]
    pub actions: Vec<ActionEntry>,
    /// Harness-observed working set containing active file snippets and failure output.
    #[serde(default)]
    pub working_set: WorkingSet,
}

/// The worker's own scratch space. Free-form on purpose.
///
/// SKILL.state's first named limitation is that a fixed schema must be known in advance, and
/// nobody knows what it is for a repository. Rather than guess, this region is an open map and
/// the prompt explicitly invites the worker to use it -- which the previous harness never did.
/// What keys actually appear across real tasks is the measurement that decides whether a
/// schema can be formalised at all.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Agent {
    #[serde(flatten)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub orchestrator: Orchestrator,
    pub runtime: Runtime,
    pub agent: Agent,
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
            agent: Agent::default(),
        }
    }

    /// Merge a patch into the AGENT region only, with null-deletion semantics.
    ///
    /// Modelled on SKILL.state's `Σt+1 = Σt ⊕ ΔΣt`, restricted to the one region the worker
    /// owns. A patch naming `goal` or `verification_cmd` does not fail loudly and does not
    /// partially apply -- there is simply nowhere for it to land, because this method cannot
    /// reach the other regions.
    pub fn apply_agent_patch(&mut self, patch: &serde_json::Value) {
        let Some(obj) = patch.as_object() else { return };
        for (k, v) in obj {
            if v.is_null() {
                self.agent.fields.remove(k);
            } else {
                self.agent.fields.insert(k.clone(), v.clone());
            }
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
