//! How the next prompt is assembled.
//!
//! [`Conversational`] is what mainstream harnesses do: append every turn and re-send the lot.
//!
//! [`Folded`] sends `[opening] [digest 1] … [digest k] [verbatim tail]`. Once the verbatim part
//! passes `budget_tokens`, its older half becomes one harness-written digest. Boundaries are
//! computed greedily from the start of history, so each depends only on the messages before it:
//! the next turn reproduces every earlier fold exactly, and between folds the output is pure
//! append. A cut never separates a tool call from its result.

use blyte_provider::Message;

use crate::state::State;

pub trait ContextStrategy: Send + Sync {
    /// Build the messages for the next call from the run so far.
    fn build(&self, state: &State, history: &[Message]) -> Vec<Message>;
    /// The folds this strategy makes over `history`, as `(a, b)` ranges of `history[1..]`.
    /// Strategies that never fold return none.
    fn folds(&self, _history: &[Message]) -> Vec<(usize, usize)> {
        Vec::new()
    }
    /// The fold budget in force after `history`, in approximate tokens, for strategies that fold.
    fn fold_budget(&self, _history: &[Message]) -> Option<usize> {
        None
    }
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

/// An append-only conversation, bounded by folding.
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
    let mut text = format!(
        "## Earlier turns, folded (messages {first}-{last}; summarised by the harness)\n\
                 The output of these tool calls is no longer in your context: read a file again \
                 before quoting or relying on its exact text.\n"
    );
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

/// Where `Folded` put its boundaries over `history[1..]`: each fold `(a, b)` and the start of the
/// verbatim tail.
struct FoldPlan {
    folds: Vec<(usize, usize)>,
    seg_start: usize,
}

/// Greedy fold boundaries over `rest`: each fold depends only on messages before it, so the next
/// turn reproduces every earlier fold exactly.
fn greedy_plan(rest: &[Message], budget: usize) -> FoldPlan {
    let mut folds: Vec<(usize, usize)> = Vec::new();
    let mut seg_start = 0;
    let mut size = 0;
    for i in 0..rest.len() {
        size += approx_tokens(&rest[i]);
        if size <= budget {
            continue;
        }
        // Fold roughly the older half, cutting only between turn blocks (never after a call or
        // before a result), so a call is never separated from its result or from calls in the
        // same batch. No safe cut yet: keep accumulating.
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
    }
    FoldPlan { folds, seg_start }
}

/// The prompt for a fold plan: the opening, one digest per fold, then the verbatim tail.
fn render_folded(opening: &Message, rest: &[Message], plan: &FoldPlan) -> Vec<Message> {
    let mut out = vec![opening.clone()];
    for &(a, b) in &plan.folds {
        // Message numbers are 1-based positions in the full history.
        out.push(digest(&rest[a..b], a + 2, b + 1));
    }
    out.extend_from_slice(&rest[plan.seg_start..]);
    out
}

impl Folded {
    /// Greedy boundaries from the start: each fold depends only on messages before it, so the next
    /// turn reproduces every earlier fold exactly.
    fn plan(&self, rest: &[Message]) -> FoldPlan {
        greedy_plan(rest, self.budget_tokens)
    }
}

impl ContextStrategy for Folded {
    fn build(&self, _state: &State, history: &[Message]) -> Vec<Message> {
        let Some((opening, rest)) = history.split_first() else {
            return Vec::new();
        };
        render_folded(opening, rest, &self.plan(rest))
    }

    fn folds(&self, history: &[Message]) -> Vec<(usize, usize)> {
        history
            .split_first()
            .map(|(_, rest)| self.plan(rest).folds)
            .unwrap_or_default()
    }

    fn fold_budget(&self, _history: &[Message]) -> Option<usize> {
        Some(self.budget_tokens)
    }

    fn name(&self) -> &'static str {
        "folded"
    }
}
