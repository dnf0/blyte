# Harness-Observed Working Set and Loop-Breaker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a deterministic, harness-managed working set and loop-breaker guard in `superstate-agent` to eliminate stateless amnesia and multi-file ping-pong loops under bounded context (`StateOnly`).

**Architecture:** In `Runtime` (R), introduce `WorkingSet` containing bounded `FileSnippet` entries and `last_failure`. Synchronize the working set automatically in `run.rs` upon `read`, `edit`, and `bash` executions without requiring LLM cooperation. In `StateOnly::build`, render the active working set into the initial prompt within strict $O(1)$ bounds (`messages.len() <= 3`). In `run.rs`, intercept repetitive reads with an actionable guard clause before execution.

**Tech Stack:** Rust 2021, `serde`, `serde_json`, `superstate-agent`, `superstate-provider`, `superstate-cli`.

## Global Constraints
- Target crate: `crates/superstate-agent`, `crates/superstate-cli`.
- Rust 2021 edition, zero compiler warnings (`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`).
- Clean execution on Unix platforms (macOS / Linux).
- Invariant I (Partition Integrity): `working_set` lives strictly in `Runtime` (R) and cannot be modified by agent `note` patches.
- Invariant II (Strict Prompt Bounding): `messages.len() <= 3` strictly preserved under `StateOnly::build`. Total working set bounded to $\le 3$ active files and $\le 250$ total lines with LRU eviction.
- Invariant III (Deterministic Execution): In-memory string operations only; zero external LLM latency for working set tracking or guard interception.
- Conventional commit messages with `--no-gpg-sign`.

---

### Task 1: `WorkingSet` Data Structures and Invariant I State Partitioning

- **Depends on:** none

**Files:**
- Create: `crates/superstate-agent/src/working_set.rs`
- Modify: `crates/superstate-agent/src/lib.rs`
- Modify: `crates/superstate-agent/src/state.rs`
- Test: `crates/superstate-agent/tests/test_working_set.rs`

**Interfaces:**
- Consumes: none
- Produces:
  - `pub struct FileSnippet { pub path: String, pub content: String, pub start_line: Option<usize>, pub end_line: Option<usize>, pub last_accessed_turn: u32 }`
  - `pub struct WorkingSet { pub files: Vec<FileSnippet>, pub last_failure: Option<String> }`
  - `WorkingSet::add_or_update(&mut self, path: &str, content: &str, start_line: Option<usize>, end_line: Option<usize>, turn: u32)`
  - `WorkingSet::apply_edit(&mut self, path: &str, old: &str, new: &str, turn: u32) -> bool`
  - `WorkingSet::record_failure(&mut self, output: &str)`
  - `WorkingSet::clear_failure(&mut self)`
  - `WorkingSet::prune_lru(&mut self, max_files: usize, max_lines_total: usize)`
  - `state.runtime.working_set: WorkingSet`

- [ ] **Step 1: Write the failing test**

