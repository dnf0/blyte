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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use blyte_provider::{Message, Provider, ProviderError, Request, Usage};

use crate::context::ContextStrategy;
use crate::fs::Fs;
use crate::gate_slots;
use crate::shell::Shell;
use crate::state::State;
use crate::tools;

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The verification command exited 0. The only way to succeed.
    Verified,
    /// The task completed without a verification gate (e.g. orchestrator task).
    Finished,
    /// Budget spent without the gate passing.
    BudgetExhausted,
    /// The model stopped calling tools and the gate kept failing.
    GateFailed,
    /// The check already fails at the base commit, so the task cannot pass until that is fixed.
    Blocked(String),
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
    /// Wall time of this turn's model call, including any provider retries and their waits.
    #[serde(default)]
    pub model_ms: u64,
    /// Wall time spent executing this turn's tool calls (0 for a turn with none).
    #[serde(default)]
    pub tools_ms: u64,
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
    /// One entry per main-loop model call, in order.
    pub turn_usage: Vec<TurnUsage>,
    /// Wall time of each gate attempt, in order.
    pub gate_ms: Vec<u64>,
    /// The turn on which each gate attempt ran, in order (one entry per gate attempt).
    pub gate_turns: Vec<u32>,
    /// Time spent waiting for a gate slot on each attempt, in order (0 when acquired at once).
    pub gate_wait_ms: Vec<u64>,
    /// Gate output for each failed attempt, bounded to 4,000 characters (keeps the head and the tail).
    pub gate_failures: Vec<String>,
    /// Calls per tool name.
    pub tool_counts: BTreeMap<String, u32>,
    /// Whether the run stopped because spending reached `--max-cost`.
    pub cost_capped: bool,
    /// The model's last non-empty text response (trimmed).
    pub final_message: Option<String>,
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
    /// A gate attempt is starting (the check runs next). Emitted so a watcher can tell a run
    /// that is waiting on its gate from one that is thinking (`blyte dashboard`, 2026-09-30).
    GateStart {
        attempt: u32,
    },
    /// A gate waited for a machine-wide slot before running.
    GateSlotWait {
        ms: u64,
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

/// Retries allowed for one model call before a transient failure ends the run.
pub const MAX_PROVIDER_RETRIES: u32 = 6;

fn parse_resets_in_seconds(body: &str) -> Option<u64> {
    let key = "\"resets_in_seconds\"";
    let pos = body.find(key)?;
    let rest = body[pos + key.len()..].trim_start();
    let after_colon = rest.strip_prefix(':')?.trim_start();
    let digits: String = after_colon
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Identifies provider errors caused by an exhausted usage quota (as opposed to a transient
/// burst rate limit). Returns `Some(message)` describing the exhaustion and reset duration if known.
pub fn quota_exhausted(e: &ProviderError) -> Option<String> {
    let ProviderError::Api { status: 429, body } = e else {
        return None;
    };
    let resets_in_seconds = parse_resets_in_seconds(body);
    let is_quota = body.contains("usage_limit_reached")
        || body.contains("insufficient_quota")
        || body.contains("PerDay")
        || resets_in_seconds.is_some_and(|s| s > 300);

    if !is_quota {
        return None;
    }

    let mut msg = "usage quota exhausted; not retrying".to_string();
    if let Some(secs) = resets_in_seconds {
        let human = if secs >= 86400 {
            format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
        } else if secs >= 3600 {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        } else {
            format!("{}m", secs / 60)
        };
        msg.push_str(&format!(" (resets in {human})"));
    }
    Some(msg)
}

/// Transient provider failures worth waiting out: rate limits, overload, server errors and
/// dropped connections. Anything else (a malformed request, a bad key) fails the run at once.
pub fn is_transient(e: &ProviderError) -> bool {
    if quota_exhausted(e).is_some() {
        return false;
    }
    match e {
        ProviderError::Api { status, .. } => matches!(status, 429 | 500 | 502 | 503 | 504),
        ProviderError::Transport(_) | ProviderError::NoAnswer { .. } => true,
        _ => false,
    }
}

fn max_retries_for(e: &ProviderError) -> u32 {
    match e {
        ProviderError::NoAnswer { .. } => 1,
        _ => MAX_PROVIDER_RETRIES,
    }
}

/// Wait before retry `attempt` (1-based): 2, 4, 8, 16, 32, then 60 seconds.
pub fn retry_delay(attempt: u32) -> Duration {
    if let Ok(ms) = std::env::var("BLYTE_RETRY_DELAY_MS") {
        if let Ok(ms) = ms.trim().parse::<u64>() {
            return Duration::from_millis(ms);
        }
    }
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
    /// Research run: periodically remind the model to keep report.md updated.
    pub research: bool,
}

const CLAIM_PHRASES: &[&str] = &[
    "not caused by",
    "pre-existing",
    "preexisting",
    "already fail",
    "unrelated to",
    "fails on main",
    "fails on the base",
    "was failing before",
];

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
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
    fn system_prompt(&self, state: &State, goal: &str) -> String {
        let bins = tools::available_binaries(self.shell, &self.root).join(", ");
        let where_ = self.shell.describe();
        let rules = crate::rules::prompt_section(&self.root);

        if let Some(text) = &state.orchestrator.instructions {
            return format!(
                "{text}## Your user's first request\n{goal}\n\n\
                 ## Environment\n{where_}\nAvailable: {bins}\n\n\
                 Change nothing outside the repository yourself (blyte's own files under `.wt/` and its records are fine).\n\n\
                 {rules}\
                 ## Working efficiently\n\
                 Independent reads, searches and commands can be issued as several tool calls in one \
                 turn: they run in order and you see every result together. Batch them whenever one \
                 does not depend on another's result; every turn you save is a full model call.\n",
                text = text,
                goal = goal,
                where_ = where_,
                bins = bins,
                rules = rules,
            );
        }

        let steps = gate_steps(&state.orchestrator.verification_cmd);
        // Ungated runs have no check command, so the model must verify the work itself before stopping.
        let completion = if steps.is_empty() {
            "No command checks your work. When you stop, the result is judged by tests you \
             cannot see, so verify it yourself first: run the project's existing test suite and \
             the tests that cover your change, and check the edge cases the task describes. \
             If you searched for problems (with grep or other commands), run those same searches \
             again before you stop and confirm they find nothing left to fix. Stop \
             only when you are confident the task is complete."
                .to_string()
        } else {
            let verify = steps.join(" && ");
            format!(
                "Your work is accepted when `{verify}` exits 0. That command is run for you when \
                 you stop calling tools, and you do not decide the verdict. A failure costs a full \
                 rerun after your fix, so before you stop, run its quick parts yourself \
                 (formatting, lint) together with the tests that cover your change; leave the slow \
                 full run to it. Do not claim to be finished; simply stop when you believe the \
                 command will pass, and you will be told."
            )
        };
        format!(
            "You are a software engineer working in a repository.\n\n\
             ## Goal\n{goal}\n\n\
             ## How completion is decided\n\
             {completion}\n\n\
             ## Environment\n{where_}\nAvailable: {bins}\n\n\
             Change nothing outside the repository: no caches, other repositories, \
             home-directory files or system settings. If something outside it blocks you \
             (disk space, network, a missing tool), say so and stop.\n\n\
             {rules}\
             ## Working efficiently\n\
             Independent reads, searches and commands can be issued as several tool calls in one \
             turn: they run in order and you see every result together. Batch them whenever one \
             does not depend on another's result; every turn you save is a full model call.\n",
            goal = goal,
            completion = completion,
            where_ = where_,
            bins = bins,
            rules = rules,
        )
    }

    /// Run the verification command. Runtime-only: nothing in the tool surface reaches this.
    ///
    /// A gate of several lines is several steps. On the first attempt, every step runs and all
    /// failures come back in one message. On retries, previously failing steps run first, and if
    /// any of them still fails, the attempt stops without re-running the passing steps.
    fn run_gate(&self, state: &State, last_failed: &[String]) -> (bool, String, u64, Vec<String>) {
        let (_guard, wait_ms) = if gate_slots::enabled() && !gate_slots::nested() {
            let max_slots = gate_slots::slots();
            let slot_dir = gate_slots::slot_dir();
            match gate_slots::acquire(max_slots, &slot_dir, Duration::from_millis(500)) {
                Ok((guard, waited)) => {
                    let wait_ms = waited.as_millis() as u64;
                    if wait_ms > 0 {
                        eprintln!(
                            "gate: waited {}s for a gate slot ({max_slots} in use)",
                            waited.as_secs()
                        );
                        self.emit(Event::GateSlotWait { ms: wait_ms });
                    }
                    (Some(guard), wait_ms)
                }
                Err(e) => {
                    eprintln!("gate: slot acquisition failed: {e}");
                    (None, 0)
                }
            }
        } else {
            (None, 0)
        };

        let steps = gate_steps(&state.orchestrator.verification_cmd);
        if steps.len() <= 1 {
            let (passed, output) = self.run_step(&state.orchestrator.verification_cmd);
            let failed = if passed {
                Vec::new()
            } else {
                vec![state.orchestrator.verification_cmd.clone()]
            };
            return (passed, output, wait_ms, failed);
        }
        let (first_group, second_group) = {
            let (f, s) = retry_order(&steps, last_failed);
            if f.is_empty() {
                (steps.clone(), Vec::new())
            } else {
                (f, s)
            }
        };
        let mut failures = Vec::new();
        let mut failed_steps = Vec::new();
        for &step in &first_group {
            let (passed, output) = self.run_step(step);
            if !passed {
                failed_steps.push(step.to_string());
                failures.push(format!("$ {step}\n{output}"));
            }
        }
        if !failures.is_empty() {
            let body = tools::bound_output(&failures.join("\n\n"));
            let not_rerun = if !second_group.is_empty() {
                format!(
                    "\nNot re-run this attempt: {} step(s) that passed last time ({}).",
                    second_group.len(),
                    second_group.join(", ")
                )
            } else {
                String::new()
            };
            return (
                false,
                format!(
                    "{} of {} steps failed: {}{not_rerun}\n\n{body}",
                    failures.len(),
                    steps.len(),
                    failed_steps.join(", ")
                ),
                wait_ms,
                failed_steps,
            );
        }
        for &step in &second_group {
            let (passed, output) = self.run_step(step);
            if !passed {
                failed_steps.push(step.to_string());
                failures.push(format!("$ {step}\n{output}"));
            }
        }
        if failures.is_empty() {
            return (true, "exit 0".into(), wait_ms, Vec::new());
        }
        let body = tools::bound_output(&failures.join("\n\n"));
        (
            false,
            format!(
                "{} of {} steps failed: {}\n\n{body}",
                failures.len(),
                steps.len(),
                failed_steps.join(", ")
            ),
            wait_ms,
            failed_steps,
        )
    }

    /// Run one gate command: exit status and its bounded output.
    fn run_step(&self, cmd: &str) -> (bool, String) {
        self.run_step_in(cmd, &self.root)
    }

    fn run_step_in(&self, cmd: &str, dir: &Path) -> (bool, String) {
        let wrapped = format!("(export {}=1\n{cmd}\n)", gate_slots::IN_GATE_ENV);
        match self.shell.exec(&wrapped, dir, Duration::from_secs(1800)) {
            Ok(o) => {
                let body = tools::bound_output(o.merged().trim_end());
                (o.exit_code == 0, format!("exit {}\n{body}", o.exit_code))
            }
            // A gate that cannot execute has not passed. Treating an error as success is how
            // an ungated run gets mistaken for a verified one.
            Err(e) => (false, format!("verification command could not run: {e}")),
        }
    }

    fn fingerprint(&self) -> Option<String> {
        let head = self
            .shell
            .exec("git rev-parse HEAD", &self.root, Duration::from_secs(30))
            .ok()?;
        if head.exit_code != 0 {
            return None;
        }
        let status = self
            .shell
            .exec(
                "git status --porcelain",
                &self.root,
                Duration::from_secs(30),
            )
            .ok()?;
        if status.exit_code != 0 {
            return None;
        }
        Some(format!("{}\n{}", head.stdout, status.stdout))
    }

    fn check_blocked_at_base(&self, base_commit: &str, failing_steps: &[String]) -> bool {
        if failing_steps.is_empty() {
            return false;
        }

        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = format!(
            "blyte-base-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let wt_path = std::env::temp_dir().join(unique);

        let add_cmd = format!(
            "git worktree add --detach {} {}",
            sh_quote(&wt_path.to_string_lossy()),
            sh_quote(base_commit)
        );
        let add_out = match self
            .shell
            .exec(&add_cmd, &self.root, Duration::from_secs(60))
        {
            Ok(o) if o.exit_code == 0 => o,
            _ => return false,
        };
        let _ = add_out;

        struct Guard<'a> {
            shell: &'a dyn Shell,
            root: &'a Path,
            path: PathBuf,
        }
        impl<'a> Drop for Guard<'a> {
            fn drop(&mut self) {
                let cmd = format!(
                    "git worktree remove --force {}",
                    sh_quote(&self.path.to_string_lossy())
                );
                let _ = self.shell.exec(&cmd, self.root, Duration::from_secs(60));
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }

        let _guard = Guard {
            shell: self.shell,
            root: &self.root,
            path: wt_path.clone(),
        };

        let mut all_failed = true;
        for step in failing_steps {
            let (passed, _) = self.run_step_in(step, &wt_path);
            if passed {
                all_failed = false;
            }
        }
        all_failed
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
        self.run_until(state, deadline)
    }

    fn run_until(
        &self,
        state: &mut State,
        deadline: Option<std::time::Instant>,
    ) -> (Outcome, RunReport) {
        let is_empty_conversation = state
            .runtime
            .conversation
            .as_ref()
            .map(|c| c.is_empty())
            .unwrap_or(false);
        if is_empty_conversation {
            state.runtime.conversation_goal = Some(state.orchestrator.goal.clone());
        }

        let mut history = match &state.runtime.conversation {
            None => vec![Message::User(format!("Begin. {}", state.orchestrator.goal))],
            Some(prev) if prev.is_empty() => {
                vec![Message::User(format!("Begin. {}", state.orchestrator.goal))]
            }
            Some(prev) => {
                let mut h = prev.clone();
                h.push(Message::User(state.orchestrator.goal.clone()));
                h
            }
        };

        let goal = match &state.runtime.conversation_goal {
            Some(g) => g.as_str(),
            None => state.orchestrator.goal.as_str(),
        };
        let system = self.system_prompt(state, goal);

        let (outcome, report) = self.run_loop(state, &mut history, &system, deadline);

        if state.runtime.conversation.is_some() {
            state.runtime.conversation = Some(history);
        }

        (outcome, report)
    }

    fn run_loop(
        &self,
        state: &mut State,
        history: &mut Vec<Message>,
        system: &str,
        deadline: Option<std::time::Instant>,
    ) -> (Outcome, RunReport) {
        let mut deadline = deadline;
        let mut report = RunReport::default();
        let specs = tools::specs();
        let baseline = self.checkpoint();
        let mut last_sent: Option<Vec<Message>> = None;
        let mut last_failed: Vec<String> = Vec::new();
        let mut last_fingerprint: Option<String> = None;
        let mut last_report = if self.research {
            self.fs.read(&self.root.join("report.md")).ok()
        } else {
            None
        };

        for turn in 1..=self.budget.max_turns {
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                return (Outcome::BudgetExhausted, report);
            }
            if let Some(cap) = state.orchestrator.max_cost_usd {
                if let Some(price) = blyte_provider::pricing(&self.provider.model_id()) {
                    if price.cost(&report.usage) >= cap {
                        report.cost_capped = true;
                        return (Outcome::BudgetExhausted, report);
                    }
                }
            }
            report.outcome_turns = turn;

            if self.budget.max_turns >= 20 && turn == self.budget.max_turns - 9 {
                history.push(Message::User(format!(
                    "You have 10 turns left of {}. Wrap up now: make sure your work is saved before you run out: committed, or written to the file the task asks for. Then stop calling tools.",
                    self.budget.max_turns
                )));
            }

            if self.research && turn % 30 == 0 {
                let current_report = self.fs.read(&self.root.join("report.md")).ok();
                if current_report == last_report {
                    history.push(Message::User(
                        "Update report.md now with what you have found so far, even if unfinished, then carry on.".to_string(),
                    ));
                }
                last_report = current_report;
            }

            let req = Request {
                system: system.to_string(),
                messages: self.context.build(state, history),
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
            let fold_budget = self.context.fold_budget(history);
            last_sent = Some(req.messages.clone());
            // A burst rate limit ended a run after 28 calls (2026-09-23). Transient failures are
            // waited out, but never past the deadline: a run that cannot finish in time reports.
            let model_start = std::time::Instant::now();
            let mut attempt = 0;
            let resp = loop {
                let call_start = std::time::Instant::now();
                match self.provider.complete(&req) {
                    Ok(r) => break r,
                    Err(e) => {
                        if let ProviderError::NoAnswer { usage, .. } = &e {
                            report.usage.input += usage.input;
                            report.usage.cached += usage.cached;
                            report.usage.output += usage.output;
                            report.usage.thinking += usage.thinking;
                        }
                        let max_retries = max_retries_for(&e);
                        if is_transient(&e) && attempt < max_retries {
                            let elapsed = call_start.elapsed().as_secs_f64().round() as u64;
                            let s = if elapsed > 0 {
                                elapsed
                            } else {
                                blyte_provider::model_timeout_secs()
                            };
                            attempt += 1;
                            let wait = retry_wait(&e, attempt);
                            if deadline.is_some_and(|d| std::time::Instant::now() + wait >= d) {
                                return (Outcome::Error(e.to_string()), report);
                            }
                            report.provider_retries += 1;
                            if e.is_timeout() {
                                eprintln!(
                                    "model call timed out after {s} s; retrying ({attempt} of {max_retries})"
                                );
                            } else {
                                eprintln!(
                                    "provider: {e}; retrying in {}s ({attempt}/{max_retries})",
                                    wait.as_secs()
                                );
                            }
                            std::thread::sleep(wait);
                        } else {
                            let err_msg = match quota_exhausted(&e) {
                                Some(msg) => format!("{msg}: {e}"),
                                None => e.to_string(),
                            };
                            return (Outcome::Error(err_msg), report);
                        }
                    }
                }
            };
            let model_ms = model_start.elapsed().as_millis() as u64;
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
                model_ms,
                tools_ms: 0,
            });

            let trimmed = resp.text.trim();
            if trimmed.is_empty() && resp.tool_calls.is_empty() {
                report.empty_turns += 1;
            }
            self.emit(Event::Turn {
                n: turn,
                text: trimmed,
            });
            if !trimmed.is_empty() {
                report.final_message = Some(trimmed.to_string());
                history.push(Message::Assistant(resp.text.clone()));
            }

            // --- the model is still working -------------------------------------------------
            if !resp.tool_calls.is_empty() {
                let tools_start = std::time::Instant::now();
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
                    *report.tool_counts.entry(call.name.clone()).or_default() += 1;

                    let outcome = tools::execute(
                        &self.root,
                        self.shell,
                        self.fs,
                        &call.name,
                        &call.arguments,
                    );

                    let summary = outcome
                        .content
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(90)
                        .collect::<String>();
                    // A read's line range is part of what it did: paging through a file and
                    // re-reading it look identical without it (blyte-eval 13.43).
                    let input = if call.name == "read" {
                        extract_action_target(&call.name, &call.arguments)
                            .chars()
                            .take(100)
                            .collect()
                    } else {
                        let clean = |s: &str| s.trim_end().replace('\n', " ↵ ");
                        let text = call
                            .arguments
                            .get("command")
                            .or_else(|| call.arguments.get("pattern"))
                            .or_else(|| call.arguments.get("path"))
                            .and_then(|v| v.as_str())
                            .map(clean)
                            .unwrap_or_else(|| clean(&call.arguments.to_string()));
                        if text.chars().count() > 2000 {
                            let mut cut: String = text.chars().take(2000).collect();
                            cut.push('…');
                            cut
                        } else {
                            text
                        }
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
                report.turn_usage.last_mut().unwrap().tools_ms =
                    tools_start.elapsed().as_millis() as u64;
                continue;
            }

            // --- the model stopped calling tools: the runtime decides -----------------------
            if report.gate_attempts > 0
                && CLAIM_PHRASES
                    .iter()
                    .any(|&p| resp.text.to_lowercase().contains(p))
            {
                if let (Some(last_fp), Some(curr_fp), Some(base_commit)) =
                    (&last_fingerprint, self.fingerprint(), &baseline)
                {
                    if *last_fp == curr_fp && self.check_blocked_at_base(base_commit, &last_failed)
                    {
                        let steps = last_failed.join("; ");
                        let text: String = resp.text.chars().take(300).collect();
                        return (
                            Outcome::Blocked(format!(
                                "the check fails at the base commit too ({steps}); the model said: {text}"
                            )),
                            report,
                        );
                    }
                }
            }

            if state.orchestrator.verification_cmd.trim().is_empty() {
                if state.orchestrator.instructions.is_some() {
                    return (Outcome::Finished, report);
                }
                return (Outcome::Verified, report);
            }

            report.gate_attempts += 1;
            report.gate_turns.push(turn);
            self.emit(Event::GateStart {
                attempt: report.gate_attempts,
            });
            let gate_start = std::time::Instant::now();
            let (passed, output, wait_ms, failing) = self.run_gate(state, &last_failed);
            last_failed = failing;
            report.gate_ms.push(gate_start.elapsed().as_millis() as u64);
            report.gate_wait_ms.push(wait_ms);
            if let Some(d) = deadline.as_mut() {
                *d += Duration::from_millis(wait_ms);
            }
            self.emit(Event::Gate {
                attempt: report.gate_attempts,
                passed,
                output: output.lines().next().unwrap_or(""),
            });
            if passed {
                return (Outcome::Verified, report);
            }
            report.gate_failures.push(cut_gate_output(&output));

            last_fingerprint = self.fingerprint();

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
                "Verification did not pass.\n\n{output}\n\n\
                 If the failure is caused by your change, fix your change, not the machine. \
                 If it is not caused by your change (disk space, network, a missing service), \
                 say so and stop.{reset_note}"
            )));
        }
        (Outcome::BudgetExhausted, report)
    }
}

