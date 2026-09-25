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
//! This is the piece blyte asserted as doctrine for months while the code let a
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

use blyte_provider::{Message, Provider, ProviderError, Request, Usage};

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
    /// Wall-clock limit, checked before each turn. The harness stops itself and reports, rather
    /// than being killed from outside with its telemetry lost (FINDINGS 13.5).
    pub max_duration: Option<Duration>,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_turns: 60,
            max_gate_attempts: 5,
            max_duration: None,
        }
    }
}

/// One call of the main loop: what it cost, and how its prompt related to the previous call's.
/// `appended` means the prompt was the previous one plus `new_chars` of new messages, the only
/// case where a prefix cache can serve everything but the new part. Fresh input far above
/// `new_chars / 4` on an appended call is a cache miss; on a rewritten one (a fold) it is expected.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TurnUsage {
    pub turn: u32,
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub thinking: u64,
    pub prompt_messages: usize,
    pub appended: bool,
    pub new_chars: usize,
    /// The fold budget in force for this call, for strategies that fold.
    #[serde(default)]
    pub fold_budget: Option<usize>,
}

/// Characters a message contributes to a prompt, as the context strategies size them.
fn message_chars(m: &Message) -> usize {
    match m {
        Message::User(t) | Message::Assistant(t) => t.len(),
        Message::ToolCall { arguments, .. } => arguments.to_string().len(),
        Message::ToolResult { content, .. } => content.len(),
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
    /// Transient provider failures (rate limits, overload) waited out during the run.
    pub provider_retries: u32,
    /// Completeness reviews run after a passing verification.
    pub reviews: u32,
    /// Reviews that sent the worker back with unmet requirements.
    pub review_rejections: u32,
    /// One entry per main-loop model call, in order.
    pub turn_usage: Vec<TurnUsage>,
}

/// What happened on one turn, emitted as it happens.
///
/// A harness whose selling point is observability printed nothing until the run finished, so a
/// working run and one stuck on `find /` looked identical for three and a half minutes. Progress
/// is reported as it occurs, not summarised at the end.
#[derive(Debug, Clone)]
pub enum Event<'e> {
    Turn {
        n: u32,
        text: &'e str,
    },
    /// `input` is the call's arguments, abbreviated. Logging only the RESULT made a run
    /// undiagnosable: twenty bash calls that changed nothing, with no record of what was run.
    Tool {
        name: &'e str,
        input: String,
        summary: String,
        is_error: bool,
    },
    Gate {
        attempt: u32,
        passed: bool,
        output: &'e str,
    },
    RolledBack {
        to: &'e str,
    },
}

/// One stage of a multi-phase run, with its own instruction and verification command.
///
/// Phases belong to the runtime, not the worker. The harness derives them (for example from the
/// repository, by autonomous TDD), and the worker can neither see nor edit the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase {
    pub id: String,
    pub instruction: String,
    pub verify_cmd: String,
    /// The only tracked path allowed to differ from the phase's starting commit when its gate
    /// passes. RED uses it: a worker that fixes the bug while "writing a failing test" makes
    /// the RED proof meaningless.
    pub scope_only: Option<String>,
    /// A path that must be unchanged from the phase's starting commit when its gate passes.
    /// GREEN uses it: the test that defines done may not be weakened after it was seen to fail.
    pub protect: Option<String>,
}

impl Phase {
    /// The command the gate actually runs: the phase's own command plus its mechanical checks,
    /// measured against `start`, the commit captured when the phase began. A worker that commits
    /// mid-phase cannot move that reference point.
    pub fn gate(&self, start: &str) -> String {
        let mut cmd = format!("( {} )", self.verify_cmd);
        if let Some(p) = &self.scope_only {
            cmd.push_str(&format!(
                " && {{ git diff --quiet {start} -- . ':(exclude){p}' || \
                 {{ echo 'blyte: in this phase only {p} may change; revert your other edits'; false; }}; }}"
            ));
        }
        if let Some(p) = &self.protect {
            cmd.push_str(&format!(
                " && {{ git diff --quiet {start} -- '{p}' || \
                 {{ echo 'blyte: {p} defines done and may not change in this phase'; false; }}; }}"
            ));
        }
        cmd
    }
}