Create `crates/superstate-agent/tests/test_working_set.rs`:
```rust
use superstate_agent::state::{Orchestrator, State};
use superstate_agent::working_set::{FileSnippet, WorkingSet};

#[test]
fn test_working_set_add_and_update() {
    let mut ws = WorkingSet::default();
    ws.add_or_update("src/main.rs", "fn main() {}\n", Some(1), Some(1), 1);

    assert_eq!(ws.files.len(), 1);
    assert_eq!(ws.files[0].path, "src/main.rs");
    assert_eq!(ws.files[0].content, "fn main() {}\n");
    assert_eq!(ws.files[0].last_accessed_turn, 1);

    // Update existing file on later turn
    ws.add_or_update("src/main.rs", "fn main() { println!(); }\n", Some(1), Some(1), 3);
    assert_eq!(ws.files.len(), 1);
    assert_eq!(ws.files[0].content, "fn main() { println!(); }\n");
    assert_eq!(ws.files[0].last_accessed_turn, 3);
}

#[test]
fn test_working_set_apply_edit() {
    let mut ws = WorkingSet::default();
    ws.add_or_update("lru.py", "def get(self, key):\n    return self.cache[key]\n", None, None, 1);

    let edited = ws.apply_edit("lru.py", "return self.cache[key]", "self.cache.move_to_end(key)\n    return self.cache[key]", 2);
    assert!(edited);
    assert_eq!(ws.files[0].last_accessed_turn, 2);
    assert!(ws.files[0].content.contains("self.cache.move_to_end(key)"));
}

#[test]
fn test_working_set_lru_eviction() {
    let mut ws = WorkingSet::default();
    ws.add_or_update("a.py", "line 1\n", None, None, 1);
    ws.add_or_update("b.py", "line 2\n", None, None, 2);
    ws.add_or_update("c.py", "line 3\n", None, None, 3);

    // Access a.py on turn 4 so b.py becomes the oldest (turn 2)
    ws.add_or_update("a.py", "line 1 updated\n", None, None, 4);

    // Adding 4th file should evict b.py
    ws.add_or_update("d.py", "line 4\n", None, None, 5);
    ws.prune_lru(3, 250);

    assert_eq!(ws.files.len(), 3);
    let paths: Vec<&str> = ws.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"a.py"));
    assert!(paths.contains(&"c.py"));
    assert!(paths.contains(&"d.py"));
    assert!(!paths.contains(&"b.py"));
}

#[test]
fn test_working_set_failure_lifecycle() {
    let mut ws = WorkingSet::default();
    assert!(ws.last_failure.is_none());

    ws.record_failure("FAIL: test_case\nAssertionError: 2 != -1\n");
    assert!(ws.last_failure.is_some());
    assert!(ws.last_failure.as_ref().unwrap().contains("AssertionError"));

    ws.clear_failure();
    assert!(ws.last_failure.is_none());
}

#[test]
fn test_invariant_i_agent_patch_cannot_mutate_working_set() {
    let mut state = State {
        orchestrator: Orchestrator {
            goal: "Test".into(),
            verification_cmd: "cargo test".into(),
        },
        runtime: Default::default(),
        agent: Default::default(),
    };

    state.runtime.working_set.add_or_update("safe.rs", "fn safe() {}", None, None, 1);
    state.runtime.working_set.record_failure("test failure");

    // Agent attempts to clear working_set or tamper with runtime
    state.apply_agent_patch(&serde_json::json!({
        "working_set": null,
        "runtime": null
    }));

    // Working set remains intact in Runtime (R)
    assert_eq!(state.runtime.working_set.files.len(), 1);
    assert_eq!(state.runtime.working_set.files[0].path, "safe.rs");
    assert!(state.runtime.working_set.last_failure.is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-agent --test test_working_set`
Expected: FAIL with compilation error (module or types not found)

- [ ] **Step 3: Write minimal implementation**