/// The steps of a gate: its non-blank lines, trimmed.
fn gate_steps(gate: &str) -> Vec<&str> {
    gate.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Splits `steps` into (the steps whose text is in `last_failed`, in gate order; the
/// others, in gate order). With an empty `last_failed`, returns (all steps, nothing).
pub fn retry_order<'a>(steps: &[&'a str], last_failed: &[String]) -> (Vec<&'a str>, Vec<&'a str>) {
    if last_failed.is_empty() {
        return (steps.to_vec(), Vec::new());
    }
    let mut failed = Vec::new();
    let mut others = Vec::new();
    for &step in steps {
        if last_failed.iter().any(|f| f == step) {
            failed.push(step);
        } else {
            others.push(step);
        }
    }
    (failed, others)
}

/// Summary lines from test runners (cargo, pytest, rustc).
fn is_summary_line(line: &str) -> bool {
    let t = line.trim();
    ((t.starts_with("FAILED ") || t.starts_with("ERROR "))
        && (t.contains("::") || t.contains(".py")))
        || (t.starts_with('=') && (t.contains(" failed") || t.contains(" error")))
        || (t.starts_with("test ") && t.ends_with("FAILED"))
        || t.starts_with("error[")
        || t.starts_with("error:")
        || t.starts_with("test result: FAILED")
}

