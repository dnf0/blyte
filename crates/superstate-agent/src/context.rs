//! How the next prompt is assembled. A seam, because the right answer is not yet known.
//!
//! [`Conversational`] is what every mainstream harness does: append every turn and re-send the
//! lot. Its cost grows with the square of the run -- each call re-sends everything before it --
//! and EXP-081 shows where that lands: a single turn costing 1,384,188 fresh input tokens
//! against 15,327,627 cached (FINDINGS 13.29).
//!
//! [`StateOnly`] is the SKILL.state model: the model sees the immutable spec, the structured
//! state, and the latest observation -- `(P, Σ, O)` -- and its reasoning is discarded rather
//! than accumulated. The paper reports 16.2x fewer tokens at equal accuracy.
//!
//! Both ship, and neither is the default by fiat, because the paper's evidence is from
//! synthetic warehouse tasks and InterCode CTF. Its own first limitation is that a fixed schema
//! must be known in advance, and nobody knows what that is for a repository; its second is an
//! observation whose relevance is only recognised later, which is the daily texture of
//! debugging and absent from its benchmarks. Shipping it as *the* architecture would repeat
//! this project's most expensive mistake -- committing to a mechanism before measuring it, as
//! the read cache was (13.25). Behind a trait it is an experiment instead.

use superstate_provider::Message;

use crate::state::State;

pub trait ContextStrategy: Send + Sync {
    /// Build the messages for the next call from the run so far.
    fn build(&self, state: &State, history: &[Message]) -> Vec<Message>;
    fn name(&self) -> &'static str;
}

/// Everything, every time. The baseline to beat.
#[derive(Debug, Default, Clone)]
pub struct Conversational;

impl ContextStrategy for Conversational {
    fn build(&self, _state: &State, history: &[Message]) -> Vec<Message> {
        history.to_vec()
    }
    fn name(&self) -> &'static str {
        "conversational"
    }
}

/// `(P, Σ, O)`: the task, the state, and the most recent observation.
///
/// The model's prior reasoning is not carried. What it wants to remember it must write to its
/// own state region with `note` -- which is the mechanism under test, since the previous
/// harness offered a state file the agent never wrote to (FINDINGS 13.30), having never been
/// asked.
#[derive(Debug, Default, Clone)]
pub struct StateOnly;