/// How one phase of a multi-phase run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseResult {
    pub id: String,
    pub outcome: Outcome,
    pub turns: u32,
    pub gate_attempts: u32,
}

/// Review rejections allowed before a passing verification is accepted as-is. The exit code stays
/// the verdict: a review can only send the worker back, and only this many times.
pub const MAX_REVIEW_ROUNDS: u32 = 2;

/// The completeness reviewer's instructions. It judges only whether the task is done, not style:
/// a reviewer that blocks on naming or refactoring would spend turns on work nobody asked for.
const REVIEW_SYSTEM: &str = "You review whether a code change fully accomplishes a task. You are \
not its author. Compare the diff against every behaviour the task statement requires. Report only \
requirements the change does not implement or implements incorrectly, each with severity \
\"Critical\" or \"High\". Do not report style, naming, refactoring, documentation or hypothetical \
improvements. Respond with strict JSON only: {\"summary\": \"<one sentence>\", \"findings\": \
[{\"severity\": \"Critical\" or \"High\", \"file\": \"<path>\", \"line_range\": [<start>, <end>], \
\"problem\": \"<the unmet requirement>\", \"suggested_fix\": \"<what to change>\"}], \
\"has_blocking_issues\": true or false}. If every requirement is met, return an empty findings \
list and has_blocking_issues false.";

#[derive(serde::Deserialize)]
struct ReviewVerdict {
    #[serde(default)]
    findings: Vec<ReviewFinding>,
}

#[derive(serde::Deserialize)]
struct ReviewFinding {
    #[serde(default)]
    severity: String,
    #[serde(default)]
    file: String,
    #[serde(default)]
    problem: String,
    #[serde(default)]
    suggested_fix: String,
}

/// Retries allowed for one model call before a transient failure ends the run.
pub const MAX_PROVIDER_RETRIES: u32 = 6;

/// Transient provider failures worth waiting out: rate limits, overload, server errors and
/// dropped connections. Anything else (a malformed request, a bad key) fails the run at once.
pub fn is_transient(e: &ProviderError) -> bool {
    match e {
        ProviderError::Api { status, .. } => matches!(status, 429 | 500 | 502 | 503 | 504),
        ProviderError::Transport(_) => true,
        _ => false,
    }
}

/// Wait before retry `attempt` (1-based): 2, 4, 8, 16, 32, then 60 seconds.
pub fn retry_delay(attempt: u32) -> Duration {
    let exp = attempt.saturating_sub(1).min(5);
    Duration::from_secs((2u64 << exp).min(60))
}

/// How long the server asked us to wait, if it said: Gemini puts `"retryDelay": "37s"` in a
/// 429's body. Capped at 120 seconds.
pub fn server_retry_hint(e: &ProviderError) -> Option<Duration> {
    let ProviderError::Api { body, .. } = e else {
        return None;
    };
    let key = "\"retryDelay\"";
    let rest = &body[body.find(key)? + key.len()..];
    let value = &rest[rest.find('"')? + 1..];
    let secs: f64 = value[..value.find('s')?].trim().parse().ok()?;
    Some(Duration::from_secs_f64(secs.max(0.0)).min(Duration::from_secs(120)))
}

/// Wait before retry `attempt` (1-based): the backoff, or the server's hint when it is longer.
pub fn retry_wait(e: &ProviderError, attempt: u32) -> Duration {
    let backoff = retry_delay(attempt);
    server_retry_hint(e).map_or(backoff, |hint| hint.max(backoff))
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
        if let Some(f) = self.on_event {
            f(e)
        }
    }
}

