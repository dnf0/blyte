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

/// Start of the turn block ending at `pos` (a `ToolResult`): the run of results ending there,
/// preceded by the run of calls that produced them.
fn block_start(history: &[Message], pos: usize) -> usize {
    let mut start = pos;
    while start > 0 && matches!(history[start - 1], Message::ToolResult { .. }) {
        start -= 1;
    }
    while start > 0 && matches!(history[start - 1], Message::ToolCall { .. }) {
        start -= 1;
    }
    start
}

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
            // The whole last turn block: its calls, then its results.
            let start = block_start(history, pos);
            out.extend_from_slice(&history[start..=pos]);
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

        // Walk back collecting whole turn blocks (a turn's calls, then its results) until at
        // least `results` results are carried. A block is never split: a result without its
        // call, or a call without its result, is rejected by providers.
        let mut blocks: Vec<Vec<Message>> = Vec::new();
        let mut carried = 0;
        let mut end = history.len();
        while carried < self.results {
            let Some(pos) = history[..end]
                .iter()
                .rposition(|m| matches!(m, Message::ToolResult { .. }))
            else {
                break;
            };
            let start = block_start(history, pos);
            if !matches!(history[start], Message::ToolCall { .. }) {
                break;
            }
            let block: Vec<Message> = history[start..=pos]
                .iter()
                .map(|m| match m {
                    Message::ToolResult {
                        id,
                        name,
                        content,
                        is_error,
                    } => Message::ToolResult {
                        id: id.clone(),
                        name: name.clone(),
                        content: crate::tools::distil(content, self.lines_per_result),
                        is_error: *is_error,
                    },
                    other => other.clone(),
                })
                .collect();
            carried += block
                .iter()
                .filter(|m| matches!(m, Message::ToolResult { .. }))
                .count();
            blocks.push(block);
            end = start;
        }
        for block in blocks.into_iter().rev() {
            out.extend(block);
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

/// An append-only conversation, bounded by folding.
///
/// `Conversational` keeps everything, which is what Gemini expects ("You MUST always resend all
/// `thought` blocks exactly as they were received") and what caches well, but it grows without
/// limit. The bounded strategies rebuild the prompt every turn, which drops thought signatures,
/// caches badly, and in every eval run lost the thread (superstate-eval FINDINGS 13.33-13.35).
///
/// `Folded` sends `[opening] [digest 1] … [digest k] [verbatim tail]`. Once the verbatim part
/// passes `budget_tokens`, its older half becomes one harness-written digest. Boundaries are
/// computed greedily from the start of history, so each depends only on the messages before it:
/// the next turn reproduces every earlier fold exactly, and between folds the output is pure
/// append. A cut never separates a tool call from its result.
#[derive(Debug, Clone)]
pub struct Folded {
    /// Approximate token budget (characters / 4) for the verbatim tail.
    pub budget_tokens: usize,
}

impl Default for Folded {
    fn default() -> Self {
        Self {
            budget_tokens: 48_000,
        }
    }
}

/// Characters / 4: a budget, not a billing figure.
fn approx_tokens(m: &Message) -> usize {
    let chars = match m {
        Message::User(t) | Message::Assistant(t) => t.len(),
        Message::ToolCall { arguments, .. } => arguments.to_string().len(),
        Message::ToolResult { content, .. } => content.len(),
    };
    chars / 4
}

fn clip(text: &str, max: usize) -> String {
    let one_line = text.lines().next().unwrap_or("").trim();
    if one_line.chars().count() > max {
        format!("{}...", one_line.chars().take(max).collect::<String>())
    } else {
        one_line.to_string()
    }
}

/// One harness-written line per folded item, from the same summarisers as the action ledger.
/// Calls are paired with results by position within their turn block (calls, then results, in
/// call order). Not by id: Gemini issues no ids, the adapter numbers calls from `call_0` every
/// turn, so ids repeat across turns and an id match would pair a call with another turn's result.
fn digest(folded: &[Message], first: usize, last: usize) -> Message {
    let mut text =
        format!("## Earlier turns, folded (messages {first}-{last}; summarised by the harness)\n");
    let mut i = 0;
    while i < folded.len() {
        match &folded[i] {
            Message::ToolCall { .. } => {
                let calls_end = (i..folded.len())
                    .find(|&j| !matches!(folded[j], Message::ToolCall { .. }))
                    .unwrap_or(folded.len());
                let results_end = (calls_end..folded.len())
                    .find(|&j| !matches!(folded[j], Message::ToolResult { .. }))
                    .unwrap_or(folded.len());
                for (k, call) in folded[i..calls_end].iter().enumerate() {
                    let Message::ToolCall {
                        name, arguments, ..
                    } = call
                    else {
                        continue;
                    };
                    let target = crate::run::extract_action_target(name, arguments);
                    let summary = match folded.get(calls_end + k) {
                        Some(Message::ToolResult {
                            content, is_error, ..
                        }) if calls_end + k < results_end => {
                            crate::run::extract_action_summary(name, content, *is_error)
                        }
                        _ => "no result".to_string(),
                    };
                    text.push_str(&format!("- `{name}` `{target}` -> {summary}\n"));
                }
                i = results_end.max(i + 1);
            }
            Message::Assistant(t) => {
                text.push_str(&format!("- you said: {}\n", clip(t, 160)));
                i += 1;
            }
            Message::User(t) => {
                text.push_str(&format!("- harness: {}\n", clip(t, 160)));
                i += 1;
            }
            Message::ToolResult { .. } => i += 1,
        }
    }
    Message::User(text)
}

/// Where `Folded` put its boundaries over `history[1..]`: each fold `(a, b)`, the start of the
/// verbatim tail, and the index at which the most recent fold was triggered (if any).
struct FoldPlan {
    folds: Vec<(usize, usize)>,
    seg_start: usize,
    last_trigger: Option<usize>,
}

impl Folded {
    /// Greedy boundaries from the start: each fold depends only on messages before it, so the next
    /// turn reproduces every earlier fold exactly.
    fn plan(&self, rest: &[Message]) -> FoldPlan {
        let mut folds: Vec<(usize, usize)> = Vec::new();
        let mut seg_start = 0;
        let mut size = 0;
        let mut last_trigger = None;
        for i in 0..rest.len() {
            size += approx_tokens(&rest[i]);
            if size <= self.budget_tokens {
                continue;
            }
            // Fold roughly the older half, cutting only between turn blocks (never after a call
            // or before a result), so a call is never separated from its result or from calls in
            // the same batch. No safe cut yet: keep accumulating.
            let target = seg_start + (i + 1 - seg_start) / 2;
            let lo = target.max(seg_start + 1);
            let Some(cut) = (lo..=i).find(|&j| {
                !matches!(rest[j], Message::ToolResult { .. })
                    && !matches!(rest[j - 1], Message::ToolCall { .. })
            }) else {
                continue;
            };
            folds.push((seg_start, cut));
            size = rest[cut..=i].iter().map(approx_tokens).sum();
            seg_start = cut;
            last_trigger = Some(i);
        }
        FoldPlan {
            folds,
            seg_start,
            last_trigger,
        }
    }
}

impl ContextStrategy for Folded {
    fn build(&self, _state: &State, history: &[Message]) -> Vec<Message> {
        let Some((opening, rest)) = history.split_first() else {
            return Vec::new();
        };
        let plan = self.plan(rest);
        let mut out = vec![opening.clone()];
        for (a, b) in plan.folds {
            // Message numbers are 1-based positions in the full history.
            out.push(digest(&rest[a..b], a + 2, b + 1));
        }
        out.extend_from_slice(&rest[plan.seg_start..]);
        out
    }

    fn name(&self) -> &'static str {
        "folded"
    }
}

