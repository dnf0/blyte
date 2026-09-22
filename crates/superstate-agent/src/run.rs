//! The turn loop, and the gate the worker cannot call.
//!
//! # Where the gate sits, and why it is not a tool
//!
//! Invariant II: an LLM must never grade its own homework. A gate the model can invoke is a
//! gate it can decline to invoke, or invoke selectively, or report the result of. So the gate
//! is not in the tool surface at all -- the runtime runs it when the model stops calling tools,
//! and the model learns the verdict the same way it learns anything else: as an observation it
//! did not choose.
//!
//! This is the piece superstate asserted as doctrine for months while the code let a
//! worker-authored test become the gate.
//!
//! # Why a failed gate resets the tree
//!
//! Invariant III. A failing attempt's edits must not accumulate across retries; the worker
//! begins each attempt from a verified checkpoint, carrying the failure telemetry rather than a
//! corrupted workspace. Measured independently at 3/3 against 2/3 and 25.3% fewer tokens per
//! solve (EXP-059). Rollback is on by default and switchable, because it is not free -- one run
//! reset a worker's committed feature five times (FINDINGS 13.2).

use std::path::PathBuf;
use std::time::Duration;

use superstate_provider::{Message, Provider, Request, Usage};

use crate::context::ContextStrategy;
use crate::fs::Fs;
use crate::shell::Shell;
use crate::state::State;
use crate::tools;

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The verification command exited 0. The only way to succeed.
    Verified,
    /// Budget spent without the gate passing.
    BudgetExhausted,
    /// The model stopped calling tools and the gate kept failing.
    GateFailed,
    /// The provider or environment broke. Distinct from failing work.
    Error(String),
}

