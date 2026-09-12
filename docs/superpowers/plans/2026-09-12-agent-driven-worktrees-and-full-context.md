# Agent-Driven Worktrees & Full-Context Turnkey Execution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate worktree permission/submodule loss on terminal tasks and context loss on SWE tasks by defaulting to in-place execution with agent-driven worktrees, injecting the full task specification into turnkey prompts, and eliminating blind planning turns.

**Architecture:** 
1. In `superstate-core`, add `goal: Option<String>` to `ExecutionState` and display `## 🎯 Global Task Objective & Requirements` in all turnkey prompts.
2. In `superstate-cli`, default `state init` to in-place execution (`no_worktree: true`) unless `--worktree` is explicitly requested, and provide `superstate worktree create/merge` commands.
3. In `superstate-eval`, make `zero_turn_planning` universal (removing the `vcmd` requirement so SWE tasks skip blind Phase 1 planning) and start execution in Turn 1 with immediate repo and shell access.

**Tech Stack:** Rust (superstate-core, superstate-cli, superstate-worktree), Python 3.12 (superstate-eval, pytest, Docker).

## Global Constraints
- Target Rust version: Rust 2021 edition.
- Target Python version: Python 3.12 with strict type hints.
- Code quality checks: `cargo test --all`, `cargo clippy --all-targets -- -D warnings`, `uv run pytest`, `uv run ruff check`, `uv run pyright`.
- Commit convention: Conventional Commits (`feat:`, `fix:`, `test:`, `docs:`).

---

### Task 1: Model & Prompt Context Fidelity (`superstate-core`)

- **Depends on:** none

**Files:**
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-core/src/models.rs:135-185`
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-core/src/state_machine.rs:85-150`
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-core/src/prompt.rs:90-160`
- Test: `/Users/danielfisher/repos/superstate/crates/superstate-core/tests/test_state_prompt.rs`
- Test: `/Users/danielfisher/repos/superstate/crates/superstate-core/tests/test_state_milestones.rs`

**Interfaces:**
- Produces: `ExecutionState.goal: Option<String>`
- Produces: `render_turnkey_prompt` rendering `## 🎯 Global Task Objective & Requirements` when `state.goal` is `Some`

- [ ] **Step 1: Write the failing tests for goal field and prompt rendering**

In `/Users/danielfisher/repos/superstate/crates/superstate-core/tests/test_state_prompt.rs`:
```rust
#[test]
fn test_render_turnkey_prompt_includes_global_goal() {
    let mut state = ExecutionState::default();
    state.goal = Some("Implement a high-performance streaming parser for JSON2 with byte-exact output.".to_string());
    state.active_task = Some(TaskSpec {
        id: "task-1".to_string(),
        title: "Add streaming buffer".to_string(),
        depends_on: vec![],
        files_modify: vec![],
        interfaces_produces: vec![],
        verification_cmd: Some("cargo test".to_string()),
    });

    let state_path = PathBuf::from("/tmp/test-state/state.json");
    let rendered = render_turnkey_prompt(&state, &state_path, None, true, false).unwrap();

    assert!(rendered.contains("## 🎯 Global Task Objective & Requirements"));
    assert!(rendered.contains("Implement a high-performance streaming parser for JSON2 with byte-exact output."));
    assert!(rendered.contains("Add streaming buffer"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-core --test test_state_prompt test_render_turnkey_prompt_includes_global_goal`
Expected: FAIL with "no field `goal` on type `ExecutionState`"

- [ ] **Step 3: Implement minimal code in `models.rs`, `state_machine.rs`, and `prompt.rs`**

1. In `/Users/danielfisher/repos/superstate/crates/superstate-core/src/models.rs`:
Add `pub goal: Option<String>` to `ExecutionState` struct (with `#[serde(default, skip_serializing_if = "Option::is_none")]`).
Add `goal: None` in `Default for ExecutionState`.

2. In `/Users/danielfisher/repos/superstate/crates/superstate-core/src/state_machine.rs`:
In `init_state_direct`:
```rust
pub fn init_state_direct(
    goal: &str,
    verify_cmd: &str,
    files_modify: Vec<PathBuf>,
    no_worktree: bool,
) -> ExecutionState {
    ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: goal.to_string(),
            depends_on: Vec::new(),
            files_modify,
            interfaces_produces: Vec::new(),
            verification_cmd: Some(verify_cmd.to_string()),
        }),
        pending_tasks: Vec::new(),
        completed_tasks: Vec::new(),
        modified_files: Vec::new(),
        errors: Vec::new(),
        plan_path: None,
        notes: Vec::new(),
        active_worktree: None,
        active_branch: None,
        no_worktree,
        base_branch: None,
        goal: Some(goal.to_string()),
    }
}
```
In `init_state_milestones`:
```rust
pub fn init_state_milestones(
    goal: Option<&str>,
    milestones: Vec<MilestoneSpec>,
    base_branch: Option<&str>,
    no_worktree: bool,
) -> Result<ExecutionState, StateError> {
    ...
    // Set goal on returned state
    state.goal = goal.map(|g| g.to_string());
    Ok(state)
}
```