/// `folded`, plus two exact reductions of tool output that only the harness can make safely,
/// because it observes every action.
///
/// Re-reading history is most of a long run's real cost (16–20M cached input tokens on
/// build-cython-ext), and tool outputs dominate history. Hiding old outputs while keeping the calls
/// halved cost at no loss of solve rate on SWE-bench Verified (JetBrains, "The Complexity Trap"),
/// but hiding by age can drop something still relevant. The harness knows which outputs are
/// **superseded**: a read of a file edited since, or a call repeated since with identical
/// arguments. Those are masked; outputs identical to one still visible are referenced. Masks are
/// applied only when a fold happens, judged by calls up to that fold's trigger: a fold already
/// rewrites the prompt from its digest on, so masking then costs no extra cache miss (v1's
/// per-epoch masks re-prefilled the tail at the fresh price and saved nothing; superstate-eval
/// FINDINGS 13.39). Calls are never removed, so no result is ever orphaned.
#[derive(Debug, Clone)]
pub struct Compact {
    /// Approximate token budget for the verbatim tail, as in `Folded`.
    pub budget_tokens: usize,
    /// Outputs shorter than this are never replaced by a duplicate reference.
    pub min_dedup_chars: usize,
}

impl Default for Compact {
    fn default() -> Self {
        Self {
            budget_tokens: 48_000,
            min_dedup_chars: 200,
        }
    }
}