#[derive(Debug, Clone)]
pub struct Budget {
    pub max_turns: u32,
    pub max_gate_attempts: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Self { max_turns: 60, max_gate_attempts: 5 }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunReport {
    pub outcome_turns: u32,
    pub usage: Usage,
    /// Turns whose response contained neither text nor a tool call -- pure waste, tracked
    /// because 47.6% of a previous harness's tokens went to turns that only waited (13.29).
    pub empty_turns: u32,
    pub tool_calls: u32,
    pub gate_attempts: u32,
}

/// What happened on one turn, emitted as it happens.
///
/// A harness whose selling point is observability printed nothing until the run finished, so a
/// working run and one stuck on `find /` looked identical for three and a half minutes. Progress
/// is reported as it occurs, not summarised at the end.
#[derive(Debug, Clone)]
pub enum Event<'e> {
    Turn { n: u32, text: &'e str },
    /// `input` is the call's arguments, abbreviated. Logging only the RESULT made a run
    /// undiagnosable: twenty bash calls that changed nothing, with no record of what was run.
    Tool { name: &'e str, input: String, summary: String, is_error: bool },
    Gate { attempt: u32, passed: bool, output: &'e str },
    RolledBack { to: &'e str },
}

pub struct Runner<'a> {
    pub provider: &'a dyn Provider,
    pub shell: &'a dyn Shell,
    pub fs: &'a dyn Fs,
    pub context: &'a dyn ContextStrategy,
    pub root: PathBuf,
    pub budget: Budget,
    /// Reset the tree to the checkpoint when the gate fails. Invariant III; default on.
    pub rollback: bool,
    /// Called as the run proceeds. Default is silent.
    pub on_event: Option<&'a dyn Fn(Event<'_>)>,
}

impl<'a> Runner<'a> {
    fn emit(&self, e: Event<'_>) {
        if let Some(f) = self.on_event { f(e) }
    }
}

impl<'a> Runner<'a> {
    /// The immutable part of the prompt: who the worker is, what it may touch, where it runs.
    ///
    /// Held constant for the whole run so it caches, which matters most under `StateOnly` where
    /// it is most of the prompt.
    fn system_prompt(&self, state: &State) -> String {
        let bins = tools::available_binaries(self.shell, &self.root).join(", ");
        format!(
            "You are a software engineer working in a repository.\n\n\
             ## Goal\n{goal}\n\n\
             ## How completion is decided\n\
             Your work is accepted when `{verify}` exits 0. That command is run for you when \
             you stop calling tools -- you cannot run it as your final step and you do not \
             decide the verdict. Do not claim to be finished; simply stop when you believe the \
             command will pass, and you will be told.\n\n\
             ## Environment\n{where_}\nAvailable: {bins}\n\n\
             ## Working notes\n\
             Use `note` to record what you learn: the cause once you find it, what you ruled \
             out, which files matter. Your notes persist across turns and are the only thing \
             guaranteed to.\n",
            goal = state.orchestrator.goal,
            verify = state.orchestrator.verification_cmd,
            where_ = self.shell.describe(),
            bins = bins,
        )
    }

    /// Run the verification command. Runtime-only: nothing in the tool surface reaches this.
    fn run_gate(&self, state: &State) -> (bool, String) {
        match self.shell.exec(&state.orchestrator.verification_cmd, &self.root, Duration::from_secs(1800)) {
            Ok(o) => {
                let body = tools::distil(o.merged().trim_end(), tools::OUTPUT_LINE_BUDGET);
                (o.exit_code == 0, format!("exit {}\n{body}", o.exit_code))
            }
            // A gate that cannot execute has not passed. Treating an error as success is how
            // an ungated run gets mistaken for a verified one.
            Err(e) => (false, format!("verification command could not run: {e}")),
        }
    }

    fn checkpoint(&self) -> Option<String> {
        let out = self.shell.exec("git rev-parse HEAD", &self.root, Duration::from_secs(30)).ok()?;
        (out.exit_code == 0).then(|| out.stdout.trim().to_string())
    }

    fn reset_to(&self, commit: &str) {
        let _ = self.shell.exec(
            &format!("git reset --hard {commit} && git clean -fd"),
            &self.root, Duration::from_secs(120));
    }

    pub fn run(&self, state: &mut State) -> (Outcome, RunReport) {
        let mut report = RunReport::default();
        let mut history: Vec<Message> = vec![Message::User(format!(
            "Begin. {}", state.orchestrator.goal))];
        let system = self.system_prompt(state);
        let specs = tools::specs();
        let baseline = self.checkpoint();

        for turn in 1..=self.budget.max_turns {
            report.outcome_turns = turn;

            let req = Request {
                system: system.clone(),
                messages: self.context.build(state, &history),
                tools: specs.clone(),
                max_output_tokens: None,
            };
            let resp = match self.provider.complete(&req) {
                Ok(r) => r,
                Err(e) => return (Outcome::Error(e.to_string()), report),
            };
            report.usage.input += resp.usage.input;
            report.usage.cached += resp.usage.cached;
            report.usage.output += resp.usage.output;
            report.usage.thinking += resp.usage.thinking;

            if resp.text.trim().is_empty() && resp.tool_calls.is_empty() {
                report.empty_turns += 1;
            }
            self.emit(Event::Turn { n: turn, text: resp.text.trim() });
            if !resp.text.trim().is_empty() {
                history.push(Message::Assistant(resp.text.clone()));
            }

            // --- the model is still working -------------------------------------------------
            if !resp.tool_calls.is_empty() {
                for call in &resp.tool_calls {
                    report.tool_calls += 1;
                    history.push(Message::ToolCall {
                        id: call.id.clone(), name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        signature: call.signature.clone() });

                    // `note` is the loop's, not the tool layer's: it mutates state, and only
                    // the agent region can be reached from here.
                    let outcome = if call.name == "note" {
                        let patch = call.arguments.get("patch").cloned().unwrap_or_default();
                        state.apply_agent_patch(&patch);
                        crate::tools::ToolOutcome {
                            content: format!("recorded; your state now has {} field(s)",
                                             state.agent.fields.len()),
                            is_error: false,
                        }
                    } else {
                        tools::execute(&self.root, self.shell, self.fs, &call.name, &call.arguments)
                    };

                    let summary = outcome.content.lines().next().unwrap_or("").chars()
                        .take(90).collect::<String>();
                    let input = call.arguments.get("command")
                        .or_else(|| call.arguments.get("pattern"))
                        .or_else(|| call.arguments.get("path"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.chars().take(70).collect::<String>())
                        .unwrap_or_else(|| {
                            call.arguments.to_string().chars().take(70).collect()
                        });
                    self.emit(Event::Tool { name: &call.name, input, summary,
                                            is_error: outcome.is_error });
                    history.push(Message::ToolResult {
                        id: call.id.clone(), name: call.name.clone(),
                        content: outcome.content, is_error: outcome.is_error });
                }
                continue;
            }

            // --- the model stopped calling tools: the runtime decides -----------------------
            report.gate_attempts += 1;
            state.runtime.attempts += 1;
            let (passed, output) = self.run_gate(state);
            self.emit(Event::Gate { attempt: report.gate_attempts, passed,
                                    output: output.lines().next().unwrap_or("") });
            if passed {
                return (Outcome::Verified, report);
            }

            state.runtime.failures.push(output.clone());
            if self.rollback {
                if let Some(base) = &baseline {
                    self.reset_to(base);
                    state.runtime.rollbacks += 1;
                    self.emit(Event::RolledBack { to: &base[..base.len().min(8)] });
                }
            }
            if report.gate_attempts >= self.budget.max_gate_attempts {
                return (Outcome::GateFailed, report);
            }

            // Fed back as an observation, not a request to try harder. The worker is told what
            // the command said and, if the tree was reset, that its edits are gone -- silently
            // resetting would leave it editing against a file it no longer has.
            let reset_note = if self.rollback && baseline.is_some() {
                "\n\nYour changes have been reverted to the last verified state. Start from \
                 the current files, not from what you remember writing."
            } else { "" };
            history.push(Message::User(format!(
                "Verification did not pass.\n\n{output}{reset_note}")));
        }
        (Outcome::BudgetExhausted, report)
    }
}
