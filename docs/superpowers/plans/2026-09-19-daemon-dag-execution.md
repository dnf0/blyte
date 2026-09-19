# Autonomous Daemon Dependency DAG Execution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable `DaemonRunner` and `superstate-core` to autonomously execute multi-milestone dependency DAGs in topological order with fail-closed deadlock detection and scoped milestone rollback isolation.

**Architecture:** Extend `MilestoneSpec` in `models.rs` to support optional explicit `id` and `depends_on`. Upgrade `advance_step()` in `state_machine.rs` to eliminate the blind `.unwrap_or(0)` fallback, strictly advancing to the first unblocked task whose dependencies are satisfied in `completed_tasks` and returning an explicit `Blocked` error if pending tasks remain deadlocked. Wire `DaemonRunner` in `daemon.rs` to track baseline commits per milestone ID, commit checkpoint git milestones on gate pass, and isolate `--auto-rollback` resets to the active milestone's baseline commit.

**Tech Stack:** Rust 2021/2024, `superstate-core`, `superstate-cli`, `serde`, `serde_json`, `chrono`, Git subprocess management, `cargo test`.

## Global Constraints
- Must compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No new external crate dependencies.
- Zero network I/O or direct API keys in Rust binary.
- Pure standard library and existing workspace crates.
- Follow conventional commits with `--no-gpg-sign`.

---

### Task 1: Model Enhancements & State Machine DAG Advancement

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-core/src/models.rs:120-135`
- Modify: `crates/superstate-core/src/state_machine.rs:120-250`
- Test: `crates/superstate-core/tests/test_state_machine_dag.rs`

**Interfaces:**
- Produces:
  - `MilestoneSpec { pub id: Option<String>, pub title: String, pub verify_cmd: String, pub depends_on: Option<Vec<String>>, pub files: Vec<PathBuf> }`
  - `init_state_milestones(goal: Option<&str>, milestones: Vec<MilestoneSpec>, base_branch: Option<&str>, no_worktree: bool) -> Result<ExecutionState, StateError>`
  - `advance_step(state: &mut ExecutionState) -> Result<(), StateError>` with deadlock detection

- [ ] **Step 1: Write failing integration test for DAG advancement and deadlock detection**

Create `crates/superstate-core/tests/test_state_machine_dag.rs`:
```rust
use superstate_core::models::{ExecutionState, ExecutionStatus, MilestoneSpec, TaskSpec};
use superstate_core::state_machine::{advance_step, init_state_milestones, ready_tasks};

#[test]
fn test_init_state_milestones_with_custom_dag_dependencies() {
    let milestones = vec![
        MilestoneSpec {
            id: Some("task-setup".to_string()),
            title: "Setup environment".to_string(),
            verify_cmd: "test -f setup.done".to_string(),
            depends_on: Some(vec![]),
            files: vec![],
        },
        MilestoneSpec {
            id: Some("task-api".to_string()),
            title: "Implement API".to_string(),
            verify_cmd: "test -f api.done".to_string(),
            depends_on: Some(vec!["task-setup".to_string()]),
            files: vec![],
        },
        MilestoneSpec {
            id: Some("task-cli".to_string()),
            title: "Implement CLI".to_string(),
            verify_cmd: "test -f cli.done".to_string(),
            depends_on: Some(vec!["task-setup".to_string()]),
            files: vec![],
        },
        MilestoneSpec {
            id: Some("task-integration".to_string()),
            title: "Integration tests".to_string(),
            verify_cmd: "test -f integration.done".to_string(),
            depends_on: Some(vec!["task-api".to_string(), "task-cli".to_string()]),
            files: vec![],
        },
    ];

    let state = init_state_milestones(Some("Full System"), milestones, None, true).unwrap();
    assert_eq!(state.status, ExecutionStatus::InProgress);
    assert_eq!(state.active_task.as_ref().unwrap().id, "task-setup");
    assert_eq!(state.pending_tasks.len(), 3);
}