impl ContextStrategy for StateOnly {
    fn build(&self, state: &State, history: &[Message]) -> Vec<Message> {
        let mut out = Vec::new();
        // Order is for the cache as much as for the reader. Gemini caches a shared prompt prefix
        // of at least 4,096 tokens, so what is stable goes first: the task, then the append-only
        // action ledger, which turn N+1 repeats exactly with one more line. Everything that
        // changes turn to turn (notes, working set, failure, verification status) follows it.
        let mut user_prompt = format!("## Task\n{}\n", state.orchestrator.goal);

        // The chronological action ledger: episodic memory of tool outcomes without growing
        // conversation history. Append-only, which is what makes it cacheable.
        if !state.runtime.actions.is_empty() {
            user_prompt.push_str("\n## Actions Taken So Far\n");
            for action in &state.runtime.actions {
                let status_indicator = if action.is_error { " [ERROR]" } else { "" };
                user_prompt.push_str(&format!(
                    "- Turn {}: `{}` `{}` -> {}{}\n",
                    action.turn, action.tool, action.target, action.summary, status_indicator
                ));
            }
        }

        user_prompt.push_str(&format!(
            "\n## Your state\n{}\n",
            serde_json::to_string_pretty(&state.agent.fields).unwrap_or_default(),
        ));

        // External working memory: active file snippets inspected or edited by the worker.
        // Injects distilled snippets so the agent has continuous visibility across turns without
        // re-reading unchanged files or bloating conversation history.
        if !state.runtime.working_set.files.is_empty() {
            user_prompt.push_str("\n## Active Working Set\n");
            for file in &state.runtime.working_set.files {
                // Format line range indicator when partial file slice is cached
                let lines_info = match (file.start_line, file.end_line) {
                    (Some(s), Some(e)) => format!(" (lines {s}-{e})"),
                    _ => String::new(),
                };
                user_prompt.push_str(&format!(
                    "### `{}`{}\n```\n{}\n```\n",
                    file.path,
                    lines_info,
                    file.content.trim_end()
                ));
            }
        }

        // Grounding telemetry: distilled output from the most recent failed test or command.
        // Provides the failure trace directly in context so the worker does not have to re-run
        // tests just to recall error lines.
        if let Some(failure) = &state.runtime.working_set.last_failure {
            user_prompt.push_str(&format!(
                "\n## Last Test Failure\n```text\n{}\n```\n",
                failure.trim_end()
            ));
        } else {
            // Only the verification command itself exiting 0 predicts the gate. Claiming "tests
            // are passing" from any test-like command told a GREEN worker its failing test passed.
            if crate::run::verification_passed_since_edit(
                &state.runtime.actions,
                &state.orchestrator.verification_cmd,
            ) {
                user_prompt.push_str(&format!(
                    "\n## Verification Status\nThe verification command `{}` exited 0 after your \
                     last edit. If your change is complete, stop calling tools to submit it.\n",
                    state.orchestrator.verification_cmd.trim()
                ));
            }
        }

        out.push(Message::User(user_prompt));

        // The latest observation, and only that. A tool result the model has already acted on
        // is not re-sent; if it mattered, it belonged in state.
        //
        // The tool CALL is carried alongside its result because a result with no call is
        // meaningless -- and because providers reject a functionResponse with no matching
        // functionCall.
        if let Some(pos) = history
            .iter()
            .rposition(|m| matches!(m, Message::ToolResult { .. }))
        {
            if pos > 0 {
                if let Message::ToolCall { .. } = &history[pos - 1] {
                    out.push(history[pos - 1].clone());
                }
            }
            out.push(history[pos].clone());
        } else if let Some(last) = history.last() {
            out.push(last.clone());
        }
        out
    }
    fn name(&self) -> &'static str {
        "state-only"
    }
}

/// `state-only` plus the last few tool results.
///
/// `StateOnly` sends only the latest tool call and result, on the theory that anything worth
/// keeping belongs in state. In practice the worker rarely writes state, so every search result
/// lived for one turn and was run again: the same grep 14 times on build-cython-ext, six files
/// re-read 101 times on scc (superstate-eval FINDINGS 13.33). This keeps recent results in view
/// and stays bounded in session length: at most `1 + 2 * results + 1` messages.
#[derive(Debug, Clone)]
pub struct StateWindow {
    /// How many of the most recent tool call/result pairs to carry.
    pub results: usize,
    /// Each carried result is cut to this many lines with `tools::distil`.
    pub lines_per_result: usize,
}

impl Default for StateWindow {
    fn default() -> Self {
        Self {
            results: 8,
            lines_per_result: 60,
        }
    }
}

impl ContextStrategy for StateWindow {
    fn build(&self, state: &State, history: &[Message]) -> Vec<Message> {
        // The state prompt, exactly as `state-only` builds it with nothing carried.
        let mut out = StateOnly.build(state, &[]);

        // Walk back collecting call/result pairs. The call is carried with its result because
        // providers reject a functionResponse with no matching functionCall.
        let mut pairs: Vec<(Message, Message)> = Vec::new();
        for i in (1..history.len()).rev() {
            if pairs.len() == self.results {
                break;
            }
            if let (
                Message::ToolCall { .. },
                Message::ToolResult {
                    id,
                    name,
                    content,
                    is_error,
                },
            ) = (&history[i - 1], &history[i])
            {
                pairs.push((
                    history[i - 1].clone(),
                    Message::ToolResult {
                        id: id.clone(),
                        name: name.clone(),
                        content: crate::tools::distil(content, self.lines_per_result),
                        is_error: *is_error,
                    },
                ));
            }
        }
        for (call, result) in pairs.into_iter().rev() {
            out.push(call);
            out.push(result);
        }

        // Whatever came after the last result (gate feedback, or the opening message on the
        // first turn) is the newest thing the worker must see.
        if let Some(last) = history.last() {
            if !matches!(last, Message::ToolResult { .. }) {
                out.push(last.clone());
            }
        }
        out
    }

    fn name(&self) -> &'static str {
        "state-window"
    }
}