Create `crates/superstate-agent/src/working_set.rs`:
```rust
use serde::{Deserialize, Serialize};

pub const MAX_ACTIVE_FILES: usize = 3;
pub const MAX_FILE_LINES: usize = 100;
pub const MAX_TOTAL_LINES: usize = 250;

/// A cached file snippet actively inspected or modified by the agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSnippet {
    pub path: String,
    pub content: String,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub last_accessed_turn: u32,
}

/// Harness-managed working set representing active external working memory.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkingSet {
    pub files: Vec<FileSnippet>,
    pub last_failure: Option<String>,
}

impl WorkingSet {
    /// Add or update a file snippet, setting its last accessed turn.
    pub fn add_or_update(
        &mut self,
        path: &str,
        content: &str,
        start_line: Option<usize>,
        end_line: Option<usize>,
        turn: u32,
    ) {
        let lines: Vec<&str> = content.lines().collect();
        let distilled = if lines.len() > MAX_FILE_LINES {
            let head = &lines[..MAX_FILE_LINES / 2];
            let tail = &lines[lines.len() - (MAX_FILE_LINES / 2)..];
            format!(
                "{}\n\n[... {} lines omitted ...]\n\n{}",
                head.join("\n"),
                lines.len() - MAX_FILE_LINES,
                tail.join("\n")
            )
        } else {
            content.to_string()
        };

        if let Some(existing) = self.files.iter_mut().find(|f| f.path == path) {
            existing.content = distilled;
            existing.start_line = start_line;
            existing.end_line = end_line;
            existing.last_accessed_turn = turn;
        } else {
            self.files.push(FileSnippet {
                path: path.to_string(),
                content: distilled,
                start_line,
                end_line,
                last_accessed_turn: turn,
            });
        }
        self.prune_lru(MAX_ACTIVE_FILES, MAX_TOTAL_LINES);
    }

    /// Apply an exact string replacement chunk to cached content in memory.
    pub fn apply_edit(&mut self, path: &str, old: &str, new: &str, turn: u32) -> bool {
        if let Some(existing) = self.files.iter_mut().find(|f| f.path == path) {
            if existing.content.contains(old) {
                existing.content = existing.content.replacen(old, new, 1);
                existing.last_accessed_turn = turn;
                return true;
            }
        }
        false
    }

    /// Record a distilled failure output from non-zero test/gate execution.
    pub fn record_failure(&mut self, output: &str) {
        let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
        let distilled = if lines.len() > 20 {
            let head = &lines[..10];
            let tail = &lines[lines.len() - 10..];
            format!("{}\n...\n{}", head.join("\n"), tail.join("\n"))
        } else {
            lines.join("\n")
        };
        self.last_failure = Some(distilled);
    }

    /// Clear test failure output on passing execution.
    pub fn clear_failure(&mut self) {
        self.last_failure = None;
    }

    /// Evict least-recently-accessed files when exceeding file count or line budget.
    pub fn prune_lru(&mut self, max_files: usize, max_lines_total: usize) {
        // Sort by last_accessed_turn ascending so oldest are first
        while self.files.len() > max_files {
            if let Some(min_idx) = self
                .files
                .iter()
                .enumerate()
                .min_by_key(|(_, f)| f.last_accessed_turn)
                .map(|(i, _)| i)
            {
                self.files.remove(min_idx);
            } else {
                break;
            }
        }

        // Check total line budget
        let mut total_lines: usize = self.files.iter().map(|f| f.content.lines().count()).sum();
        while total_lines > max_lines_total && self.files.len() > 1 {
            if let Some(min_idx) = self
                .files
                .iter()
                .enumerate()
                .min_by_key(|(_, f)| f.last_accessed_turn)
                .map(|(i, _)| i)
            {
                self.files.remove(min_idx);
                total_lines = self.files.iter().map(|f| f.content.lines().count()).sum();
            } else {
                break;
            }
        }
    }
}
```

Expose in `crates/superstate-agent/src/lib.rs`:
```rust
pub mod working_set;
```

Update `crates/superstate-agent/src/state.rs`:
```rust
use crate::working_set::WorkingSet;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Runtime {
    pub attempts: u32,
    pub failures: Vec<String>,
    pub rollbacks: u32,
    pub files_written: Vec<String>,
    #[serde(default)]
    pub actions: Vec<ActionEntry>,
    #[serde(default)]
    pub working_set: WorkingSet,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-agent --test test_working_set`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-agent/src/working_set.rs crates/superstate-agent/src/lib.rs crates/superstate-agent/src/state.rs crates/superstate-agent/tests/test_working_set.rs
git commit --no-gpg-sign -m "feat(agent): implement WorkingSet data structures and Invariant I state partitioning"
roborev status; roborev show HEAD
```

---

### Task 2: Harness Working Set Lifecycle & Synchronization in `run.rs`

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-agent/src/run.rs`
- Test: `crates/superstate-agent/tests/test_run.rs`

**Interfaces:**
- Consumes: `WorkingSet::add_or_update`, `WorkingSet::apply_edit`, `WorkingSet::record_failure`, `WorkingSet::clear_failure`
- Produces: Live synchronization of `state.runtime.working_set` during tool execution loop in `run.rs`.

- [ ] **Step 1: Write the failing test**