impl<'a> Runner<'a> {
    /// The immutable part of the prompt: who the worker is, what it may touch, where it runs.
    ///
    /// Held constant for the whole run so it caches.
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
             ## Working efficiently\n\
             Independent reads, searches and commands can be issued as several tool calls in one \
             turn: they run in order and you see every result together. Batch them whenever one \
             does not depend on another's result; every turn you save is a full model call.\n\n\
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
        match self.shell.exec(
            &state.orchestrator.verification_cmd,
            &self.root,
            Duration::from_secs(1800),
        ) {
            Ok(o) => {
                let body = tools::bound_output(o.merged().trim_end());
                (o.exit_code == 0, format!("exit {}\n{body}", o.exit_code))
            }
            // A gate that cannot execute has not passed. Treating an error as success is how
            // an ungated run gets mistaken for a verified one.
            Err(e) => (false, format!("verification command could not run: {e}")),
        }
    }

    /// An independent review of the change against the goal, by the same model with no tools.
    /// `Some(feedback)` when it names Critical or High unmet requirements. Any failure to review
    /// (no baseline, empty diff, provider error, unparseable reply) accepts: the exit code remains
    /// the verdict, and a reviewer that cannot run must not be able to block completion.
    fn review(
        &self,
        state: &State,
        baseline: Option<&str>,
        report: &mut RunReport,
    ) -> Option<String> {
        let base = baseline?;
        let out = self
            .shell
            .exec(
                &format!("git add -A --intent-to-add && git diff {base}"),
                &self.root,
                Duration::from_secs(60),
            )
            .ok()?;
        if out.exit_code != 0 || out.stdout.trim().is_empty() {
            return None;
        }
        let prompt = format!(
            "## Task\n{}\n\n## Change (git diff against the starting point)\n```diff\n{}\n```\n",
            state.orchestrator.goal,
            tools::distil(&out.stdout, 1500)
        );
        let req = Request {
            system: REVIEW_SYSTEM.into(),
            messages: vec![Message::User(prompt)],
            tools: Vec::new(),
            max_output_tokens: None,
        };
        let resp = self.provider.complete(&req).ok()?;
        report.usage.input += resp.usage.input;
        report.usage.cached += resp.usage.cached;
        report.usage.output += resp.usage.output;
        report.usage.thinking += resp.usage.thinking;
        report.reviews += 1;

        let text = resp.text;
        let (start, end) = (text.find('{')?, text.rfind('}')?);
        if end < start {
            return None;
        }
        let verdict: ReviewVerdict = serde_json::from_str(&text[start..=end]).ok()?;
        let blocking: Vec<&ReviewFinding> = verdict
            .findings
            .iter()
            .filter(|f| {
                matches!(
                    f.severity.to_ascii_lowercase().as_str(),
                    "critical" | "high"
                )
            })
            .collect();
        if blocking.is_empty() {
            return None;
        }
        let mut feedback = String::from(
            "The verification command passed, but an independent review of your change against \
             the task found requirements it does not meet:\n",
        );
        for f in blocking {
            feedback.push_str(&format!(
                "- {} (in {}): {}\n",
                f.problem, f.file, f.suggested_fix
            ));
        }
        feedback
            .push_str("\nAddress them, then stop again; the verification command will run again.");
        Some(feedback)
    }

    /// Ask the worker what to keep from messages `first..=last`, which are about to be folded.
    /// `prefix` is the previous request's messages plus everything since, and the system prompt
    /// and tools are the loop's own, so all but the newest turn is read from cache. Any failure
    fn checkpoint(&self) -> Option<String> {
        let out = self
            .shell
            .exec("git rev-parse HEAD", &self.root, Duration::from_secs(30))
            .ok()?;
        (out.exit_code == 0).then(|| out.stdout.trim().to_string())
    }

    fn reset_to(&self, commit: &str) {
        let _ = self.shell.exec(
            &format!("git reset --hard {commit} && git clean -fd"),
            &self.root,
            Duration::from_secs(120),
        );
    }

    pub fn run(&self, state: &mut State) -> (Outcome, RunReport) {
        let deadline = self
            .budget
            .max_duration
            .map(|d| std::time::Instant::now() + d);
        self.run_until(state, deadline)
    }

    /// Run `phases` in order under one deadline. After each verified phase except the last, a
    /// checkpoint is committed, so the next phase's rollback baseline keeps it: a GREEN rollback
    /// must never delete RED's verified test. Turn and gate budgets apply per phase, matching
    /// the daemon's per-milestone retry budget. The deadline covers the whole run.
    pub fn run_phases(
        &self,
        state: &mut State,
        phases: &[Phase],
    ) -> (Outcome, RunReport, Vec<PhaseResult>) {
        let deadline = self
            .budget
            .max_duration
            .map(|d| std::time::Instant::now() + d);
        let task = state.orchestrator.goal.clone();
        let wants_review = state.orchestrator.review;
        let mut total = RunReport::default();
        let mut results = Vec::new();
        for (i, phase) in phases.iter().enumerate() {
            let Some(start) = self.checkpoint() else {
                state.orchestrator.review = wants_review;
                return (
                    Outcome::Error("phases need a git repository: cannot read HEAD".into()),
                    total,
                    results,
                );
            };
            state.orchestrator.goal = format!(
                "{task}\n\n## Current milestone ({}/{}): {}\n{}",
                i + 1,
                phases.len(),
                phase.id,
                phase.instruction
            );
            state.orchestrator.verification_cmd = phase.gate(&start);
            state.orchestrator.review = wants_review && i + 1 == phases.len();
            let (outcome, report) = self.run_until(state, deadline);
            total.outcome_turns += report.outcome_turns;
            total.usage.input += report.usage.input;
            total.usage.cached += report.usage.cached;
            total.usage.output += report.usage.output;
            total.usage.thinking += report.usage.thinking;
            total.empty_turns += report.empty_turns;
            total.tool_calls += report.tool_calls;
            total.gate_attempts += report.gate_attempts;
            total.provider_retries += report.provider_retries;
            total.reviews += report.reviews;
            total.review_rejections += report.review_rejections;
            results.push(PhaseResult {
                id: phase.id.clone(),
                outcome: outcome.clone(),
                turns: report.outcome_turns,
                gate_attempts: report.gate_attempts,
            });
            if outcome != Outcome::Verified {
                state.orchestrator.goal = task;
                state.orchestrator.review = wants_review;
                return (outcome, total, results);
            }
            if i + 1 < phases.len() {
                if let Err(e) = self.commit_checkpoint(&phase.id) {
                    state.orchestrator.goal = task;
                    state.orchestrator.review = wants_review;
                    return (Outcome::Error(e), total, results);
                }
            }
        }
        state.orchestrator.goal = task;
        state.orchestrator.review = wants_review;
        (Outcome::Verified, total, results)
    }

    /// Commit the tree as the verified end of a phase. The identity is set inline because a
    /// task container often has no git user configured.
    fn commit_checkpoint(&self, phase_id: &str) -> Result<(), String> {
        let cmd = format!(
            "git add -A && git -c user.name=blyte -c user.email=blyte@localhost \
             commit --no-gpg-sign -q --allow-empty -m 'chore(blyte): checkpoint phase {phase_id}'"
        );
        match self.shell.exec(&cmd, &self.root, Duration::from_secs(120)) {
            Ok(o) if o.exit_code == 0 => Ok(()),
            Ok(o) => Err(format!(
                "checkpoint commit for phase {phase_id} failed: {}",
                o.merged().trim()
            )),
            Err(e) => Err(format!(
                "checkpoint commit for phase {phase_id} could not run: {e}"
            )),
        }
    }

    fn run_until(
        &self,
        state: &mut State,
        deadline: Option<std::time::Instant>,
    ) -> (Outcome, RunReport) {
        let mut report = RunReport::default();
        let mut history = vec![Message::User(format!("Begin. {}", state.orchestrator.goal))];
        let system = self.system_prompt(state);
        let specs = tools::specs();
        let baseline = self.checkpoint();
        let mut review_rounds = 0;
        let mut last_sent: Option<Vec<Message>> = None;

        for turn in 1..=self.budget.max_turns {
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                return (Outcome::BudgetExhausted, report);
            }
            report.outcome_turns = turn;

            let req = Request {
                system: system.clone(),
                messages: self.context.build(state, &history),
                tools: specs.clone(),
                max_output_tokens: None,
            };
            let (appended, new_chars) = match &last_sent {
                Some(prev)
                    if req.messages.len() >= prev.len()
                        && req.messages[..prev.len()] == prev[..] =>
                {
                    (
                        true,
                        req.messages[prev.len()..].iter().map(message_chars).sum(),
                    )
                }
                _ => (false, req.messages.iter().map(message_chars).sum()),
            };
            let fold_budget = self.context.fold_budget(&history);
            last_sent = Some(req.messages.clone());
            // A burst rate limit ended a run after 28 calls (2026-09-23). Transient failures are
            // waited out, but never past the deadline: a run that cannot finish in time reports.
            let mut attempt = 0;
            let resp = loop {
                match self.provider.complete(&req) {
                    Ok(r) => break r,
                    Err(e) if is_transient(&e) && attempt < MAX_PROVIDER_RETRIES => {
                        attempt += 1;
                        let wait = retry_wait(&e, attempt);
                        if deadline.is_some_and(|d| std::time::Instant::now() + wait >= d) {
                            return (Outcome::Error(e.to_string()), report);
                        }
                        report.provider_retries += 1;
                        eprintln!(
                            "provider: {e}; retrying in {}s ({attempt}/{MAX_PROVIDER_RETRIES})",
                            wait.as_secs()
                        );
                        std::thread::sleep(wait);
                    }
                    Err(e) => return (Outcome::Error(e.to_string()), report),
                }
            };
            report.usage.input += resp.usage.input;
            report.usage.cached += resp.usage.cached;
            report.usage.output += resp.usage.output;
            report.usage.thinking += resp.usage.thinking;
            report.turn_usage.push(TurnUsage {
                turn,
                input: resp.usage.input,
                cached: resp.usage.cached,
                output: resp.usage.output,
                thinking: resp.usage.thinking,
                prompt_messages: req.messages.len(),
                appended,
                new_chars,
                fold_budget,
            });

            if resp.text.trim().is_empty() && resp.tool_calls.is_empty() {
                report.empty_turns += 1;
            }
            self.emit(Event::Turn {
                n: turn,
                text: resp.text.trim(),
            });
            if !resp.text.trim().is_empty() {
                history.push(Message::Assistant(resp.text.clone()));
            }

            // --- the model is still working -------------------------------------------------
            if !resp.tool_calls.is_empty() {
                // A turn's calls are recorded together, then its results together, in call
                // order. Providers encode one model turn carrying several calls and one reply
                // carrying their results; interleaving them split a parallel batch into turns
                // Gemini rejects (it signs only the first call of a batch).
                for call in &resp.tool_calls {
                    history.push(Message::ToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        signature: call.signature.clone(),
                    });
                }
                for call in &resp.tool_calls {
                    report.tool_calls += 1;

                    // `note` is the loop's, not the tool layer's: it mutates state, and only
                    // the agent region can be reached from here.
                    let outcome = if call.name == "note" {
                        let patch = call.arguments.get("patch").cloned().unwrap_or_default();
                        state.apply_agent_patch(&patch);
                        crate::tools::ToolOutcome {
                            content: format!(
                                "recorded; your state now has {} field(s)",
                                state.agent.fields.len()
                            ),
                            is_error: false,
                        }
                    } else {
                        tools::execute(&self.root, self.shell, self.fs, &call.name, &call.arguments)
                    };

                    let summary = outcome
                        .content
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(90)
                        .collect::<String>();
                    // A read's line range is part of what it did: paging through a file and
                    // re-reading it look identical without it (superstate-eval 13.43).
                    let input = if call.name == "read" {
                        extract_action_target(&call.name, &call.arguments)
                            .chars()
                            .take(100)
                            .collect()
                    } else {
                        call.arguments
                            .get("command")
                            .or_else(|| call.arguments.get("pattern"))
                            .or_else(|| call.arguments.get("path"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.chars().take(70).collect::<String>())
                            .unwrap_or_else(|| {
                                call.arguments.to_string().chars().take(70).collect()
                            })
                    };
                    self.emit(Event::Tool {
                        name: &call.name,
                        input,
                        summary,
                        is_error: outcome.is_error,
                    });

                    history.push(Message::ToolResult {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        content: outcome.content,
                        is_error: outcome.is_error,
                    });
                }
                continue;
            }

            // --- the model stopped calling tools: the runtime decides -----------------------
            report.gate_attempts += 1;
            state.runtime.attempts += 1;
            let (passed, output) = self.run_gate(state);
            self.emit(Event::Gate {
                attempt: report.gate_attempts,
                passed,
                output: output.lines().next().unwrap_or(""),
            });
            if passed {
                // The exit code is necessary, not sufficient: a worker-written gate is only as
                // complete as the worker's reading of the task (superstate-eval FINDINGS 13.38).
                if state.orchestrator.review && review_rounds < MAX_REVIEW_ROUNDS {
                    if let Some(feedback) = self.review(state, baseline.as_deref(), &mut report) {
                        review_rounds += 1;
                        report.review_rejections += 1;
                        history.push(Message::User(feedback));
                        continue;
                    }
                }
                return (Outcome::Verified, report);
            }

            state.runtime.failures.push(output.clone());
            if self.rollback {
                if let Some(base) = &baseline {
                    self.reset_to(base);
                    state.runtime.rollbacks += 1;
                    let short_sha = &base[..base.len().min(8)];
                    self.emit(Event::RolledBack { to: short_sha });
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
            } else {
                ""
            };
            history.push(Message::User(format!(
                "Verification did not pass.\n\n{output}{reset_note}"
            )));
        }
        (Outcome::BudgetExhausted, report)
    }
}

