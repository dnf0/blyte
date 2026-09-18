# Rapid Canary Evaluation and Deterministic Daemon Rollback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provide a 12–15 minute Canary evaluation suite (`scripts/run_canary.sh`) and implement deterministic git rollback (`--auto-rollback`) in `superstate` daemon to prevent compounding errors across retry turns.

**Architecture:** 
- In `superstate-eval`, define a 3-task Canary slice (`build-cython-ext`, `chess-best-move`, `httpx-multipart-response-parsing`) wired to `--canary` in `scripts/paired_run.py`.
- In `superstate`, add `auto_rollback: bool` to `DaemonConfig` and `--auto-rollback` CLI flag. When a verification gate fails or review blocks, `DaemonRunner` executes `git reset --hard <start_commit>` and `git clean -fd`, appending an epistemic `[rollback]` note before the next retry attempt.
- In `superstate-eval`, wire `SuperstateDaemonAdapter` to forward `--auto-rollback`.

**Tech Stack:** Rust (1.80+), Python (3.12), Git, Docker.

## Global Constraints
- All Rust changes must compile with `cargo check --all` and pass `cargo test --all` with zero warnings.
- All Python changes must pass `ruff check` and `pytest`.
- Rollback must never destroy uncommitted work outside the milestone boundary: only resets to `milestone_start_commits[active_task.id]`.
- Do not add any new external crate dependencies.

---

### Task 1: Canary 3 Task Set & Evaluation Runner in `superstate-eval`

- **Depends on:** none

**Files:**
- Modify: `/Users/danielfisher/repos/superstate-eval/lab/benchmarks/task_sets.py`
- Modify: `/Users/danielfisher/repos/superstate-eval/scripts/paired_run.py`
- Create: `/Users/danielfisher/repos/superstate-eval/scripts/run_canary.sh`
- Test: `/Users/danielfisher/repos/superstate-eval/tests/test_canary_task_set.py`

**Interfaces:**
- Produces: `CANARY_TASKS: tuple[str, ...]` and `is_canary(task_name: str) -> bool` in `lab/benchmarks/task_sets.py`.
- Produces: `--canary` flag in `scripts/paired_run.py`.

- [ ] **Step 1: Write the failing test for Canary task set**

```python
# tests/test_canary_task_set.py
from lab.benchmarks.task_sets import CANARY_TASKS, is_canary, is_discriminating

def test_canary_tasks_defined_and_subset_of_discriminating():
    assert len(CANARY_TASKS) == 3
    assert "build-cython-ext" in CANARY_TASKS
    assert "chess-best-move" in CANARY_TASKS
    assert "httpx-multipart-response-parsing" in CANARY_TASKS
    for task in CANARY_TASKS:
        assert is_canary(task)
        assert is_discriminating(task)

def test_non_canary_task():
    assert not is_canary("openssl-selfsigned-cert")
    assert not is_canary("scc-bounded-memory-spilling")
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_canary_task_set.py`
Expected: FAIL with `ImportError: cannot import name 'CANARY_TASKS'`

- [ ] **Step 3: Implement `CANARY_TASKS` in `task_sets.py` and `--canary` in `paired_run.py`**

In `lab/benchmarks/task_sets.py`:
```python
CANARY_TASKS: tuple[str, ...] = (
    "build-cython-ext",
    "chess-best-move",
    "httpx-multipart-response-parsing",
)

def is_canary(task_name: str) -> bool:
    """Whether a task belongs to the fast 3-task Canary slice."""
    return task_name in CANARY_TASKS
```

In `scripts/paired_run.py`, add `--canary` argument:
```python
parser.add_argument(
    "--canary",
    action="store_true",
    help="Run the fast 3-task Canary slice (~15 min total)",
)
```
When `--canary` is set, filter tasks to `CANARY_TASKS`.

- [ ] **Step 4: Create executable `scripts/run_canary.sh`**

```bash
#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")/.."

ARMS=${1:-"superstate_daemon_review superstate_mcp"}
STAMP=$(date +%Y%m%d-%H%M%S)
OUT="results/canary_${STAMP}.json"

echo "Starting Canary 3 evaluation: $ARMS"
echo "Concurrency: 2"
echo "Output: $OUT"

.venv/bin/python scripts/paired_run.py \
  --arms $ARMS \
  --canary \
  --driver agy \
  --model gemini-3.8-flash-high \
  --concurrency 2 \
  --out "$OUT"
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_canary_task_set.py`
Expected: PASS (2 passed)

- [ ] **Step 6: Commit**

```bash
git add lab/benchmarks/task_sets.py scripts/paired_run.py scripts/run_canary.sh tests/test_canary_task_set.py
git commit --no-gpg-sign -m "feat(eval): add canary 3 task set and runner script"
```

