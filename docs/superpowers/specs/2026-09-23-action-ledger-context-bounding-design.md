# Harness-Observed Action Ledger for Context Bounding Design Spec

## 1. Context & Motivation

Under the SKILL.state formulation (arXiv:2608.26263v2), `superstate --context state-only` achieves $\mathcal{O}(1)$ prompt context footprint by passing only the task spec, structured state, and the latest observation:
$$\mathcal{C}_t = (P, \Sigma_t, O_t)$$
Prior conversational turns are discarded rather than accumulated.

### The Empirical Failure Mode: The "Goldfish" Effect
In real-world repository debugging tasks, frontier LLMs (pre-trained predominantly on conversational dialogue) frequently fail to self-report intermediate discoveries via the `note` tool. When an agent runs `read`, `grep`, or `bash` and does not call `note(patch: {...})` on that turn, the observation is discarded on the subsequent turn.

This produces the **"Goldfish" effect**:
1. The agent loses track of files it has already inspected.
2. The agent re-runs identical search patterns or commands.
3. If an edit fails, the agent may attempt the same invalid replacement.
4. When a rollback occurs, the agent risks repeating the exact sequence of actions that caused the gate failure.

Prior repository telemetry (FINDINGS 13.30) demonstrated that across long tasks, **all recorded notes were written by the runtime, and none by the agent**. Expecting the agent to self-report is structurally unreliable.

### The Solution: Harness-Observed Action Ledger
The harness already intercepts every tool call and executes it deterministically. Rather than expecting self-reporting, the **harness itself** will maintain an episodic **Action Ledger** inside the `Runtime` state partition. On every turn in `StateOnly` mode, the prompt presents a compact 1-line chronological summary of every action taken and its outcome.

---

## 2. Goals & Non-Goals

### Goals
- **Eliminate the Goldfish Loop**: Ensure the model always sees what it has tried, what files it read/edited, what search queries were run, and what errors occurred.
- **Strictly Bounded Context Overhead**: Action entries must be compact (approx. 15–25 tokens per entry). A 30-turn run adds only ~500 tokens total, preserving the bounded context footprint.
- **Zero Additional LLM Latency**: Summaries must be produced deterministically by the harness via fast string/AST/regex extraction—no intermediate summarization LLM calls.
- **Rollback Telemetry Persistence**: Rollback events must be preserved in the ledger so retries learn from prior failed attempts.
- **Type-Safe Three-Region Architecture**: The Action Ledger lives strictly in `Runtime` ($R$), preventing agent tampering.

### Non-Goals
- Replacing conversational mode (`Conversational` remains available as the full-history baseline).
- Storing full verbatim file contents or multi-megabyte stdout in the ledger (full output is only retained for the immediate latest observation $O_t$).

---

## 3. Data Model (`crates/superstate-agent/src/state.rs`)

### Action Entry Structure
```rust
/// A single action observed and summarized objectively by the harness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionEntry {
    /// Turn number when this action occurred.
    pub turn: u32,
    /// Name of the tool invoked (e.g. "read", "write", "edit", "bash", "glob", "grep", "note").
    pub tool: String,
    /// Canonical target or command (e.g. "stats.py (lines 10-40)", "pytest tests/", "pattern 'mean'").
    pub target: String,
    /// Short 1-line summary of the outcome (e.g. "22 lines read", "exit 1: ZeroDivisionError", "replaced 3 lines").
    pub summary: String,
    /// Whether the action resulted in an error or non-zero exit code.
    pub is_error: bool,
}
```

### Updates to `Runtime` Partition
```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Runtime {
    pub attempts: u32,
    /// Verification failures, most recent last.
    pub failures: Vec<String>,
    pub rollbacks: u32,
    /// Files the harness saw written, in order of first write.
    pub files_written: Vec<String>,
    /// Chronological log of all actions executed in this session.
    #[serde(default)]
    pub actions: Vec<ActionEntry>,
}
```

---

## 4. Deterministic Summarization Engine (`crates/superstate-agent/src/run.rs`)