/// Collects up to 40 unique summary lines in order of appearance.
fn summary_lines(output: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in output.lines() {
        let t = line.trim();
        if is_summary_line(t) && seen.insert(t) {
            lines.push(t);
            if lines.len() == 40 {
                break;
            }
        }
    }
    lines
}

/// Checks if a line matches blyte's own "N of M steps failed: …" header format.
fn is_blyte_header(line: &str) -> bool {
    let t = line.trim();
    let Some((n, rest)) = t.split_once(" of ") else {
        return false;
    };
    if !n.chars().all(|c| c.is_ascii_digit()) || n.is_empty() {
        return false;
    }
    let Some((m, _)) = rest.split_once(" steps failed:") else {
        return false;
    };
    m.chars().all(|c| c.is_ascii_digit()) && !m.is_empty()
}

/// Cuts gate output longer than 4,000 characters, keeping test runner summary lines,
/// the header, and the head and tail.
pub fn cut_gate_output(output: &str) -> String {
    const MAX_CHARS: usize = 4_000;

    let total_chars = output.chars().count();
    if total_chars <= MAX_CHARS {
        return output.to_string();
    }

    let sum_lines = summary_lines(output);
    let has_summary = !sum_lines.is_empty();

    let first_line = output.lines().next().unwrap_or("");
    let has_blyte_header = is_blyte_header(first_line);

    let (header, rem) = if has_summary && has_blyte_header {
        let rem = &output[first_line.len()..];
        let rem = rem.trim_start_matches(['\r', '\n']);
        (Some(first_line), rem)
    } else {
        (None, output)
    };

    let summary_block = if has_summary {
        let mut block = String::from("Failing, from the full output:\n");
        block.push_str(&sum_lines.join("\n"));
        Some(block)
    } else {
        None
    };

    let prefix_chars = header.map(|h| h.chars().count() + 1).unwrap_or(0)
        + summary_block
            .as_ref()
            .map(|s| s.chars().count() + 1)
            .unwrap_or(0);

    let available = MAX_CHARS.saturating_sub(prefix_chars);
    let head_budget = available / 4;
    let tail_budget = available - head_budget;

    let head = if head_budget == 0 || rem.is_empty() {
        ""
    } else {
        let head_cand_end = rem
            .char_indices()
            .nth(head_budget)
            .map(|(i, _)| i)
            .unwrap_or(rem.len());
        let head_cand = &rem[..head_cand_end];
        match head_cand.rfind('\n') {
            Some(idx) => &head_cand[..idx],
            None => head_cand,
        }
    };

    let tail = if tail_budget == 0 {
        ""
    } else {
        let tail_cand_start = output
            .char_indices()
            .nth(total_chars.saturating_sub(tail_budget))
            .map(|(i, _)| i)
            .unwrap_or(output.len());
        let tail_cand = &output[tail_cand_start..];
        match tail_cand.find('\n') {
            Some(idx) => &tail_cand[idx + 1..],
            None => tail_cand,
        }
    };

    let dropped = total_chars.saturating_sub(head.chars().count() + tail.chars().count());

    let mut pieces = Vec::new();
    if let Some(h) = header {
        pieces.push(h);
    }
    if let Some(sb) = &summary_block {
        pieces.push(sb.as_str());
    }
    if !head.is_empty() {
        pieces.push(head);
    }
    let marker = format!("… [{dropped} characters cut] …");
    pieces.push(&marker);
    if !tail.is_empty() {
        pieces.push(tail);
    }
    pieces.join("\n")
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
        _ => {
            // Fallback for custom or unknown tools clamps the first line to 80 chars.
            let first = content.lines().next().unwrap_or("");
            first.chars().take(80).collect()
        }
    }
}