#[test]
fn test_advance_step_resolves_diamond_dag_topologically() {
    let mut state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "t1".to_string(),
            title: "T1".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("true".to_string()),
        }),
        pending_tasks: vec![
            TaskSpec {
                id: "t3".to_string(),
                title: "T3 (depends on t2a and t2b)".to_string(),
                depends_on: vec!["t2a".to_string(), "t2b".to_string()],
                files_modify: vec![],
                verification_cmd: Some("true".to_string()),
            },
            TaskSpec {
                id: "t2b".to_string(),
                title: "T2b (depends on t1)".to_string(),
                depends_on: vec!["t1".to_string()],
                files_modify: vec![],
                verification_cmd: Some("true".to_string()),
            },
            TaskSpec {
                id: "t2a".to_string(),
                title: "T2a (depends on t1)".to_string(),
                depends_on: vec!["t1".to_string()],
                files_modify: vec![],
                verification_cmd: Some("true".to_string()),
            },
        ],
        completed_tasks: vec![],
        modified_files: vec![],
        errors: vec![],
        plan_path: None,
        notes: vec![],
        active_worktree: None,
        active_branch: None,
        no_worktree: true,
        base_branch: None,
        goal: None,
    };

    // Advancing from t1 should pick t2b (first ready pending task depending on t1, skipping t3)
    advance_step(&mut state).unwrap();
    assert_eq!(state.active_task.as_ref().unwrap().id, "t2b");
    assert_eq!(state.completed_tasks.len(), 1);
    assert_eq!(state.completed_tasks[0].id, "t1");

    // Advancing from t2b should pick t2a (t3 still blocked because t2a not completed)
    advance_step(&mut state).unwrap();
    assert_eq!(state.active_task.as_ref().unwrap().id, "t2a");

    // Advancing from t2a should now unblock and pick t3
    advance_step(&mut state).unwrap();
    assert_eq!(state.active_task.as_ref().unwrap().id, "t3");

    // Advancing from t3 completes the graph
    advance_step(&mut state).unwrap();
    assert!(state.active_task.is_none());
    assert_eq!(state.status, ExecutionStatus::Completed);
    assert_eq!(state.completed_tasks.len(), 4);
}

#[test]
fn test_advance_step_deadlock_halts_with_blocked_status() {
    let mut state = ExecutionState {
        status: ExecutionStatus::InProgress,
        step: 1,
        active_task: Some(TaskSpec {
            id: "t1".to_string(),
            title: "T1".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("true".to_string()),
        }),
        pending_tasks: vec![TaskSpec {
            id: "t2".to_string(),
            title: "T2 (depends on non-existent t99)".to_string(),
            depends_on: vec!["t99".to_string()],
            files_modify: vec![],
            verification_cmd: Some("true".to_string()),
        }],
        completed_tasks: vec![],
        modified_files: vec![],
        errors: vec![],
        plan_path: None,
        notes: vec![],
        active_worktree: None,
        active_branch: None,
        no_worktree: true,
        base_branch: None,
        goal: None,
    };

    let res = advance_step(&mut state);
    assert!(res.is_err());
    assert_eq!(state.status, ExecutionStatus::Blocked);
    assert!(state.active_task.is_none());
}
```

- [ ] **Step 2: Verify test fails (RED)**

Run:
```bash
cargo test -p superstate-core --test test_state_machine_dag
```
Expected: Compilation errors or test failures because `MilestoneSpec` fields and deadlock handling in `advance_step` are not yet updated.

- [ ] **Step 3: Update `MilestoneSpec` in `crates/superstate-core/src/models.rs`**

Update lines 122-129 in `crates/superstate-core/src/models.rs`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MilestoneSpec {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    pub verify_cmd: String,
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
    #[serde(default)]
    pub files: Vec<PathBuf>,
}
```

- [ ] **Step 4: Update `init_state_milestones` and `advance_step` in `crates/superstate-core/src/state_machine.rs`**

Update `init_state_milestones`:
- Assign `id` using `milestone.id.unwrap_or_else(|| format!("task-{}", idx + 1))`.
- Assign `depends_on`:
  - If `Some(deps)`, use `deps`.
  - If `None`, if `idx == 0`, `vec![]`; if `idx > 0`, `vec![prev_id]`.