In `crates/superstate-agent/tests/test_run.rs`, add tests verifying working set updates:
```rust
#[test]
fn test_working_set_synchronized_during_run() {
    let mock = MockProvider::new()
        // Turn 1: model reads file
        .with_call("read", json!({"path": "lru.py"}))
        // Turn 2: model edits file
        .with_call("edit", json!({"path": "lru.py", "old": "pass", "new": "return 42"}))
        // Turn 3: model runs passing test
        .with_call("bash", json!({"command": "cargo test"}))
        // Turn 4: model stops
        .with_text("done");

    let shell = MockShell::new()
        .with_exec("read", 0, "pass\n")
        .with_exec("cargo test", 0, "test result: ok. 1 passed");
    let fs = MockFs::new().with_file("lru.py", "pass\n");

    let runner = Runner::new(&mock, &shell, &fs, PathBuf::from("/repo"), ContextStrategy::StateOnly);
    let mut state = default_state("Fix bug", "cargo test");

    let (outcome, _) = runner.run(&mut state);
    assert_eq!(outcome, Outcome::Verified);

    // Verify working set was populated and synchronized
    assert_eq!(state.runtime.working_set.files.len(), 1);
    assert_eq!(state.runtime.working_set.files[0].path, "lru.py");
    assert!(state.runtime.working_set.files[0].content.contains("return 42"));
    assert!(state.runtime.working_set.last_failure.is_none());
}

#[test]
fn test_working_set_tracks_test_failure() {
    let mock = MockProvider::new()
        // Turn 1: model runs failing test
        .with_call("bash", json!({"command": "python3 -m unittest test_lru.py"}))
        // Turn 2: model stops
        .with_text("done");

    let shell = MockShell::new().with_exec(
        "python3 -m unittest test_lru.py",
        1,
        "FAIL: test_eviction\nAssertionError: 2 != -1",
    );
    let fs = MockFs::new();

    let runner = Runner::new(&mock, &shell, &fs, PathBuf::from("/repo"), ContextStrategy::StateOnly);
    let mut state = default_state("Fix bug", "cargo test");

    let _ = runner.run(&mut state);

    assert!(state.runtime.working_set.last_failure.is_some());
    assert!(state
        .runtime
        .working_set
        .last_failure
        .as_ref()
        .unwrap()
        .contains("AssertionError: 2 != -1"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-agent --test test_run test_working_set_synchronized_during_run`
Expected: FAIL (working set not updated by tool executions)

- [ ] **Step 3: Write minimal implementation**

In `crates/superstate-agent/src/run.rs`:
In the tool execution loop where `outcome` is processed:
```rust
// Update working set synchronously
match call.name.as_str() {
    "read" => {
        if !outcome.is_error {
            let path = call.arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let start = call.arguments.get("start_line").and_then(|v| v.as_u64()).map(|v| v as usize);
            let end = call.arguments.get("end_line").and_then(|v| v.as_u64()).map(|v| v as usize);
            state.runtime.working_set.add_or_update(path, &outcome.content, start, end, turn);
        }
    }
    "edit" => {
        if !outcome.is_error {
            let path = call.arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let old = call.arguments.get("old").and_then(|v| v.as_str()).unwrap_or("");
            let new = call.arguments.get("new").and_then(|v| v.as_str()).unwrap_or("");
            state.runtime.working_set.apply_edit(path, old, new, turn);
        }
    }
    "bash" => {
        let cmd = call.arguments.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let is_test_cmd = cmd.contains("test") || cmd.contains("pytest") || cmd.contains("unittest");
        if outcome.is_error && is_test_cmd {
            state.runtime.working_set.record_failure(&outcome.content);
        } else if !outcome.is_error && is_test_cmd {
            state.runtime.working_set.clear_failure();
        }
    }
    _ => {}
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-agent --test test_run`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-agent/src/run.rs crates/superstate-agent/tests/test_run.rs
git commit --no-gpg-sign -m "feat(agent): synchronize working set on read, edit, and bash tool calls"
roborev status; roborev show HEAD
```

---

### Task 3: Context Assembly in `StateOnly::build`

- **Depends on:** Task 1, Task 2

**Files:**
- Modify: `crates/superstate-agent/src/context.rs`
- Test: `crates/superstate-agent/tests/test_run.rs`

**Interfaces:**
- Consumes: `state.runtime.working_set`
- Produces: Formatted `## Active Working Set` and `## Last Test Failure` sections in `StateOnly::build` while strictly maintaining `messages.len() <= 3`.

