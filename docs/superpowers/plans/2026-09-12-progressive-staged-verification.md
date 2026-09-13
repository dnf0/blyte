# Progressive Staged Verification & Regression Quarantine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement two-tiered progressive verification and regression quarantine across Superstate worktrees and MCP tools, allowing fast per-milestone iteration while strictly preventing P2P regressions upon merge.

**Architecture:** Extend `worktree_merge` to enforce an optional full regression suite before touching the base branch, extend `state_next` to support staged verification (`milestone` vs `final`), and provide impact-aware test slicing that infers affected unit tests from `git diff`.

**Tech Stack:** Rust (edition 2021), `superstate_core`, `superstate_worktree`, `superstate_mcp`, `serde_json`, `git2`, `cargo test`.

## Global Constraints

- Never modify base branch (`main` / `master`) if pre-merge verification fails; quarantine failures inside `.wt/<branch>`.
- All MCP tools must return structured JSON or clean JSON-RPC errors, never panic or unwrap.
- Respect branch-scoped state routing and `.agent-state/` git exclusions.
- TDD required: write failing integration tests before implementing each handler.
- Code style: clean `cargo fmt` and zero `cargo clippy -p superstate-mcp -p superstate-worktree -p superstate-core` warnings.

---

### Task 1: Worktree Pre-Merge Verification Gate

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-worktree/src/manager.rs:379-405`
- Modify: `crates/superstate-mcp/src/server.rs:1551-1582`
- Test: `crates/superstate-mcp/tests/test_mcp_worktree_premerge_gate.rs`

**Interfaces:**
- Consumes: `IsolatedWorktree`, `execute_preflight`
- Produces: `worktree_merge(verify_full_suite: Option<String>) -> Result<Value, String>`

- [ ] **Step 1: Write failing integration test for `worktree_merge` with `verify_full_suite`**

Create `crates/superstate-mcp/tests/test_mcp_worktree_premerge_gate.rs`:
```rust
use serde_json::json;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn test_worktree_merge_blocks_on_failing_full_suite() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Initialize git repository
    Command::new("git").args(["init"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.name", "Test"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.email", "test@test.com"]).current_dir(root).output().unwrap();
    fs::write(root.join("file.txt"), "hello").unwrap();
    Command::new("git").args(["add", "."]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial"]).current_dir(root).output().unwrap();

    // Create a worktree branch
    let manager = superstate_worktree::WorktreeManager::new(root).unwrap();
    let wt = manager.ensure_task_worktree("task-1", Some("main")).unwrap();

    // Modify a file in the worktree
    fs::write(wt.path.join("file.txt"), "broken changes").unwrap();

    // Call MCP worktree_merge with a failing verify_full_suite command
    let args = json!({
        "repo_root": root.to_str().unwrap(),
        "branch": wt.branch,
        "verify_full_suite": "false" // Command that exits with code 1
    });

    let res = superstate_mcp::server::dispatch_mcp_tool("worktree_merge", args);
    assert!(res.is_err(), "Merge must fail when verify_full_suite command fails");
    let err_msg = res.unwrap_err();
    assert!(err_msg.contains("Pre-merge verification failed") || err_msg.contains("Verification gate"));

    // Verify base branch was not merged
    let content = fs::read_to_string(root.join("file.txt")).unwrap();
    assert_eq!(content, "hello", "Base branch must remain untouched on verification failure");

    // Verify worktree still exists for remediation
    assert!(wt.path.exists(), "Worktree must be preserved for remediation");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-mcp --test test_mcp_worktree_premerge_gate`
Expected: FAIL (unrecognized argument or does not block merge).

- [ ] **Step 3: Implement pre-merge verification in `server.rs`**

In `crates/superstate-mcp/src/server.rs`, update `"worktree_merge"` handler:
```rust
        "worktree_merge" => {
            let branch = args
                .get("branch")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required argument 'branch'".to_string())?;

            let repo_root = args
                .get("repo")
                .or_else(|| args.get("repo_root"))
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            let base = args.get("base").and_then(|v| v.as_str());
            let message = args
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Merge worktree branch");

            let verify_full_suite = args.get("verify_full_suite").and_then(|v| v.as_str());

            let isolated = resolve_worktree_for_merge(&repo_root, branch, base)?;
            let _ = isolated.auto_commit_if_dirty(message);

            // Execute full regression suite in the worktree prior to merging
            if let Some(cmd) = verify_full_suite {
                let gate_res = execute_preflight(cmd, &isolated.path)?;
                if gate_res.exit_code != 0 {
                    return Err(format!(
                        "Pre-merge verification failed (exit code {}):\nSTDOUT:\n{}\nSTDERR:\n{}",
                        gate_res.exit_code, gate_res.stdout, gate_res.stderr
                    ));
                }
            }

            isolated
                .merge_into_base(message)
                .map_err(|e| format!("Failed to merge worktree: {}", e))?;

            Ok(serde_json::json!({
                "status": "success",
                "branch": branch,
                "base": isolated.base_branch,
            }))
        }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-mcp --test test_mcp_worktree_premerge_gate`
Expected: PASS.

- [ ] **Step 5: Commit changes**

```bash
git add crates/superstate-mcp/src/server.rs crates/superstate-mcp/tests/test_mcp_worktree_premerge_gate.rs
git commit -m "feat(mcp): implement worktree_merge pre-merge regression verification gate"
```

---

### Task 2: Tiered Verification in `state_next`

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-mcp/src/server.rs:656-726`
- Test: `crates/superstate-mcp/tests/test_mcp_tiered_state_next.rs`

**Interfaces:**
- Consumes: `state_next`, `verify_stage`
- Produces: Tiered gate execution (fast milestone vs full final gate)

- [ ] **Step 1: Write failing integration test for tiered `state_next`**

Create `crates/superstate-mcp/tests/test_mcp_tiered_state_next.rs`:
```rust
use serde_json::json;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn test_state_next_tiered_verification_final_stage() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    Command::new("git").args(["init"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.name", "Test"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.email", "test@test.com"]).current_dir(root).output().unwrap();
    fs::write(root.join("README.md"), "test").unwrap();
    Command::new("git").args(["add", "."]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "init"]).current_dir(root).output().unwrap();

    // Initialize 2-milestone state
    let init_args = json!({
        "repo_root": root.to_str().unwrap(),
        "milestones": [
            {"id": "task-1", "title": "M1", "verify_cmd": "echo m1"},
            {"id": "task-2", "title": "M2", "verify_cmd": "echo m2"}
        ],
        "no_worktree": true
    });
    superstate_mcp::server::dispatch_mcp_tool("superstate_handoff", init_args).unwrap();

    // Advance task-1 with milestone stage (fast verify)
    let next_args = json!({
        "repo_root": root.to_str().unwrap(),
        "verify_cmd": "true",
        "verify_stage": "milestone"
    });
    let res = superstate_mcp::server::dispatch_mcp_tool("state_next", next_args).unwrap();
    assert_eq!(res["status"], "success");
    assert_eq!(res["active_task"]["id"], "task-2");

    // Advance task-2 with final stage and failing full suite command
    let final_args = json!({
        "repo_root": root.to_str().unwrap(),
        "verify_cmd": "false",
        "verify_stage": "final"
    });
    let final_res = superstate_mcp::server::dispatch_mcp_tool("state_next", final_args);
    assert!(final_res.is_err(), "Final stage must enforce full regression check");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-mcp --test test_mcp_tiered_state_next`
Expected: FAIL.

- [ ] **Step 3: Implement `verify_stage` handling in `server.rs`**

In `crates/superstate-mcp/src/server.rs`, update `state_next` to inspect `verify_stage` (`"milestone"` vs `"final"`):
```rust
            let verify_stage = args
                .get("verify_stage")
                .and_then(|v| v.as_str())
                .unwrap_or("milestone");
```
When `verify_stage == "final"` or state is on the terminal task without next tasks:
Log milestone stage and enforce full task verification command or provided `verify_cmd`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-mcp --test test_mcp_tiered_state_next`
Expected: PASS.

- [ ] **Step 5: Commit changes**

```bash
git add crates/superstate-mcp/src/server.rs crates/superstate-mcp/tests/test_mcp_tiered_state_next.rs
git commit -m "feat(mcp): support verify_stage in state_next for tiered progressive verification"
```

---

### Task 3: Automated Test Slicing & Impact Analysis

- **Depends on:** Task 2

**Files:**
- Create: `crates/superstate-core/src/impact.rs`
- Modify: `crates/superstate-core/src/lib.rs`
- Test: `crates/superstate-core/tests/test_impact_analysis.rs`

**Interfaces:**
- Produces: `pub fn infer_impacted_tests(repo_root: &Path, base_branch: Option<&str>) -> Vec<PathBuf>`

- [ ] **Step 1: Write failing test for impact analysis**

Create `crates/superstate-core/tests/test_impact_analysis.rs`:
```rust
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn test_infer_impacted_tests_discovers_adjacent_test_file() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    Command::new("git").args(["init"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.name", "Test"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.email", "test@test.com"]).current_dir(root).output().unwrap();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(root.join("src/auth.py"), "def login(): pass").unwrap();
    fs::write(root.join("tests/test_auth.py"), "def test_login(): pass").unwrap();

    Command::new("git").args(["add", "."]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "init"]).current_dir(root).output().unwrap();

    // Create a feature branch and modify src/auth.py
    Command::new("git").args(["checkout", "-b", "feat/new-auth"]).current_dir(root).output().unwrap();
    fs::write(root.join("src/auth.py"), "def login(): return True").unwrap();

    let impacted = superstate_core::impact::infer_impacted_tests(root, Some("main"));
    assert_eq!(impacted.len(), 1);
    assert_eq!(impacted[0], root.join("tests/test_auth.py"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p superstate-core --test test_impact_analysis`
Expected: FAIL (module not found).

- [ ] **Step 3: Implement `infer_impacted_tests` in `crates/superstate-core/src/impact.rs`**

```rust
use std::path::{Path, PathBuf};
use std::process::Command;

/// Inspect git diff against base branch and discover test files covering modified source files.
pub fn infer_impacted_tests(repo_root: &Path, base_branch: Option<&str>) -> Vec<PathBuf> {
    let base = base_branch.unwrap_or("main");
    let output = match Command::new("git")
        .args(["diff", "--name-only", base])
        .current_dir(repo_root)
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return Vec::new(),
    };

    let mut tests = Vec::new();
    for line in output.lines() {
        let p = Path::new(line.trim());
        if let Some(file_stem) = p.file_stem().and_then(|s| s.to_str()) {
            let candidate_1 = repo_root.join("tests").join(format!("test_{}.py", file_stem));
            let candidate_2 = repo_root.join(format!("tests/test_{}.rs", file_stem));
            let candidate_3 = repo_root.join(format!("{}_test.go", p.display()));

            if candidate_1.exists() {
                tests.push(candidate_1);
            } else if candidate_2.exists() {
                tests.push(candidate_2);
            } else if candidate_3.exists() {
                tests.push(candidate_3);
            }
        }
    }
    tests.sort();
    tests.dedup();
    tests
}
```
Export in `crates/superstate-core/src/lib.rs`: `pub mod impact;`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p superstate-core --test test_impact_analysis`
Expected: PASS.

- [ ] **Step 5: Commit changes**

```bash
git add crates/superstate-core/src/impact.rs crates/superstate-core/src/lib.rs crates/superstate-core/tests/test_impact_analysis.rs
git commit -m "feat(core): implement infer_impacted_tests for automatic test slicing"
```

---

### Task 4: MCP Schema Parity & End-to-End Verification

- **Depends on:** Task 1, Task 2, Task 3

**Files:**
- Modify: `crates/superstate-mcp/src/tools.rs`
- Modify: `crates/superstate-mcp/src/server.rs`
- Test: Full cargo suite `cargo test --all`

- [ ] **Step 1: Update MCP tool schemas in `tools.rs`**

Add `verify_full_suite` to `worktree_merge` properties:
```rust
"verify_full_suite": json!({
    "type": "string",
    "description": "Optional full regression suite command to execute inside the worktree prior to merging. If non-zero, merge is aborted."
})
```
Add `verify_stage` to `state_next` properties:
```rust
"verify_stage": json!({
    "type": "string",
    "enum": ["milestone", "final"],
    "description": "Stage of verification: 'milestone' for fast unit iteration, 'final' for comprehensive regression enforcement."
})
```

- [ ] **Step 2: Run full test suite and clippy**

Run: `cargo test --all`
Run: `cargo clippy --all -- -D warnings`
Run: `cargo fmt --all -- --check`
Expected: All tests pass with zero warnings.

- [ ] **Step 3: Commit changes**

```bash
git add crates/superstate-mcp/src/tools.rs
git commit -m "feat(mcp): update tool schemas with verify_full_suite and verify_stage parameters"
```