- Identify initial `active_task`:
  - Select the first task whose `depends_on` are empty or satisfied.
  - Remaining tasks placed in `pending_tasks`.

Update `advance_step`:
- When `!state.pending_tasks.is_empty()`:
  - Search `pending_tasks.iter().position(...)`.
  - On `Some(idx)`: remove task, set `active_task`, increment `step`, status `InProgress`.
  - On `None`: set `active_task = None`, `status = ExecutionStatus::Blocked`, return `Err(StateError::InvalidTransition("Cannot advance: pending tasks exist but none have satisfied dependencies".to_string()))`.
- When `state.pending_tasks.is_empty()`:
  - Set `active_task = None`, `status = ExecutionStatus::Completed`.

- [ ] **Step 5: Verify tests pass (GREEN)**

Run:
```bash
cargo test -p superstate-core --test test_state_machine_dag
cargo test -p superstate-core
```
Expected: All tests pass cleanly.

- [ ] **Step 6: Commit changes**

```bash
git add crates/superstate-core/src/models.rs crates/superstate-core/src/state_machine.rs crates/superstate-core/tests/test_state_machine_dag.rs
git commit --no-gpg-sign -m "feat(core): implement DAG milestone resolution and deadlock detection in advance_step"
```

---

### Task 2: Daemon Runner Topological Advancement & Scoped Rollback

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-core/src/daemon.rs:920-990`
- Test: `crates/superstate-core/tests/test_daemon_dag_execution.rs`

**Interfaces:**
- Produces:
  - `DaemonRunner::run_step()` handling DAG milestone transitions and blocked graph outcomes cleanly.
  - Milestone checkpoint commits and scoped git rollback resetting only to the active milestone start commit.

- [ ] **Step 1: Write failing integration test for multi-milestone daemon execution and rollback isolation**

Create `crates/superstate-core/tests/test_daemon_dag_execution.rs`:
```rust
use std::fs;
use std::path::Path;
use std::process::Command;
use superstate_core::daemon::{DaemonConfig, DaemonRunner, DaemonStepOutcome};
use superstate_core::models::{ExecutionState, ExecutionStatus, MilestoneSpec};
use superstate_core::state_machine::{init_state_milestones, save_state_atomic};
use tempfile::tempdir;

fn init_git_repo(dir: &Path) {
    Command::new("git").args(["init"]).current_dir(dir).output().unwrap();
    Command::new("git").args(["config", "user.name", "Test"]).current_dir(dir).output().unwrap();
    Command::new("git").args(["config", "user.email", "test@test.com"]).current_dir(dir).output().unwrap();
    fs::write(dir.join(".gitkeep"), "").unwrap();
    Command::new("git").args(["add", "."]).current_dir(dir).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial commit"]).current_dir(dir).output().unwrap();
}

#[test]
fn test_daemon_executes_sequential_milestones_with_checkpoints() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    init_git_repo(root);

    let milestones = vec![
        MilestoneSpec {
            id: Some("task-1".to_string()),
            title: "Create file A".to_string(),
            verify_cmd: "test -f a.txt".to_string(),
            depends_on: None,
            files: vec![],
        },
        MilestoneSpec {
            id: Some("task-2".to_string()),
            title: "Create file B".to_string(),
            verify_cmd: "test -f b.txt".to_string(),
            depends_on: None,
            files: vec![],
        },
    ];

    let state = init_state_milestones(Some("Sequential"), milestones, None, true).unwrap();
    let state_path = root.join(".agent-state").join("state.json");
    save_state_atomic(&state_path, &state).unwrap();

    // Driver command creates a.txt when on task-1, b.txt when on task-2
    let driver_script = r#"
if [ "$SUPERSTATE_ACTIVE_TASK" = "task-1" ]; then
    echo "content a" > a.txt
elif [ "$SUPERSTATE_ACTIVE_TASK" = "task-2" ]; then
    echo "content b" > b.txt