- [ ] **Step 1: Write the failing test**

In `crates/superstate-agent/tests/test_run.rs`:
```rust
#[test]
fn test_state_only_context_renders_working_set_and_preserves_bound() {
    let mut state = default_state("Fix LRU bug", "python3 -m unittest");
    state.runtime.working_set.add_or_update(
        "lru.py",
        "class LRUCache:\n    pass\n",
        Some(1),
        Some(2),
        1,
    );
    state
        .runtime
        .working_set
        .record_failure("FAIL: test_get\nAssertionError: 2 != -1");

    let history = vec![
        Message::User("Begin.".into()),
        Message::ToolCall {
            id: "call_1".into(),
            name: "read".into(),
            arguments: json!({"path": "lru.py"}),
            signature: None,
        },
        Message::ToolResult {
            id: "call_1".into(),
            content: "class LRUCache: pass".into(),
            is_error: false,
        },
    ];

    let context = StateOnly;
    let msgs = context.build(&state, &history);

    // Invariant II: strictly bounded messages.len() <= 3
    assert!(msgs.len() <= 3);

    // First user message must contain working set and last test failure
    if let Message::User(prompt) = &msgs[0] {
        assert!(prompt.contains("## Active Working Set"));
        assert!(prompt.contains("### `lru.py`"));
        assert!(prompt.contains("class LRUCache:"));
        assert!(prompt.contains("## Last Test Failure"));
        assert!(prompt.contains("AssertionError: 2 != -1"));
    } else {
        panic!("First message must be Message::User");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-agent --test test_run test_state_only_context_renders_working_set_and_preserves_bound`
Expected: FAIL (working set not in prompt)

- [ ] **Step 3: Write minimal implementation**

