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

use superstate_provider::{Message, Provider, ProviderError, Request, ToolCall, ToolSpec, Usage};

use crate::context::ContextStrategy;
use crate::fs::Fs;
use crate::shell::Shell;
use crate::state::{ActionEntry, State};
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
    /// Dollar limit at list price (`superstate_provider::pricing`), checked before each turn of a
    /// `run_until` call (so per phase in a phased run). Requires a model with a known price.
    pub max_cost_usd: Option<f64>,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_turns: 60,
            max_gate_attempts: 5,
            max_duration: None,
            max_cost_usd: None,
        }
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
    /// Fold notes recorded, and fold-note requests that failed (the fold kept the harness digest).
    pub fold_notes: u32,
    pub fold_note_failures: u32,
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
    /// The run so far, emitted before each model call and when the turn budget runs out. Taken
    /// between turns, so `Runner::resume` from it never re-executes a recorded tool result.
    Checkpoint {
        history: &'e [Message],
        state: &'e State,
    },
}

/// The action-ledger tool name `run_phases` writes when a phase begins. Loop-breaker inferences
/// look no further back than the latest marker: what passed in RED says nothing about GREEN.
pub const PHASE_MARKER_TOOL: &str = "phase";

/// Actions since the current phase began, or all of them when the run has no phases.
pub fn current_phase_actions(actions: &[ActionEntry]) -> &[ActionEntry] {
    match actions.iter().rposition(|a| a.tool == PHASE_MARKER_TOOL) {
        Some(i) => &actions[i + 1..],
        None => actions,
    }
}