When `Runner` processes a `ToolCall`, it extracts a target and an outcome summary without invoking any external services.

### Target Extraction
- `read`: Extracts `path` and optional `start_line` / `end_line`. E.g. `"crates/cli/src/main.rs (L1-L50)"` or `"stats.py"`.
- `write`: Extracts `path` and payload size. E.g. `"src/lib.rs (1420 bytes)"`.
- `edit`: Extracts `path`. E.g. `"src/parser.rs"`.
- `bash`: Extracts `command`, clamped to 60 characters with ellipses. E.g. `"python3 -m pytest tests/test_stats.py -q"`.
- `glob`: Extracts `pattern`. E.g. `"**/*.rs"`.
- `grep`: Extracts `pattern` and optional `path`. E.g. `"pattern 'ZeroDivisionError' in src/"`.
- `note`: Extracts patch key summary. E.g. `"patch (3 keys: root_cause, plan, files)"`.

### Outcome Compaction
- `read`: Count lines from content. E.g. `"45 lines read"`.
- `write`: `"file written"`.
- `edit`: If error: `"failed: <error_message>"`. If success: `"string replaced"`.
- `bash`: If exit 0: `"exit 0"`. If non-zero: parses stdout/stderr for the most informative failure line (e.g. `"exit 1: FAILED tests/test_calc.py::test_div_zero"`).
- `glob`: Count matching paths. E.g. `"5 files matched"`.
- `grep`: Count match lines. E.g. `"3 matches found"`.
- `note`: E.g. `"recorded; agent state has 4 field(s)"`.

### Rollback Annotation
When gate verification fails and rollback is triggered in `Runner::run`:
```rust
state.runtime.actions.push(ActionEntry {
    turn,
    tool: "rollback".to_string(),
    target: format!("checkpoint attempt #{}", state.runtime.attempts),
    summary: format!("gate failed; working tree reset to commit {}", base_sha),
    is_error: true,
});
```

---

## 5. Context Assembly (`crates/superstate-agent/src/context.rs`)

`StateOnly::build` assembles the initial User message containing the immutable task, the action ledger, and the agent state:

```rust
impl ContextStrategy for StateOnly {
    fn build(&self, state: &State, history: &[Message]) -> Vec<Message> {
        let mut out = Vec::new();

        let mut user_prompt = format!("## Task\n{}\n\n", state.orchestrator.goal);

        // Format actions ledger if any actions have occurred
        if !state.runtime.actions.is_empty() {
            user_prompt.push_str("## Actions Taken So Far\n");
            for act in &state.runtime.actions {
                let status_indicator = if act.is_error { " [ERROR]" } else { "" };
                user_prompt.push_str(&format!(
                    "- [Turn {}] {} `{}` -> {}{}\n",
                    act.turn, act.tool, act.target, act.summary, status_indicator
                ));
            }
            user_prompt.push('\n');
        }

        user_prompt.push_str(&format!(
            "## Your state\n{}\n",
            serde_json::to_string_pretty(&state.agent.fields).unwrap_or_default()
        ));

        out.push(Message::User(user_prompt));

        // Append the latest observation (ToolCall + ToolResult or last assistant message)
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
}
```

---

## 6. Verification & Invariants

1. **Prompt Message Bound**: In `StateOnly`, `messages.len() <= 3` remains invariant.
2. **Token Economy**: In a 30-turn simulation, the Action Ledger occupies < 750 tokens total, avoiding quadratic $\mathcal{O}(T^2)$ growth.
3. **No Information Loss Across Turns**: Files read, commands executed, and edit failures remain visible to the agent across turns 1 through $N$.
4. **Rollback Visibility**: On attempt retries after git rollback, the action ledger contains the `rollback` entry and prior failed actions.
5. **Quality Gates**:
   - `cargo check --workspace --all-targets` (0 warnings)
   - `cargo clippy --workspace --all-targets -- -D warnings` (0 warnings)
   - `cargo test --workspace` (all tests pass)