/// Extracts a concise, human-readable target descriptor from a tool call's arguments.
///
/// Deterministic and zero-latency: formats tool targets without LLM invocation to
/// bound the prompt footprint in episodic action ledgers.
///
/// # Arguments
/// * `tool` - The name of the tool invoked (e.g. "read", "bash", "write").
/// * `args` - The JSON arguments supplied to the tool.
///
/// # Returns
/// A compact string identifying the target file, command, or parameter.
pub fn extract_action_target(tool: &str, args: &serde_json::Value) -> String {
    // Discriminate on known tool types to extract their canonical target representations.
    match tool {
        "read" => {
            // Extract the target file path and optionally the requested line range.
            if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                let start = args.get("start_line").and_then(|v| {
                    v.as_u64()
                        .map(|n| n.to_string())
                        .or_else(|| v.as_str().map(|s| s.to_string()))
                });
                let end = args.get("end_line").and_then(|v| {
                    v.as_u64()
                        .map(|n| n.to_string())
                        .or_else(|| v.as_str().map(|s| s.to_string()))
                });
                // When line bounds are specified, annotate the path with 1-indexed line boundaries.
                match (start, end) {
                    (Some(s), Some(e)) => return format!("{path} (L{s}-L{e})"),
                    (Some(s), None) => return format!("{path} (L{s}+)"),
                    (None, Some(e)) => return format!("{path} (L1-L{e})"),
                    (None, None) => return path.to_string(),
                }
            }
        }
        "write" => {
            // Extract the destination path and include byte size for visibility into write volume.
            if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                if let Some(content) = args.get("content").and_then(|v| v.as_str()) {
                    return format!("{path} ({} bytes)", content.len());
                }
                return path.to_string();
            }
        }
        "edit" => {
            // Edit targets are identified primarily by the target file being modified.
            if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                return path.to_string();
            }
        }
        "bash" => {
            // Bash commands are bounded at 60 characters so long pipelines do not dominate context.
            // Newlines are sanitized to spaces to preserve single-line formatting in the action ledger.
            if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                let single_line = cmd.replace('\n', " ");
                if single_line.chars().count() > 60 {
                    let prefix: String = single_line.chars().take(57).collect();
                    return format!("{prefix}...");
                }
                return single_line;
            }
        }
        "glob" => {
            // Glob targets are represented by the file search pattern.
            if let Some(pattern) = args.get("pattern").and_then(|v| v.as_str()) {
                return pattern.to_string();
            }
        }
        "grep" => {
            // Grep targets represent the regex pattern and, when restricted, the target subtree.
            if let Some(pattern) = args.get("pattern").and_then(|v| v.as_str()) {
                if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                    if !path.is_empty() {
                        return format!("pattern '{pattern}' in {path}");
                    }
                }
                return format!("pattern '{pattern}'");
            }
        }
        "note" => {
            // Note patches are summarized by the number of state keys modified.
            if let Some(patch) = args.get("patch").and_then(|v| v.as_object()) {
                return format!("patch ({} keys)", patch.len());
            }
        }
        _ => {}
    }

    // Fallback: extract the first string argument value found, or default to the tool name.
    if let Some(obj) = args.as_object() {
        for val in obj.values() {
            if let Some(s) = val.as_str() {
                return s.to_string();
            }
        }
    }
    tool.to_string()
}