/// Whether, in the current phase and since its last edit, the worker ran the verification
/// command itself and it exited 0. That is the only exit code that predicts the gate. Another
/// test-like command passing (a compile-only `go test -run '^$'`, a single test) says nothing
/// about it. Treating it as if it did told a GREEN worker its failing test was passing.
pub fn verification_passed_since_edit(actions: &[ActionEntry], verification_cmd: &str) -> bool {
    let v = verification_cmd.trim();
    if v.is_empty() || v == "true" || v == "false" {
        return false;
    }
    let phase = current_phase_actions(actions);
    let edited = phase
        .iter()
        .any(|a| (a.tool == "edit" || a.tool == "write") && !a.is_error);
    edited
        && phase
            .iter()
            .rev()
            .take_while(|a| a.tool != "edit" && a.tool != "write")
            .any(|a| {
                a.tool == "bash"
                    && !a.is_error
                    && !a.summary.starts_with("intercepted")
                    && a.target.trim() == v
            })
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
                 {{ echo 'superstate: in this phase only {p} may change; revert your other edits'; false; }}; }}"
            ));
        }
        if let Some(p) = &self.protect {
            cmd.push_str(&format!(
                " && {{ git diff --quiet {start} -- '{p}' || \
                 {{ echo 'superstate: {p} defines done and may not change in this phase'; false; }}; }}"
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

/// First line of a fold-note request. Test providers recognise note requests by it.
pub const FOLD_NOTE_MARKER: &str = "## Fold notes requested by the harness";

/// Render a fold-note reply: a JSON object with any of `findings`, `ruled_out` and `next`
/// (string lists) and `files` (path -> role). `None` when there is no such object or it says
/// nothing, so the fold keeps the harness digest alone. Deterministic, so the note enters the
/// prompt identically every turn.
pub fn render_fold_note(text: &str) -> Option<String> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    if end < start {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&text[start..=end]).ok()?;
    let obj = v.as_object()?;
    let list = |key: &str| -> Vec<String> {
        obj.get(key)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .take(12)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = String::new();
    let mut section = |title: &str, items: Vec<String>| {
        if !items.is_empty() {
            out.push_str(&format!("{title}:\n"));
            for it in items {
                out.push_str(&format!("- {it}\n"));
            }
        }
    };
    section("Established", list("findings"));
    section("Ruled out", list("ruled_out"));
    let files: Vec<String> = obj
        .get("files")
        .and_then(|x| x.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(p, r)| r.as_str().map(|r| format!("`{p}`: {}", r.trim())))
                .take(12)
                .collect()
        })
        .unwrap_or_default();
    section("Files", files);
    section("Still to do", list("next"));
    (!out.is_empty()).then_some(out)
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
    /// Run the loop-breaker guards (redundant read and inspection interception). Default on;
    /// off is the ablation control, because a guard that changes what the worker sees must be
    /// measured, not assumed (FINDINGS 13.25).
    pub loop_breaker: bool,
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
    /// yields `None`: the fold keeps the harness digest.
    fn fold_note(
        &self,
        system: &str,
        specs: &[ToolSpec],
        prefix: Vec<Message>,
        (first, last): (usize, usize),
        report: &mut RunReport,
    ) -> Option<String> {
        let mut messages = prefix;
        messages.push(Message::User(format!(
            "{FOLD_NOTE_MARKER}\n\
             Messages {first}-{last} are about to be folded: from the next turn you will see only \
             a one-line summary of each call there, not its output. Record now what you still \
             need from them. Reply with only a JSON object and no tool calls:\n\
             {{\"findings\": [\"facts established, with paths, line numbers and identifiers \
             verbatim\"], \"ruled_out\": [\"approaches tried and why they failed\"], \
             \"files\": {{\"path\": \"what it is for\"}}, \"next\": [\"what remains\"]}}\n\
             Keep it under 300 words."
        )));
        let req = Request {
            system: system.to_string(),
            messages,
            tools: specs.to_vec(),
            max_output_tokens: None,
        };
        let resp = self.provider.complete(&req).ok()?;
        report.usage.input += resp.usage.input;
        report.usage.cached += resp.usage.cached;
        report.usage.output += resp.usage.output;
        report.usage.thinking += resp.usage.thinking;
        if !resp.tool_calls.is_empty() {
            return None;
        }
        render_fold_note(&resp.text)
    }

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
        self.run_until(state, deadline, None)
    }

    /// Continue a run from a checkpoint (`Event::Checkpoint`) with a fresh budget. Tool results
    /// already in `history` are never re-executed.
    pub fn resume(&self, state: &mut State, history: Vec<Message>) -> (Outcome, RunReport) {
        let deadline = self
            .budget
            .max_duration
            .map(|d| std::time::Instant::now() + d);
        self.run_until(state, deadline, Some(history))
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
            state.runtime.actions.push(ActionEntry {
                turn: 0,
                tool: PHASE_MARKER_TOOL.into(),
                target: phase.id.clone(),
                summary: format!("phase {}/{} started", i + 1, phases.len()),
                is_error: false,
            });
            let (outcome, report) = self.run_until(state, deadline, None);
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
            "git add -A && git -c user.name=superstate -c user.email=superstate@localhost \
             commit --no-gpg-sign -q --allow-empty -m 'chore(superstate): checkpoint phase {phase_id}'"
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
        resumed: Option<Vec<Message>>,
    ) -> (Outcome, RunReport) {
        let mut report = RunReport::default();
        let resuming = resumed.is_some();
        let mut history: Vec<Message> = resumed
            .unwrap_or_else(|| vec![Message::User(format!("Begin. {}", state.orchestrator.goal))]);
        let system = self.system_prompt(state);
        let specs = tools::specs();
        let baseline = self.checkpoint();
        let mut review_rounds = 0;

        // Fold notes are keyed by position in this history, which starts afresh here (each phase
        // of a multi-phase run has its own).
        if !resuming {
            state.runtime.fold_notes.clear();
        }
        let mut last_sent: Option<(Vec<Message>, usize)> = None;

        let price = superstate_provider::pricing(&self.provider.model_id());
        if self.budget.max_cost_usd.is_some() && price.is_none() {
            return (
                Outcome::Error(format!(
                    "a cost cap needs a known price; none for model {}",
                    self.provider.model_id()
                )),
                report,
            );
        }

        for turn in 1..=self.budget.max_turns {
            self.emit(Event::Checkpoint {
                history: &history,
                state,
            });
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                return (Outcome::BudgetExhausted, report);
            }
            if let (Some(cap), Some(p)) = (self.budget.max_cost_usd, price) {
                if p.cost(&report.usage) >= cap {
                    return (Outcome::BudgetExhausted, report);
                }
            }
            report.outcome_turns = turn;

            // The first time a fold appears, the worker records what to keep from it. The prompt
            // changes then anyway, so the note costs no extra cache miss.
            if state.orchestrator.fold_notes {
                if let Some((sent, len)) = &last_sent {
                    for (a, b) in self.context.folds(&history) {
                        if state.runtime.fold_notes.contains_key(&b) {
                            continue;
                        }
                        let mut prefix = sent.clone();
                        prefix.extend_from_slice(&history[*len..]);
                        let note =
                            self.fold_note(&system, &specs, prefix, (a + 2, b + 1), &mut report);
                        if note.is_some() {
                            report.fold_notes += 1;
                        } else {
                            report.fold_note_failures += 1;
                        }
                        state.runtime.fold_notes.insert(b, note.unwrap_or_default());
                    }
                }
            }

            let req = Request {
                system: system.clone(),
                messages: self.context.build(state, &history),
                tools: specs.clone(),
                max_output_tokens: None,
            };
            last_sent = Some((req.messages.clone(), history.len()));
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

                    let guards_on = self.loop_breaker && !is_insist(call, &state.runtime.actions);

                    // Intercept redundant file reads and redundant read-only inspections before execution
                    // using the loop-breaker guard.
                    // Also `note` is the loop's, not the tool layer's: it mutates state, and only
                    // the agent region can be reached from here.
                    let outcome = if let Some(guard_msg) = guards_on
                        .then(|| {
                            should_intercept_redundant_read(
                                call,
                                &state.runtime.working_set,
                                &state.runtime.actions,
                            )
                        })
                        .flatten()
                    {
                        crate::tools::ToolOutcome {
                            content: guard_msg,
                            is_error: false,
                        }
                    } else if let Some(guard_msg) = guards_on
                        .then(|| {
                            should_intercept_redundant_bash(
                                call,
                                &state.runtime.actions,
                                &state.runtime.working_set,
                                &state.orchestrator.verification_cmd,
                            )
                        })
                        .flatten()
                    {
                        crate::tools::ToolOutcome {
                            content: guard_msg,
                            is_error: false,
                        }
                    } else if call.name == "note" {
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
                    let input = call
                        .arguments
                        .get("command")
                        .or_else(|| call.arguments.get("pattern"))
                        .or_else(|| call.arguments.get("path"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.chars().take(70).collect::<String>())
                        .unwrap_or_else(|| call.arguments.to_string().chars().take(70).collect());
                    self.emit(Event::Tool {
                        name: &call.name,
                        input,
                        summary,
                        is_error: outcome.is_error,
                    });

                    // Record tool invocation in the chronological action ledger.
                    // Captures deterministic targets and outcome summaries so that episodic memory
                    // retains an accurate, bounded audit trail of all agent actions.
                    let action_target = extract_action_target(&call.name, &call.arguments);
                    let action_summary =
                        extract_action_summary(&call.name, &outcome.content, outcome.is_error);
                    state.runtime.actions.push(crate::state::ActionEntry {
                        turn,
                        tool: call.name.clone(),
                        target: action_target,
                        summary: action_summary,
                        is_error: outcome.is_error,
                    });

                    // Synchronize working set on read, edit, write, and bash tool calls.
                    // Maintains external working memory in Runtime (Invariant I) without LLM intervention.
                    match call.name.as_str() {
                        "read" => {
                            // Track active file snippet on successful read, stripping line numbers.
                            // If the read was intercepted by the guard, do not overwrite cached file content.
                            if !outcome.is_error
                                && !outcome
                                    .content
                                    .starts_with("[Harness Working Memory Guard]")
                            {
                                let raw_path = call
                                    .arguments
                                    .get("path")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let path = normalize_path(raw_path);
                                let raw_content = strip_line_number_prefixes(&outcome.content);
                                let start = call.arguments.get("start_line").and_then(|v| {
                                    v.as_u64().map(|n| n as usize).or_else(|| {
                                        v.as_str().and_then(|s| s.parse::<usize>().ok())
                                    })
                                });
                                let end = call.arguments.get("end_line").and_then(|v| {
                                    v.as_u64().map(|n| n as usize).or_else(|| {
                                        v.as_str().and_then(|s| s.parse::<usize>().ok())
                                    })
                                });
                                state.runtime.working_set.add_or_update(
                                    path,
                                    &raw_content,
                                    start,
                                    end,
                                    turn,
                                );
                            }
                        }
                        "edit" => {
                            // Apply exact replacement chunk to working set cache
                            if !outcome.is_error {
                                let raw_path = call
                                    .arguments
                                    .get("path")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let path = normalize_path(raw_path);
                                let old = call
                                    .arguments
                                    .get("old")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let new = call
                                    .arguments
                                    .get("new")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                state.runtime.working_set.apply_edit(path, old, new, turn);
                            }
                        }
                        "write" => {
                            // Update or insert full file snippet on successful write
                            if !outcome.is_error {
                                let raw_path = call
                                    .arguments
                                    .get("path")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let path = normalize_path(raw_path);
                                let content = call
                                    .arguments
                                    .get("content")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                state
                                    .runtime
                                    .working_set
                                    .add_or_update(path, content, None, None, turn);
                            }
                        }
                        "bash" => {
                            // Track or clear test failures based on test command exit code
                            let cmd = call
                                .arguments
                                .get("command")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let is_test_cmd =
                                is_test_command(cmd, &state.orchestrator.verification_cmd);
                            if outcome.is_error && is_test_cmd {
                                state
                                    .runtime
                                    .working_set
                                    .record_failure_for(cmd, &outcome.content);
                            } else if !outcome.is_error && is_test_cmd {
                                state.runtime.working_set.clear_failure_for(cmd);
                            }
                        }
                        _ => {}
                    }

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
                    // Record rollback in the action ledger to ensure the worker and orchestrator
                    // observe workspace resets explicitly within episodic action history.
                    state.runtime.actions.push(crate::state::ActionEntry {
                        turn,
                        tool: "rollback".into(),
                        target: format!("checkpoint {short_sha}"),
                        summary: "Gate failed; working tree reset to verified state".into(),
                        is_error: true,
                    });
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
        self.emit(Event::Checkpoint {
            history: &history,
            state,
        });
        (Outcome::BudgetExhausted, report)
    }
}

/// Helper to determine if an action target matches a normalized file path.
/// Handles raw paths, leading `./`, line range annotations like `lru.py (L1-L20)`,
/// and byte count annotations like `lru.py (26 bytes)`, even when paths contain parentheses or " (L".
pub fn target_matches_path(target: &str, path: &str) -> bool {
    let mut raw = target;
    if target.ends_with(" bytes)") {
        if let Some(idx) = target.rfind(" (") {
            let byte_str = &target[idx + 2..target.len() - 7];
            if !byte_str.is_empty() && byte_str.chars().all(|c| c.is_ascii_digit()) {
                raw = &target[..idx];
            }
        }
    } else if target.ends_with(')') {
        if let Some(idx) = target.rfind(" (L") {
            // " (L" is 3 bytes, closing ')' is 1 byte at target.len() - 1
            if target.len() >= idx + 5 {
                let inside = &target[idx + 3..target.len() - 1];
                if inside.chars().next().is_some_and(|c| c.is_ascii_digit())
                    && inside
                        .chars()
                        .all(|c| c.is_ascii_digit() || c == '-' || c == '+' || c == 'L')
                {
                    raw = &target[..idx];
                }
            }
        }
    }
    normalize_path(raw) == path || target == path
}

/// True when `call` repeats, verbatim, a call the loop-breaker intercepted on the immediately
/// preceding action. Insisting wins: a worker that re-issues a refused request has decided it
/// needs it, and refusing again only spends the turn budget (FINDINGS 13.22, 13.25).
pub fn is_insist(call: &ToolCall, actions: &[ActionEntry]) -> bool {
    actions.last().is_some_and(|a| {
        a.summary.starts_with("intercepted")
            && a.tool == call.name
            && a.target == extract_action_target(&call.name, &call.arguments)
    })
}

/// Evaluates whether a requested tool call is a redundant file read that should
/// be intercepted by the loop-breaker guard clause.
///
/// Deterministic and zero-latency (Invariant III): operates purely on in-memory
/// working set cache and action history without shelling out or touching disk.
///
/// Interception triggers if:
/// 1. Tool is `read`, and
/// 2. Target path is already cached in `working_set.files`, and
/// 3. The requested line range is fully covered by the cached snippet, and
/// 4. EITHER the immediately preceding action was also a `read` on this path,
///    OR the agent has read this path 2+ times in the last 4 actions without an intervening `edit` or `write` on this file.
pub fn should_intercept_redundant_read(
    call: &ToolCall,
    working_set: &crate::working_set::WorkingSet,
    actions: &[ActionEntry],
) -> Option<String> {
    if call.name != "read" {
        return None;
    }
    let raw_path = call.arguments.get("path").and_then(|v| v.as_str())?;
    let path = normalize_path(raw_path);
    let cached_file = working_set.files.iter().find(|f| f.path == path)?;

    // Parse requested line range
    let req_start = call.arguments.get("start_line").and_then(|v| {
        v.as_u64()
            .map(|n| n as usize)
            .or_else(|| v.as_str().and_then(|s| s.parse::<usize>().ok()))
    });
    let req_end = call.arguments.get("end_line").and_then(|v| {
        v.as_u64()
            .map(|n| n as usize)
            .or_else(|| v.as_str().and_then(|s| s.parse::<usize>().ok()))
    });

    // Check if the requested range is already fully covered by the cached snippet.
    // If the cached content was distilled with middle lines omitted, only an identical
    // request is guaranteed to be redundant; sub-slices might target omitted lines.
    let has_omitted = cached_file.content.contains("lines omitted");
    let is_covered = if has_omitted {
        (req_start, req_end) == (cached_file.start_line, cached_file.end_line)
    } else {
        match (
            cached_file.start_line,
            cached_file.end_line,
            req_start,
            req_end,
        ) {
            // Full file cached without bounds: any slice or full file is covered
            (None, None, _, _) => true,
            // Both cached snippet and request have bounds
            (Some(c_s), Some(c_e), Some(r_s), Some(r_e)) => r_s >= c_s && r_e <= c_e,
            (Some(c_s), None, Some(r_s), Some(_)) => r_s >= c_s,
            (None, Some(c_e), Some(_), Some(r_e)) => r_e <= c_e,
            (Some(c_s), None, Some(r_s), None) => r_s >= c_s,
            (None, Some(c_e), None, Some(r_e)) => r_e <= c_e,
            _ => false,
        }
    };
    if !is_covered {
        return None;
    }

    // Check if the immediately preceding tool call was also a read on this path
    let last_was_same_read = actions
        .last()
        .map(|a| a.tool == "read" && target_matches_path(&a.target, path))
        .unwrap_or(false);

    // Or read this path 2+ times in the last 4 actions without an intervening edit or write to this file
    let file_modified_since = |a: &ActionEntry| {
        (a.tool == "edit" || a.tool == "write") && target_matches_path(&a.target, path)
    };
    let recent_same_reads = actions
        .iter()
        .rev()
        .take(4)
        .take_while(|a| !file_modified_since(a))
        .filter(|a| {
            a.tool == "read"
                && target_matches_path(&a.target, path)
                && !a.summary.starts_with("intercepted")
        })
        .count();

    if last_was_same_read || recent_same_reads >= 2 {
        Some(format!(
            "[Harness Working Memory Guard]: `{path}` is already loaded in your Active Working Set above. \
             You do not need to re-read it. Propose your exact edit using `edit` or update your plan in `note`."
        ))
    } else {
        None
    }
}

/// Evaluates whether a bash inspection command (such as `git diff` or `git status`) is redundant
/// and should be intercepted by the loop-breaker guard clause.
///
/// Prevents the model from entering post-fix amnesiac loops where it repeatedly calls read-only
/// inspection commands instead of concluding its turn.
pub fn should_intercept_redundant_bash(
    call: &ToolCall,
    actions: &[ActionEntry],
    _working_set: &crate::working_set::WorkingSet,
    verification_cmd: &str,
) -> Option<String> {
    if call.name != "bash" {
        return None;
    }
    let raw_cmd = call.arguments.get("command").and_then(|v| v.as_str())?;
    let cmd = raw_cmd.trim();

    // Guard 1: re-running a command that already exited 0, with no edit since, cannot tell the
    // worker anything new. Only the IDENTICAL command counts: a different test command passing
    // says nothing about this one. A target the ledger truncated cannot be compared, so it runs.
    // Truncation is judged by length, not by a trailing `...`: Go's `./...` ends that way too.
    if is_test_command(cmd, verification_cmd) {
        let target = extract_action_target("bash", &call.arguments);
        let truncated = raw_cmd.replace('\n', " ").chars().count() > 60;
        let already_passed = !truncated
            && current_phase_actions(actions)
                .iter()
                .rev()
                .take_while(|a| a.tool != "edit" && a.tool != "write")
                .any(|a| {
                    a.tool == "bash"
                        && a.target == target
                        && !a.is_error
                        && !a.summary.starts_with("intercepted")
                });
        if already_passed {
            return Some(format!(
                "[Harness Working Memory Guard]: `{cmd}` already exited 0 and no file has changed since, \
                 so it would exit 0 again. If your change is complete, stop calling tools to submit it \
                 for verification."
            ));
        }
    }

    let is_readonly_inspection = matches!(
        cmd,
        "git diff" | "git status" | "git status -s" | "git diff --stat" | "git diff HEAD"
    );
    if !is_readonly_inspection {
        return None;
    }

    // Check if the immediately preceding tool call was an identical bash command
    let last_was_same = actions
        .last()
        .map(|a| a.tool == "bash" && a.target.trim() == cmd)
        .unwrap_or(false);

    // Or executed 2+ times in recent actions without an intervening edit or write
    let recent_same = actions
        .iter()
        .rev()
        .take(4)
        .take_while(|a| a.tool != "edit" && a.tool != "write")
        .filter(|a| {
            a.tool == "bash" && a.target.trim() == cmd && !a.summary.starts_with("intercepted")
        })
        .count();

    let verified = verification_passed_since_edit(actions, verification_cmd);
    if last_was_same || recent_same >= 2 || verified {
        let advice = if verified {
            format!(
                "[Harness Working Memory Guard]: `{cmd}` inspection intercepted: the verification \
                 command `{}` exited 0 after your last edit. If your change is complete, stop calling \
                 tools to submit it.",
                verification_cmd.trim()
            )
        } else {
            format!(
                "[Harness Working Memory Guard]: `{cmd}` was already executed. \
                 Review your Active Working Set or Last Test Failure above and apply your fix using `edit`."
            )
        };
        Some(advice)
    } else {
        None
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
    // Check if the tool invocation was intercepted by the working memory guard
    if content.starts_with("[Harness Working Memory Guard]") {
        if tool == "bash" {
            return "intercepted: redundant inspection".to_string();
        }
        return "intercepted: already in working set".to_string();
    }

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

/// Normalizes file paths for working set lookup and tracking.
/// Strips leading `./` and whitespace to prevent path mismatches across tools.
pub fn normalize_path(path: &str) -> &str {
    let p = path.trim();
    p.strip_prefix("./").unwrap_or(p)
}

/// Strips line number prefixes (`{:>6}\t`) generated by the `read` tool.
///
/// Ensures the raw file snippet content is stored in memory so that subsequent
/// `apply_edit` string replacements match accurately across single-line and
/// multi-line blocks, preserving trailing newlines if present.
pub fn strip_line_number_prefixes(content: &str) -> String {
    let mut res = content
        .lines()
        .map(|line| {
            if let Some((num, rest)) = line.split_once('\t') {
                let num_trimmed = num.trim();
                // Ensure num is non-empty digits so lines beginning with tabs are not stripped
                if !num_trimmed.is_empty() && num_trimmed.chars().all(|c| c.is_ascii_digit()) {
                    return rest;
                }
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Preserve trailing newline if present in input content
    if content.ends_with('\n') {
        res.push('\n');
    }
    res
}

/// Determines if a shell command represents a test execution.
///
/// Distinguishes test runners (`cargo test`, `pytest`, `jest`, etc.) and the
/// orchestrator verification command from benign filesystem or VCS operations
/// (e.g. `cat tests/foo`, `ls tests/`, `git checkout test-branch`, `grep "test"`,
/// `npm run build`, `rm -rf tmp || true`).
pub fn is_test_command(cmd: &str, verification_cmd: &str) -> bool {
    let trimmed = cmd.trim();
    let trimmed_verif = verification_cmd.trim();

    // Check exact match of the entire command against verification_cmd first
    // (supporting compound commands like "cargo test && cargo check", while ignoring
    // dummy shell defaults like "true" or "false").
    if !trimmed_verif.is_empty()
        && trimmed_verif != "true"
        && trimmed_verif != "false"
        && trimmed == trimmed_verif
    {
        return true;
    }

    // Split compound commands / pipelines into subcommands
    for part in trimmed.split(&[';', '&', '|'][..]) {
        let part_trimmed = part.trim();
        if part_trimmed.is_empty() {
            continue;
        }

        // Exact match against orchestrator verification command for each subcommand
        // (ignoring trivial shell dummies like "true" or "false" which are defaults in test fixtures)
        if !trimmed_verif.is_empty()
            && trimmed_verif != "true"
            && trimmed_verif != "false"
            && part_trimmed == trimmed_verif
        {
            return true;
        }

        let tokens: Vec<&str> = part_trimmed.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }

        // Skip leading environment variable assignments (e.g. `RUST_BACKTRACE=1 cargo test`)
        let mut idx = 0;
        while idx < tokens.len() && tokens[idx].contains('=') && !tokens[idx].starts_with('-') {
            idx += 1;
        }
        if idx >= tokens.len() {
            continue;
        }

        // Helper to find the first non-flag argument after a given token index
        let find_sub = |start_idx: usize| -> Option<(usize, &str)> {
            let mut i = start_idx;
            while i < tokens.len() && (tokens[i].starts_with('-') || tokens[i].starts_with('+')) {
                i += 1;
            }
            tokens.get(i).copied().map(|s| (i, s))
        };

        // Extract the executable name without directory path (e.g. `./venv/bin/pytest` -> `pytest`)
        let exe = tokens[idx].rsplit('/').next().unwrap_or(tokens[idx]);
        match exe {
            "pytest" | "jest" | "vitest" | "rspec" | "ctest" | "nosetests" | "tox" => return true,
            "cargo" => {
                if let Some((_, sub)) = find_sub(idx + 1) {
                    if sub == "test" || sub == "t" {
                        return true;
                    }
                }
            }
            "go" => {
                if let Some((_, sub)) = find_sub(idx + 1) {
                    if sub == "test" {
                        return true;
                    }
                }
            }
            "npm" | "yarn" | "pnpm" | "bun" => {
                if let Some((sub_idx, sub)) = find_sub(idx + 1) {
                    if sub == "test" || sub == "t" {
                        return true;
                    }
                    if sub == "run" {
                        if let Some((_, run_sub)) = find_sub(sub_idx + 1) {
                            if run_sub == "test" || run_sub == "t" {
                                return true;
                            }
                        }
                    }
                }
            }
            "python" | "python3" => {
                for w in &tokens[idx + 1..] {
                    if *w == "unittest" || *w == "pytest" {
                        return true;
                    }
                }
            }
            "make"
                if tokens[idx + 1..]
                    .iter()
                    .any(|&t| t == "test" || t == "check") =>
            {
                return true;
            }
            _ => {}
        }
    }

    false
}