/// The call that produced the result at `r`: calls and results of a turn are recorded as a block
/// (calls, then results, in call order), so the k-th result of a run pairs with the k-th call of
/// the run before it. Also correct for the older interleaved layout (a block of one).
fn call_for_result(h: &[Message], r: usize) -> Option<&Message> {
    let mut results_start = r;
    while results_start > 0 && matches!(h[results_start - 1], Message::ToolResult { .. }) {
        results_start -= 1;
    }
    let k = r - results_start;
    let mut calls_start = results_start;
    while calls_start > 0 && matches!(h[calls_start - 1], Message::ToolCall { .. }) {
        calls_start -= 1;
    }
    h[calls_start..results_start].get(k)
}

/// Why the output at `r` (produced by `name(args)`) is superseded by a later call before `cutoff`.
fn superseded(
    h: &[Message],
    r: usize,
    cutoff: usize,
    name: &str,
    args: &serde_json::Value,
) -> Option<String> {
    let path = args.get("path").and_then(|v| v.as_str());
    for m in &h[r + 1..cutoff] {
        let Message::ToolCall {
            name: later,
            arguments: later_args,
            ..
        } = m
        else {
            continue;
        };
        let later_path = later_args.get("path").and_then(|v| v.as_str());
        match (name, later.as_str()) {
            ("read", "edit" | "write") if path.is_some() && path == later_path => {
                return Some(format!(
                    "superseded: {} changed later",
                    path.unwrap_or_default()
                ));
            }
            ("read", "read") if args == later_args => {
                return Some(format!(
                    "superseded by a later identical read of {}",
                    path.unwrap_or_default()
                ));
            }
            (n, l)
                if n == l
                    && !matches!(n, "read" | "edit" | "write" | "note")
                    && args == later_args =>
            {
                return Some("superseded by a later run of the same call".to_string());
            }
            _ => {}
        }
    }
    None
}

impl ContextStrategy for Compact {
    fn build(&self, state: &State, history: &[Message]) -> Vec<Message> {
        let folded = Folded {
            budget_tokens: self.budget_tokens,
        };
        let mut out = folded.build(state, history);
        let Some((_, rest)) = history.split_first() else {
            return out;
        };
        let plan = folded.plan(rest);
        let tail_out = 1 + plan.folds.len();
        let tail_start = 1 + plan.seg_start;
        // Supersession is judged only by calls up to the most recent fold's trigger. Between folds
        // that cutoff is fixed, so the prompt is pure append; before the first fold nothing masks.
        let cutoff = plan
            .last_trigger
            .map_or(tail_start, |t| (t + 2).min(history.len()));

        // Superseded outputs, judged only by calls up to the cutoff.
        let mut masked = vec![false; history.len()];
        for r in tail_start..cutoff {
            if !matches!(history[r], Message::ToolResult { .. }) {
                continue;
            }
            let Some(Message::ToolCall {
                name, arguments, ..
            }) = call_for_result(history, r)
            else {
                continue;
            };
            if let Some(reason) = superseded(history, r, cutoff, name, arguments) {
                if let Message::ToolResult { content, .. } = &mut out[tail_out + (r - tail_start)] {
                    *content = format!("[{reason}; output elided by superstate]");
                }
                masked[r] = true;
            }
        }

        // Exact duplicates of an output still visible above.
        for r in tail_start..history.len() {
            let Message::ToolResult { content: here, .. } = &history[r] else {
                continue;
            };
            if masked[r] || here.len() < self.min_dedup_chars {
                continue;
            }
            let earlier = (tail_start..r).find(|&q| {
                !masked[q]
                    && matches!(&history[q], Message::ToolResult { content, .. } if content == here)
            });
            let Some(q) = earlier else {
                continue;
            };
            let what = match call_for_result(history, q) {
                Some(Message::ToolCall {
                    name, arguments, ..
                }) => format!(
                    "`{name}` `{}`",
                    crate::run::extract_action_target(name, arguments)
                ),
                _ => "an earlier call".to_string(),
            };
            if let Message::ToolResult { content, .. } = &mut out[tail_out + (r - tail_start)] {
                *content = format!(
                    "[identical to the earlier output of {what} above; elided by superstate]"
                );
            }
        }
        out
    }

    fn name(&self) -> &'static str {
        "compact"
    }
}