---

### Task 2: Deterministic Daemon Rollback in `crates/superstate-core`

- **Depends on:** none

**Files:**
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-core/src/daemon.rs`
- Test: `/Users/danielfisher/repos/superstate/crates/superstate-core/tests/test_daemon_rollback.rs`

**Interfaces:**
- Consumes: `milestone_start_commits: HashMap<String, String>` from `DaemonRunner`.
- Produces: `pub auto_rollback: bool` on `DaemonConfig`.

- [ ] **Step 1: Write the failing integration test for rollback**

```rust
// crates/superstate-core/tests/test_daemon_rollback.rs
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

use superstate_core::daemon::{DaemonConfig, DaemonRunner, DaemonStepOutcome};
use superstate_core::state::{ExecutionStatus, StateFile, TaskItem};

fn init_git_repo(path: &Path) {
    Command::new("git").args(["init"]).current_dir(path).output().unwrap();
    Command::new("git").args(["config", "user.name", "Test"]).current_dir(path).output().unwrap();
    Command::new("git").args(["config", "user.email", "test@example.com"]).current_dir(path).output().unwrap();
    fs::write(path.join("README.md"), "initial\n").unwrap();
    Command::new("git").args(["add", "README.md"]).current_dir(path).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial"]).current_dir(path).output().unwrap();
}

#[test]
fn test_daemon_auto_rollback_cleans_dirty_changes_and_untracked_files_on_failure() {
    let dir = tempdir().unwrap();
    let repo_path = dir.path();
    init_git_repo(repo_path);

    let state_dir = repo_path.join(".agent-state");
    fs::create_dir_all(&state_dir).unwrap();
    let state_path = state_dir.join("state.json");

    let task = TaskItem {
        id: "task-1".to_string(),
        title: "Test rollback task".to_string(),
        status: None,
        depends_on: vec![],
        files_modify: vec![],
        verification_cmd: Some("test -f expected.txt".to_string()),
    };

    let mut state = StateFile::new("Test goal".to_string());
    state.active_task = Some(task);
    state.status = ExecutionStatus::Running;
    superstate_core::state::save_state_atomic(&state_path, &state).unwrap();

    // Driver modifies existing file and creates untracked garbage, then exits 0 (but verification gate will fail)
    let driver_script = repo_path.join("dirty_driver.sh");
    fs::write(
        &driver_script,
        "#!/bin/sh\necho 'corrupted' >> README.md\necho 'garbage' > untracked.txt\nexit 0\n",
    ).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&driver_script, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let config = DaemonConfig {
        state_path: state_path.clone(),
        driver_cmd: Some(driver_script.to_str().unwrap().to_string()),
        max_retries: 2,
        timeout_secs: 10,
        advisory_gate: false,
        advisory_review: false,
        review_blocking: false,
        auto_rollback: true,
    };

    let mut runner = DaemonRunner::new(config);
    let outcome = runner.step().unwrap();

    assert!(matches!(outcome, DaemonStepOutcome::Failed { .. }));

    // Verify README.md was rolled back to pristine baseline
    let content = fs::read_to_string(repo_path.join("README.md")).unwrap();
    assert_eq!(content, "initial\n");

    // Verify untracked garbage was cleaned
    assert!(!repo_path.join("untracked.txt").exists());

    // Verify [rollback] note was recorded
    let updated_state = superstate_core::state::load_state(&state_path).unwrap();
    let has_rollback_note = updated_state.notes.iter().any(|n| n.note.starts_with("[rollback]"));
    assert!(has_rollback_note, "Expected [rollback] note in state.notes");
}
```

- [ ] **Step 2: Run test to verify it fails to compile**

Run: `cargo test -p superstate-core --test test_daemon_rollback`
Expected: FAIL (`unknown field auto_rollback in struct DaemonConfig`)

- [ ] **Step 3: Implement `auto_rollback` in `crates/superstate-core/src/daemon.rs`**

1. In `DaemonConfig`:
   ```rust
   pub auto_rollback: bool,
   ```
   Add default `auto_rollback: false`.
2. In `DaemonRunner::step`:
   When verification fails (`!gate_res.passed` or review blocks) AND `attempts_so_far < attempt_ceiling`:
   ```rust
   if self.config.auto_rollback && is_inside_git_repo(&working_dir) {
       if let Some(start_commit) = self.milestone_start_commits.get(&active_task.id) {
           let _ = Command::new("git")
               .args(["reset", "--hard", start_commit])
               .current_dir(&working_dir)
               .output();
           let _ = Command::new("git")
               .args(["clean", "-fd"])
               .current_dir(&working_dir)
               .output();
           state.notes.push(LedgerEntry::new(
               "superstate-daemon",
               format!(
                   "[rollback] Task {}: attempt {} failed verification; rolled back working tree to baseline commit {}. Avoid repeating previous failed approach.",
                   active_task.id, attempts_so_far + 1, start_commit
               ),
           ));
           save_state_atomic(&self.config.state_path, &state)?;
       }
   }
   ```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-core --test test_daemon_rollback`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/superstate-core/src/daemon.rs crates/superstate-core/tests/test_daemon_rollback.rs