fi
"#;

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(driver_script.to_string()),
        max_retries: 2,
        timeout_secs: 10,
        driver_effort: None,
        advisory_gate: false,
        review_blocking: false,
        review_model: None,
        auto_rollback: true,
        shunt_driver_output: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let report = runner.run_to_completion().unwrap();

    assert_eq!(report.status, "completed");
    assert!(root.join("a.txt").exists());
    assert!(root.join("b.txt").exists());

    // Verify git log has checkpoint commits for both milestones
    let git_log = Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(root)
        .output()
        .unwrap();
    let log_str = String::from_utf8_lossy(&git_log.stdout);
    assert!(log_str.contains("checkpoint completed milestone task-1"));
    assert!(log_str.contains("checkpoint completed milestone task-2"));
}

#[test]
fn test_daemon_rollback_isolation_preserves_earlier_milestone_commits() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    init_git_repo(root);

    let milestones = vec![
        MilestoneSpec {
            id: Some("task-1".to_string()),
            title: "Create file A".to_string(),
            verify_cmd: "test -f a.txt".to_string(),
            depends_on: None,
            files: vec![],
        },
        MilestoneSpec {
            id: Some("task-2".to_string()),
            title: "Create file B".to_string(),
            verify_cmd: "test -f b.txt".to_string(),
            depends_on: None,
            files: vec![],
        },
    ];

    let state = init_state_milestones(Some("Rollback Isolation"), milestones, None, true).unwrap();
    let state_path = root.join(".agent-state").join("state.json");
    save_state_atomic(&state_path, &state).unwrap();

    // Driver passes task-1, but on task-2 attempt 1 it writes a bad file and fails, then on attempt 2 writes b.txt
    let attempt_file = root.join("attempt_counter.txt");
    fs::write(&attempt_file, "0").unwrap();

    let driver_script = format!(
        r#"
if [ "$SUPERSTATE_ACTIVE_TASK" = "task-1" ]; then
    echo "content a" > a.txt
elif [ "$SUPERSTATE_ACTIVE_TASK" = "task-2" ]; then
    COUNT=$(cat {})
    COUNT=$((COUNT + 1))
    echo $COUNT > {}
    if [ "$COUNT" -eq 1 ]; then
        echo "bad content" > bad.txt
    else
        echo "content b" > b.txt
    fi
fi
"#,
        attempt_file.display(),
        attempt_file.display()
    );

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(driver_script),
        max_retries: 3,
        timeout_secs: 10,
        driver_effort: None,
        advisory_gate: false,
        review_blocking: false,
        review_model: None,
        auto_rollback: true,
        shunt_driver_output: false,
    };

    let mut runner = DaemonRunner::new(config).unwrap();
    let report = runner.run_to_completion().unwrap();

    assert_eq!(report.status, "completed");
    assert!(root.join("a.txt").exists());
    assert!(root.join("b.txt").exists());
    // bad.txt must have been wiped by rollback and not present in final state
    assert!(!root.join("bad.txt").exists());
}
```

- [ ] **Step 2: Run test to verify it fails (RED)**

Run:
```bash
cargo test -p superstate-core --test test_daemon_dag_execution
```
Expected: Failures or unexpected advancement behavior.

- [ ] **Step 3: Update `DaemonRunner::run_step` in `crates/superstate-core/src/daemon.rs`**

Refine `daemon.rs` lines 960-985 to handle `advance_step(&mut state)` errors:
- If `advance_step` returns `Ok(())`:
  - Save updated state.
  - If `state.status == ExecutionStatus::Completed`, finalize execution.
  - Emit `DaemonStepOutcome::Advanced`.
- If `advance_step` returns `Err(StateError::InvalidTransition(msg))`:
  - Transition state to `ExecutionStatus::Blocked`.
  - Record note `[dag-blocked] Milestone {id} completed, but next task cannot advance: {msg}`.
  - Save updated state.
  - Emit `DaemonStepOutcome::Blocked { task_id: active_task.id, reason: msg }`.

- [ ] **Step 4: Verify test passes (GREEN)**

Run:
```bash
cargo test -p superstate-core --test test_daemon_dag_execution
cargo test -p superstate-core
```
Expected: All tests pass cleanly.

- [ ] **Step 5: Commit changes**

```bash
git add crates/superstate-core/src/daemon.rs crates/superstate-core/tests/test_daemon_dag_execution.rs
git commit --no-gpg-sign -m "feat(daemon): wire DAG milestone transitions, checkpoint commits, and scoped rollback"
```

---

### Task 3: CLI Integration & Verification

- **Depends on:** Task 1, Task 2

**Files:**
- Modify: `crates/superstate-cli/src/commands/state.rs:340-380`
- Test: `crates/superstate-cli/tests/test_cli_dag_daemon.rs`

**Interfaces:**
- Produces:
  - `superstate state init --milestones '<json>'` accepting arbitrary DAG milestone definitions with `id` and `depends_on`.
  - `superstate state daemon` running the initialized DAG to completion.

- [ ] **Step 1: Write failing CLI integration test**

Create `crates/superstate-cli/tests/test_cli_dag_daemon.rs`:
```rust
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn test_cli_daemon_executes_diamond_dag_milestones() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Initialize git repo
    Command::new("git").args(["init"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.name", "Test"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.email", "test@test.com"]).current_dir(root).output().unwrap();
    fs::write(root.join(".gitkeep"), "").unwrap();
    Command::new("git").args(["add", "."]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial commit"]).current_dir(root).output().unwrap();

    let milestones_json = r#"[
        {"id": "t1", "title": "Step 1", "verify_cmd": "test -f 1.txt", "depends_on": []},
        {"id": "t2a", "title": "Step 2A", "verify_cmd": "test -f 2a.txt", "depends_on": ["t1"]},
        {"id": "t2b", "title": "Step 2B", "verify_cmd": "test -f 2b.txt", "depends_on": ["t1"]},
        {"id": "t3", "title": "Step 3", "verify_cmd": "test -f 3.txt", "depends_on": ["t2a", "t2b"]}
    ]"#;

    // Run superstate state init --milestones ...
    let bin_path = env!("CARGO_BIN_EXE_superstate");
    let init_res = Command::new(bin_path)
        .args(["state", "init", "--milestones", milestones_json, "--no-worktree"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(init_res.status.success(), "state init failed: {}", String::from_utf8_lossy(&init_res.stderr));

    // Driver command creates corresponding file for active task
    let driver_script = r#"
case "$SUPERSTATE_ACTIVE_TASK" in
    t1) echo done > 1.txt ;;
    t2a) echo done > 2a.txt ;;
    t2b) echo done > 2b.txt ;;
    t3) echo done > 3.txt ;;