3. In `/Users/danielfisher/repos/superstate/crates/superstate-core/src/prompt.rs`:
In `render_turnkey_prompt`:
```rust
    let mut rendered = String::new();

    rendered.push_str(&format!(
        "You are an Implementer Subagent working on **{}: {}**.\n\n",
        active.id, active.title
    ));

    if let Some(ref goal) = state.goal {
        rendered.push_str("## 🎯 Global Task Objective & Requirements\n");
        rendered.push_str(goal.trim());
        rendered.push_str("\n\n");
    }

    rendered.push_str("## Task Specification\n");
    rendered.push_str(&format!("- Task: {}: {}\n", active.id, active.title));
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-core`
Expected: PASS

- [ ] **Step 5: Commit changes**

```bash
git -C /Users/danielfisher/repos/superstate add crates/superstate-core
git -C /Users/danielfisher/repos/superstate commit -m "feat(core): record global goal in ExecutionState and render in turnkey prompts"
```

---

### Task 2: In-Place Default & Agent Worktree CLI (`superstate-cli`)

- **Depends on:** Task 1

**Files:**
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-cli/src/state_cli.rs:15-80`
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-cli/src/commands/state.rs:280-450`
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-cli/src/commands/worktree.rs`
- Test: `/Users/danielfisher/repos/superstate/crates/superstate-cli/tests/test_state_prompt_worktree.rs`

**Interfaces:**
- Produces: `superstate state init` defaulting to in-place (`no_worktree: true`) unless `--worktree` is explicitly provided.
- Produces: `superstate worktree create [task-id]` and `superstate worktree merge [commit-msg]` for agent-driven isolation.

- [ ] **Step 1: Write the failing tests for in-place default and explicit worktree opt-in**

In `/Users/danielfisher/repos/superstate/crates/superstate-cli/tests/test_state_init_default.rs`:
```rust
#[test]
fn test_state_init_defaults_to_in_place() {
    let temp = tempfile::tempdir().unwrap();
    let state_file = temp.path().join(".agent-state/state.json");
    
    let args = StateInitArgs {
        plan: None,
        goal: Some("Test goal".to_string()),
        verify_cmd: Some("echo 1".to_string()),
        files: vec![],
        milestones: None,
        state: Some(state_file.clone()),
        worktree: false, // Default is false (in-place)
    };

    run_state_init(&args).unwrap();
    let state = load_state(&state_file).unwrap();
    assert!(state.no_worktree);
    assert_eq!(state.active_worktree, None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-cli --test test_state_init_default`
Expected: FAIL (argument or field not defined)

- [ ] **Step 3: Implement minimal CLI changes in `state_cli.rs` and `commands/state.rs`**

1. In `crates/superstate-cli/src/state_cli.rs`:
Replace `no_worktree: bool` with `worktree: bool` (flag `--worktree`, default `false`).
```rust
    /// Enable Git worktree isolation for tasks (default: false, runs in-place)
    #[arg(long, default_value_t = false)]
    pub worktree: bool,
```
2. In `crates/superstate-cli/src/commands/state.rs`:
When `args.worktree` is false, set `no_worktree: true` and do NOT create a worktree.
When `args.worktree` is true, set `no_worktree: false` and provision `ensure_task_worktree`.

- [ ] **Step 4: Run tests to verify all tests pass**

Run: `cargo test --all`
Expected: ALL PASS

- [ ] **Step 5: Commit changes**

```bash
git -C /Users/danielfisher/repos/superstate add crates/superstate-cli
git -C /Users/danielfisher/repos/superstate commit -m "feat(cli): default state init to in-place execution with explicit --worktree opt-in"
```

---

### Task 3: Universal Zero-Turn Planning in Adapter (`superstate-eval`)

- **Depends on:** Task 2

**Files:**
- Modify: `/Users/danielfisher/repos/superstate-eval/lab/engine/adapters/superstate.py:730-850`
- Test: `/Users/danielfisher/repos/superstate-eval/tests/test_adapters.py`

**Interfaces:**
- Consumes: `superstate state init --goal "<instruction>"`
- Produces: `SuperstateAdapter` initializing state directly without planning turns for all tasks.

- [ ] **Step 1: Write failing test in `tests/test_adapters.py`**

In `/Users/danielfisher/repos/superstate-eval/tests/test_adapters.py`:
```python
def test_superstate_zero_turn_init_without_verification_cmd(tmp_path: Path):
    adapter = SuperstateAdapter()
    task = TaskSpec(
        id="datacurve/test-task",
        instruction="Fix the null pointer exception in parser.",
        verification_cmd=None, # Sealed tests
    )
    # Verify adapter directly initializes state without requiring plan.md
    initialized = adapter._init_direct_state_if_needed(tmp_path, task)
    assert initialized is True
    state_file = tmp_path / ".agent-state" / "state.json"
    assert state_file.is_file()
    state_data = json.loads(state_file.read_text())
    assert state_data["goal"] == task.instruction
    assert state_data["active_task"]["id"] == "task-1"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `uv run pytest tests/test_adapters.py -k test_superstate_zero_turn_init_without_verification_cmd`
Expected: FAIL

- [ ] **Step 3: Implement universal direct state init in `lab/engine/adapters/superstate.py`**

In `lab/engine/adapters/superstate.py`:
Update lines 737–757:
```python
        if not plan_text and self.zero_turn_planning:
            state_path = workspace_path / ".agent-state" / "state.json"
            state_path.parent.mkdir(parents=True, exist_ok=True)
            init_ok = False

            # Direct state initialization via the superstate binary
            if self.superstate_bin:
                cmd = [
                    str(self.superstate_bin),
                    "state",
                    "init",
                    "--goal",
                    task.instruction,
                    "--state",
                    str(state_path),
                ]
                if vcmd:
                    cmd.extend(["--verify-cmd", vcmd])
                else:
                    cmd.extend(["--verify-cmd", "true"])

                # If no_worktree is enabled or default, run in-place
                if not self.worktree:
                    cmd.append("--no-worktree")
```

- [ ] **Step 4: Run test to verify it passes**

Run: `uv run pytest tests/test_adapters.py -k test_superstate_zero_turn_init_without_verification_cmd`
Expected: PASS

- [ ] **Step 5: Run full test suite and linters in `superstate-eval`**

Run: `uv run ruff check lab tests`
Run: `uv run pyright lab`
Run: `uv run pytest tests/`
Expected: ALL PASS

- [ ] **Step 6: Commit changes**

```bash
git -C /Users/danielfisher/repos/superstate-eval add lab/engine/adapters/superstate.py tests/test_adapters.py
git -C /Users/danielfisher/repos/superstate-eval commit -m "feat(eval): make zero-turn planning universal across all tasks"
```

---

### Task 4: Targeted Benchmark Verification on Target Subset

- **Depends on:** Task 3

**Files:**
- Script: `/Users/danielfisher/repos/superstate-eval/scripts/paired_run.py`
- Target Tasks:
  - `terminal-bench/openssl-selfsigned-cert`
  - `terminal-bench/build-cython-ext`
  - `datacurve/meriyah-explicit-resource-declarations`
  - `datacurve/expr-try-catch-errors`

- [ ] **Step 1: Run paired evaluation on Terminal-Bench subset (`openssl-selfsigned-cert`, `build-cython-ext`)**

Run:
```bash
uv run python scripts/paired_run.py \
  --tasks terminal-bench/openssl-selfsigned-cert terminal-bench/build-cython-ext \
  --arms vanilla superstate_rust \
  --repeats 1 \
  --concurrency 2 \
  --out results/targeted_terminal_validation.json
```
Verify:
1. `openssl-selfsigned-cert`: `superstate_rust` passes 100% (file permissions `0600` preserved).
2. `build-cython-ext`: `superstate_rust` passes 100% (cloned subrepo `pyknotid` present).
3. Superstate matches or beats Vanilla on terminal-bench tasks.

- [ ] **Step 2: Run paired evaluation on SWE-Bench subset (`expr-try-catch-errors`)**

Run:
```bash
uv run python scripts/paired_run.py \
  --tasks datacurve/expr-try-catch-errors \
  --arms vanilla superstate_rust \
  --repeats 1 \
  --concurrency 2 \
  --out results/targeted_swe_validation.json
```
Verify:
1. Zero-turn direct execution activates in Turn 1 without a separate planning turn.
2. Full task instructions appear in turnkey prompts.
3. Test passes binary grading.

- [ ] **Step 3: Commit benchmark results and summary report**

```bash
git -C /Users/danielfisher/repos/superstate-eval add results/targeted_terminal_validation.json results/targeted_swe_validation.json
git -C /Users/danielfisher/repos/superstate-eval commit -m "chore(eval): add targeted validation results for agent-driven worktrees and full context"
```