git commit --no-gpg-sign -m "feat(daemon): implement deterministic git rollback on verification failure"
```

---

### Task 3: CLI Flag `--auto-rollback` in `crates/superstate-cli`

- **Depends on:** Task 2

**Files:**
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-cli/src/state_cli.rs`
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate-cli/src/commands/daemon.rs`
- Test: `/Users/danielfisher/repos/superstate/crates/superstate-cli/tests/test_cli_rollback.rs`

**Interfaces:**
- Produces: `--auto-rollback` flag on `superstate daemon` and `superstate state daemon`.

- [ ] **Step 1: Write the failing CLI integration test**

```rust
// crates/superstate-cli/tests/test_cli_rollback.rs
use clap::Parser;
use superstate_cli::state_cli::{Cli, Commands};

#[test]
fn test_cli_parses_auto_rollback_flag() {
    let args = vec!["superstate", "state", "daemon", "--auto-rollback"];
    let cli = Cli::try_parse_from(args).unwrap();
    match cli.command {
        Commands::State(superstate_cli::state_cli::StateCommands::Daemon(dargs)) => {
            assert!(dargs.auto_rollback);
        }
        _ => panic!("Expected StateCommands::Daemon"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-cli --test test_cli_rollback`
Expected: FAIL (`no field auto_rollback on DaemonArgs`)

- [ ] **Step 3: Implement `--auto-rollback` flag**

In `crates/superstate-cli/src/state_cli.rs`:
```rust
#[arg(long, help = "Automatically roll back working tree to milestone baseline on verification failure")]
pub auto_rollback: bool,
```
In `crates/superstate-cli/src/commands/daemon.rs`:
- Map `args.auto_rollback` to `DaemonConfig.auto_rollback`.
- If `args.detach`, append `"--auto-rollback"` to `daemon_args` when spawning detached child.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-cli --test test_cli_rollback`
Expected: PASS

- [ ] **Step 5: Rebuild and reinstall release binary**

```bash
cargo install --path crates/superstate-cli --locked --force
```

- [ ] **Step 6: Commit**

```bash
git add crates/superstate-cli/src/state_cli.rs crates/superstate-cli/src/commands/daemon.rs crates/superstate-cli/tests/test_cli_rollback.rs
git commit --no-gpg-sign -m "feat(cli): wire --auto-rollback flag and detachment forwarding"
```

---

### Task 4: Eval Adapter Wiring & Canary Baseline in `superstate-eval`

- **Depends on:** Task 1, Task 3

**Files:**
- Modify: `/Users/danielfisher/repos/superstate-eval/lab/engine/adapters/superstate_daemon.py`
- Test: `/Users/danielfisher/repos/superstate-eval/tests/test_superstate_daemon_adapter.py`

**Interfaces:**
- Produces: `SUPERSTATE_EVAL_AUTO_ROLLBACK` env flag and `auto_rollback` configuration on `SuperstateDaemonAdapter`.

- [ ] **Step 1: Write failing test in `tests/test_superstate_daemon_adapter.py`**

```python
def test_superstate_daemon_adapter_passes_auto_rollback_flag():
    adapter = SuperstateDaemonAdapter(driver=MockDriver(), auto_rollback=True)
    cmd = adapter._build_daemon_spawn_cmd(Path("/tmp/state.json"), Path("/tmp/workspace"))
    assert "--auto-rollback" in cmd
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_superstate_daemon_adapter.py -k test_superstate_daemon_adapter_passes_auto_rollback_flag`
Expected: FAIL

- [ ] **Step 3: Implement `auto_rollback` in `superstate_daemon.py`**

In `lab/engine/adapters/superstate_daemon.py`:
- Add `AUTO_ROLLBACK_ENV_FLAG = "SUPERSTATE_EVAL_AUTO_ROLLBACK"`.
- Support `auto_rollback: Optional[bool] = None` in `SuperstateDaemonAdapter.__init__`.
- In `_spawn_daemon`, append `"--auto-rollback"` if enabled.

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_superstate_daemon_adapter.py`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add lab/engine/adapters/superstate_daemon.py tests/test_superstate_daemon_adapter.py
git commit --no-gpg-sign -m "feat(eval): wire auto_rollback into superstate daemon adapter"
```
