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
        Self {
            max_turns: 60,
            max_gate_attempts: 5,
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
        match self.shell.exec(
            &state.orchestrator.verification_cmd,
            &self.root,
            Duration::from_secs(1800),
        ) {
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
        let mut report = RunReport::default();
        let mut history: Vec<Message> =
            vec![Message::User(format!("Begin. {}", state.orchestrator.goal))];
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
            self.emit(Event::Turn {
                n: turn,
                text: resp.text.trim(),
            });
            if !resp.text.trim().is_empty() {
                history.push(Message::Assistant(resp.text.clone()));
            }

            // --- the model is still working -------------------------------------------------
            if !resp.tool_calls.is_empty() {
                for call in &resp.tool_calls {
                    report.tool_calls += 1;
                    history.push(Message::ToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        signature: call.signature.clone(),
                    });

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
                            // Track active file snippet on successful read, stripping line numbers
                            if !outcome.is_error {
                                let raw_path = call
                                    .arguments
                                    .get("path")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let path = normalize_path(raw_path);
                                let raw_content = strip_line_number_prefixes(&outcome.content);
                                let start = call.arguments.get("start_line").and_then(|v| {
                                    v.as_u64()
                                        .map(|n| n as usize)
                                        .or_else(|| v.as_str().and_then(|s| s.parse::<usize>().ok()))
                                });
                                let end = call.arguments.get("end_line").and_then(|v| {
                                    v.as_u64()
                                        .map(|n| n as usize)
                                        .or_else(|| v.as_str().and_then(|s| s.parse::<usize>().ok()))
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
                                state.runtime.working_set.add_or_update(
                                    path,
                                    content,
                                    None,
                                    None,
                                    turn,
                                );
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
                                state.runtime.working_set.record_failure(&outcome.content);
                            } else if !outcome.is_error && is_test_cmd {
                                state.runtime.working_set.clear_failure();
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

    // Preserve trailing newline to match write and support edits at EOF
    if !res.is_empty() && !res.ends_with('\n') {
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
            "make" if tokens[idx + 1..].iter().any(|&t| t == "test" || t == "check") => {
                return true;
            }
            _ => {}
        }
    }

    false
}