esac
"#;

    let daemon_res = Command::new(bin_path)
        .args(["state", "daemon", "--driver", driver_script, "--max-retries", "2", "--timeout", "10", "--auto-rollback"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(daemon_res.status.success(), "daemon failed: {}", String::from_utf8_lossy(&daemon_res.stderr));

    assert!(root.join("1.txt").exists());
    assert!(root.join("2a.txt").exists());
    assert!(root.join("2b.txt").exists());
    assert!(root.join("3.txt").exists());
}
```

- [ ] **Step 2: Run test to verify it fails (RED)**

Run:
```bash
cargo test -p superstate-cli --test test_cli_dag_daemon
```
Expected: Verify test fails before any fixes.

- [ ] **Step 3: Update `crates/superstate-cli/src/commands/state.rs` if needed**

Ensure `MilestoneSpec` deserialization from CLI JSON cleanly maps `id` and `depends_on`.

- [ ] **Step 4: Run test to verify it passes (GREEN)**

Run:
```bash
cargo test -p superstate-cli --test test_cli_dag_daemon
cargo test --all
```
Expected: All workspace tests pass cleanly.

- [ ] **Step 5: Commit changes**

```bash
git add crates/superstate-cli/ crates/superstate-core/
git commit --no-gpg-sign -m "feat(cli): add CLI integration tests for arbitrary DAG daemon execution"
```