/// Extracts a one-line deterministic outcome summary from a tool execution result.
///
/// # Arguments
/// * `tool` - The name of the tool executed.
/// * `content` - Raw string output produced by the tool.
/// * `is_error` - Whether the tool execution resulted in an error status.
///
/// # Returns
/// A single-line summary string describing the outcome.
pub fn extract_action_summary(tool: &str, content: &str, is_error: bool) -> String {
    // Produce deterministic, bounded status summaries per tool category.
    match tool {
        "read" => {
            if is_error {
                let first = content
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("read failed");
                let clamped: String = first.chars().take(80).collect();
                format!("failed: {clamped}")
            } else {
                // Count lines returned to indicate the volume read without including full content.
                let count = content.lines().count();
                format!("{count} lines read")
            }
        }
        "write" => {
            if is_error {
                let first = content
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("write failed");
                let clamped: String = first.chars().take(80).collect();
                format!("failed: {clamped}")
            } else {
                // Static confirmation of file write completion.
                "file written".to_string()
            }
        }
        "edit" => {
            // Distinguish successful replacements from rejection/failure reasons.
            if is_error {
                let first = content
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("edit failed");
                let clamped: String = first.chars().take(80).collect();
                format!("failed: {clamped}")
            } else {
                "string replaced".to_string()
            }
        }
        "bash" => {
            // Non-zero bash exits extract the terminal failure line to preserve diagnostics.
            if !is_error {
                "exit 0".to_string()
            } else {
                let last_line = content
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("command failed");
                let clamped: String = last_line.chars().take(80).collect();
                format!("failed: {clamped}")
            }
        }
        "glob" => {
            if is_error {
                let first = content
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("glob failed");
                let clamped: String = first.chars().take(80).collect();
                format!("failed: {clamped}")
            } else {
                // Count matching file entries in the glob output.
                let count = content.lines().filter(|l| !l.trim().is_empty()).count();
                format!("{count} files matched")
            }
        }
        "grep" => {
            if is_error {
                let first = content
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("grep failed");
                let clamped: String = first.chars().take(80).collect();
                format!("failed: {clamped}")
            } else {
                // Count matching occurrences found in content search.
                let count = content.lines().filter(|l| !l.trim().is_empty()).count();
                format!("{count} matches found")
            }
        }
        "note" => {
            // Surface the first line of the note confirmation message.
            let first = content.lines().next().unwrap_or("recorded");
            if first.is_empty() {
                "recorded".to_string()
            } else {
                first.to_string()
            }
        }
        _ => {
            // Fallback for custom or unknown tools clamps the first line to 80 chars.
            let first = content.lines().next().unwrap_or("");
            first.chars().take(80).collect()
        }
    }
}