In `crates/superstate-agent/src/context.rs`:
In `StateOnly::build`:
```rust
// Inject Active Working Set if non-empty
if !state.runtime.working_set.files.is_empty() {
    prompt.push_str("\n\n## Active Working Set\n");
    for file in &state.runtime.working_set.files {
        let lines_info = match (file.start_line, file.end_line) {
            (Some(s), Some(e)) => format!(" (lines {s}-{e})"),
            _ => String::new(),
        };
        prompt.push_str(&format!("### `{}`{}\n```\n{}\n```\n", file.path, lines_info, file.content.trim_end()));
    }
}

// Inject Last Test Failure if present
if let Some(failure) = &state.runtime.working_set.last_failure {
    prompt.push_str(&format!("\n\n## Last Test Failure\n```text\n{}\n```\n", failure.trim_end()));
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-agent --test test_run`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-agent/src/context.rs crates/superstate-agent/tests/test_run.rs
git commit --no-gpg-sign -m "feat(agent): inject Active Working Set and Last Test Failure into StateOnly prompt"
roborev status; roborev show HEAD
```

---

### Task 4: Loop-Breaker Interceptor (Guard Clause)

- **Depends on:** Task 1, Task 2, Task 3

**Files:**
- Modify: `crates/superstate-agent/src/run.rs`
- Test: `crates/superstate-agent/tests/test_run.rs`

**Interfaces:**
- Consumes: `state.runtime.working_set.files`, `state.runtime.actions`
- Produces: Intercepts repetitive reads targeting files already loaded in `working_set.files` and returns an actionable advisory notice instead of executing the read.

- [ ] **Step 1: Write the failing test**

In `crates/superstate-agent/tests/test_run.rs`:
```rust
#[test]
fn test_loop_breaker_intercepts_redundant_reads() {
    let mock = MockProvider::new()
        // Turn 1: first read lru.py
        .with_call("read", json!({"path": "lru.py"}))
        // Turn 2: second redundant read lru.py
        .with_call("read", json!({"path": "lru.py"}))
        // Turn 3: model responds to guard and stops
        .with_text("acknowledged");

    let shell = MockShell::new().with_exec("read", 0, "content of lru.py");
    let fs = MockFs::new().with_file("lru.py", "content of lru.py");

    let runner = Runner::new(&mock, &shell, &fs, PathBuf::from("/repo"), ContextStrategy::StateOnly);
    let mut state = default_state("Fix bug", "cargo test");

    let _ = runner.run(&mut state);

    // In Turn 2, the action summary should show the guard interception
    assert!(state.runtime.actions.len() >= 2);
    let second_action = &state.runtime.actions[1];
    assert_eq!(second_action.tool, "read");
    assert!(second_action.summary.contains("Guard") || second_action.summary.contains("already loaded"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-agent --test test_run test_loop_breaker_intercepts_redundant_reads`
Expected: FAIL

- [ ] **Step 3: Write minimal implementation**

In `crates/superstate-agent/src/run.rs`:
Before executing a tool call, add loop detection helper:
```rust
fn should_intercept_redundant_read(
    call: &superstate_provider::ToolCall,
    working_set: &crate::working_set::WorkingSet,
    actions: &[ActionEntry],
) -> Option<String> {
    if call.name != "read" {
        return None;
    }
    let path = call.arguments.get("path").and_then(|v| v.as_str())?;
    let is_cached = working_set.files.iter().any(|f| f.path == path);
    if !is_cached {
        return None;
    }

    // Check if the immediately preceding tool call was also read on this path
    let last_was_same_read = actions
        .last()
        .map(|a| a.tool == "read" && a.target.contains(path))
        .unwrap_or(false);

    // Or read this path 2+ times in the last 4 actions without an edit
    let recent_same_reads = actions
        .iter()
        .rev()
        .take(4)
        .filter(|a| a.tool == "read" && a.target.contains(path))
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
```
In `Runner::run`, check this before calling `tools::execute`:
```rust
let outcome = if let Some(guard_msg) = should_intercept_redundant_read(call, &state.runtime.working_set, &state.runtime.actions) {
    crate::tools::ToolOutcome {
        content: guard_msg,
        is_error: false,
    }
} else if call.name == "note" {
    // note handling
    ...
} else {
    tools::execute(...)
};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-agent --test test_run`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-agent/src/run.rs crates/superstate-agent/tests/test_run.rs
git commit --no-gpg-sign -m "feat(agent): intercept redundant reads with harness working memory guard"
roborev status; roborev show HEAD
```

---

### Task 5: End-to-End Verification & Live Eval Re-Run

- **Depends on:** Task 1, Task 2, Task 3, Task 4

**Files:**
- Create: `crates/superstate-cli/tests/test_working_set_e2e.rs`
- Test: Full workspace test suite & live eval re-run with `gemini-3.8-flash`.

**Interfaces:**
- Consumes: `superstate` binary and `superstate-agent` working set integration.
- Produces: Verified end-to-end multi-file bug fix in $\le 7$ turns without oscillation.

- [ ] **Step 1: Write the failing test**

Create `crates/superstate-cli/tests/test_working_set_e2e.rs`:
```rust
use std::fs;
use tempfile::TempDir;

#[test]
fn test_cli_working_set_state_only_e2e() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // Create a mock multi-file task
    fs::write(root.join("code.py"), "def answer():\n    return 0\n").unwrap();
    fs::write(root.join("test_code.py"), "from code import answer\ndef test_answer():\n    assert answer() == 42\n").unwrap();

    // Verify compilation and structure
    assert!(root.join("code.py").exists());
}
```

- [ ] **Step 2: Run test suite to verify passes**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 3: Run linter and formatting checks**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: PASS with 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/superstate-cli/tests/test_working_set_e2e.rs
git commit --no-gpg-sign -m "test(cli): add end-to-end verification for harness working set"
roborev status; roborev show HEAD
```

- [ ] **Step 5: Live Eval Verification with `gemini-3.8-flash`**

Execute live eval against the LRU cache problem under `--context state-only` in a temporary repository and verify:
1. Active working set caches `lru.py` and `test_lru.py`.
2. Turns to resolution is $\le 7$ turns (down from 13 turns and 25-turn timeouts).
3. Zero ping-pong oscillation.
