# Harness-Observed Working Set and Loop-Breaker Guard Design

## Context & Problem Statement
During live evaluations with `gemini-3.8-flash` under `--context state-only`, the autonomous agent suffered from **"Stateless Amnesia"**:
1. **Voluntary State Neglect**: The model was equipped with an optional `note` tool, but never invoked it across multi-turn runs. The epistemic state ledger remained `{}`.
2. **The Two-File Ping-Pong Loop**: `StateOnly::build` strictly bounds prompt context by retaining only the single most recent tool result. When fixing a bug across a test file (`test_lru.py`) and an implementation file (`lru.py`), reading `lru.py` flushed `test_lru.py` from context. The model oscillated back and forth between re-reading both files for 7+ turns.
3. **Token Inefficiency**: On a 9-turn task, `StateOnly` consumed 17,482 input tokens across 13 turns (and exhausted 25 turns on unguided runs) compared to 16,797 tokens in `Conversational` mode.

Rather than relying on system prompt persuasion ("please use `note`"), the harness itself must maintain the agent's external working memory deterministically.

---

## Architectural Invariants
1. **Invariant I (Partition Integrity)**:
   - The working set lives strictly in **$R$ (`Runtime`)**. Agent patches from `note` cannot overwrite, corrupt, or truncate it.
2. **Invariant II (Strict $O(1)$ Prompt Bounding)**:
   - In `StateOnly::build`, `messages.len() <= 3` remains strictly preserved:
     - Message 0: User (System prompt, goal, epistemic state, active working set, action ledger).
     - Message 1: Assistant (Latest tool call).
     - Message 2: Tool/User (Latest tool result).
   - Total working set size is strictly capped ($\le 3$ active files, $\le 250$ lines total).
3. **Invariant III (Deterministic Zero-Latency Execution)**:
   - Working set updates, string replacements, line slicing, and loop detection execute synchronously in in-memory Rust string operations with zero external LLM latency.

---

## Data Architecture & Structures

### Data Model (`crates/superstate-core/src/state.rs` or `crates/superstate-agent/src/working_set.rs`)
```rust
/// A cached file snippet actively inspected or modified by the agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSnippet {
    /// Relative path within the repository workspace.
    pub path: String,
    /// Exact content of the inspected or edited range.
    pub content: String,
    /// Start line of the snippet (1-indexed, inclusive) if a slice was read.
    pub start_line: Option<usize>,
    /// End line of the snippet (1-indexed, inclusive) if a slice was read.
    pub end_line: Option<usize>,
    /// Turn number when this snippet was last read or edited.
    pub last_accessed_turn: u32,
}

/// Harness-managed working set representing active external working memory.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkingSet {
    /// Active file snippets keyed by relative path.
    pub files: Vec<FileSnippet>,
    /// Last observed test failure output from non-zero gate or test execution.
    pub last_failure: Option<String>,
}
```

### Bounds & Eviction Policy
- **Max Files**: At most 3 concurrent files in `files`.
- **Max Lines per File**: 100 lines (head/tail distillation if the read is larger).
- **Max Total Lines**: 250 lines across all files in the working set.
- **Eviction**: When a new file is read and the collection exceeds 3 files or 250 lines, the least-recently accessed file (`last_accessed_turn`) is evicted.

---

## Lifecycle & Synchronization (`crates/superstate-agent/src/run.rs`)

### 1. File Inspection (`read`)
- When `read` succeeds:
  - Check if `path` already exists in `working_set.files`.
  - If it exists, update its `content`, `start_line`, `end_line`, and set `last_accessed_turn = turn`.
  - If new, push `FileSnippet` and run LRU eviction if bounds are exceeded.

### 2. File Mutation (`edit`)
- When `edit` succeeds on `path`:
  - If `path` is tracked in `working_set.files`:
    - Apply the replacement chunk (`old` -> `new`) directly to the cached `content` in memory.
    - Update `last_accessed_turn = turn`.

### 3. Test Failure Tracking (`bash`)
- When `bash` executes a command:
  - If `exit_code != 0` and the command resembles a test command (`test`, `pytest`, `cargo test`, `unittest`, `npm test`):
    - Distil the output to the first 10 and last 10 lines.
    - Store in `working_set.last_failure`.
  - If `exit_code == 0` and `working_set.last_failure.is_some()`:
    - Clear `working_set.last_failure = None`.

---

## Context Assembly (`crates/superstate-agent/src/context.rs`)

In `StateOnly::build`, inject the working set into the initial `Message::User` prompt:

```markdown
## Active Working Set
### `lru.py` (lines 1-22)
```python
from collections import OrderedDict
...
```

### `test_lru.py` (lines 1-25)
```python
import unittest
...
```

## Last Test Failure
```text
FAIL: test_get_updates_lru_order (test_lru.TestLRUCache)
AssertionError: 2 != -1
```

## Actions Taken So Far
- Turn 1: `bash` `python3 -m unittest test_lru.py` -> exit 1
- Turn 2: `read` `lru.py` -> 22 lines cached in working set
...
```

If `working_set.files` is empty and `last_failure` is `None`, these sections are omitted entirely.

---

## Loop-Breaker Interceptor (Guard Clause)

To prevent the agent from getting stuck in amnesic reading loops:
- In `run.rs`, check each requested `read` call:
  - If the requested `path` is already in `working_set.files` with matching content/range:
  - And the agent has called `read` on this file **2 times consecutively** or **3 times without an intervening `edit` or `note`**:
  - The harness intercepts the call before execution and returns:
    ```text
    [Harness Working Memory Guard]: `{path}` is already loaded in your Active Working Set above.
    You do not need to re-read it. Propose your exact edit using `edit` or update your plan in `note`.
    ```
- This forces the agent out of the reading loop immediately without wasting further turns or tokens.

---

## Testing & Verification Strategy
1. **Unit Tests (`crates/superstate-agent/tests/test_working_set.rs`)**:
   - `test_working_set_add_and_update`: Verify adding and updating file snippets.
   - `test_working_set_lru_eviction`: Verify LRU eviction when exceeding file count (3) or line budget (250).
   - `test_working_set_edit_synchronization`: Verify in-memory content updates when `edit` replaces text.
   - `test_working_set_test_failure_lifecycle`: Verify capturing non-zero test output and clearing on exit 0.
2. **Context Assembly Tests (`crates/superstate-agent/tests/test_run.rs`)**:
   - `test_state_only_context_renders_working_set`: Assert active files and last failure appear in prompt.
   - `test_state_only_context_preserves_messages_bound`: Assert `messages.len() <= 3`.
3. **Loop-Breaker Tests (`crates/superstate-agent/tests/test_run.rs`)**:
   - `test_loop_breaker_intercepts_repeated_reads`: Assert guard triggers on redundant reads.
4. **End-to-End CLI Verification**:
   - Re-run live evaluation against the LRU cache problem with `gemini-3.8-flash` and verify resolution in $\le 7$ turns without oscillation.
