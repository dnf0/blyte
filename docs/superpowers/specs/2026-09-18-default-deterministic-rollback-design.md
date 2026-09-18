# Design Specification: Default Deterministic Git Rollback in Superstate Daemon

**Date:** 2026-09-18  
**Status:** Approved  
**Author:** AI Pair Assistant  
**Repository:** `superstate`  
**Reference Experiments:** EXP-058, EXP-059  

---

## 1. Background & Motivation

Evaluation EXP-059 conducted a pure head-to-head ablation between `superstate_daemon_rollback` (`--auto-rollback`) and `superstate_daemon_review` (no rollback, dirty tree retained across retries) on the 3-task Canary slice (`build-cython-ext`, `chess-best-move`, `httpx-multipart-response-parsing`):

* **Solve Rate:** `superstate_daemon_rollback` achieved **3/3 (100.0%)** vs `superstate_daemon_review` at **2/3 (66.7%)** (+33.3pp lift).
* **Token Efficiency:** Rollback used **25.3% fewer input tokens per solve** (683,307 vs 915,270).
* **Cost Efficiency:** Rollback lowered cost per solve by **25.5%** ($0.79 vs $1.06).
* **Mechanism:** On `httpx-multipart-response-parsing`, retaining dirty working-tree state across retries led to a subtle whitespace formatting bug (`assert '1\tz' == '1 z'`) from intermediate attempts. In contrast, resetting to the clean milestone baseline (`git reset --hard HEAD` and `git clean -fd`) while keeping the verifier's failure logs and review notes in the epistemic ledger allowed the worker to produce a 100% passing solution (122/122 F2P tests).

Retaining dirty state between failed verification attempts is an anti-pattern: it invites compounding debris and corrupts subsequent attempts. Deterministic rollback should be the default execution policy.

---

## 2. Goals & Non-Goals

### Goals
1. Make deterministic rollback the **default** behavior in `DaemonConfig::default()` and `DaemonRunner`.
2. Ensure serialized `DaemonConfig` without an explicit `auto_rollback` field deserializes with `auto_rollback = true`.
3. Provide a first-class `--no-auto-rollback` CLI flag on `superstate daemon` and `superstate state daemon`.
4. Support environment variable override (`SUPERSTATE_DAEMON_AUTO_ROLLBACK=0` or `false`) to disable rollback in automated environments.
5. Update all CLI and daemon integration tests to assert the new default and verify opt-out paths.

### Non-Goals
1. Modifying git worktree isolation or COW cache behaviors.
2. Modifying advisory code review or the epistemic ledger note schema.

---

## 3. Technical Design

### 3.1 Core Daemon Defaults (`crates/superstate-core/src/daemon.rs`)

1. Define serde helper:
   ```rust
   fn default_true() -> bool {
       true
   }
   ```
2. Update `DaemonConfig`:
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct DaemonConfig {
       // ...
       #[serde(default = "default_true")]
       pub auto_rollback: bool,
   }

   impl Default for DaemonConfig {
       fn default() -> Self {
           Self {
               state_path: PathBuf::from(".agent-state/state.json"),
               driver_cmd: None,
               max_retries: 3,
               timeout_secs: 120,
               advisory_gate: false,
               advisory_review: false,
               review_blocking: false,
               auto_rollback: true, // Default enabled
           }
       }
   }
   ```

### 3.2 CLI Interface & Opt-out (`crates/superstate-cli/src/state_cli.rs`)

In `DaemonArgs`:
```rust
#[derive(clap::Args, Debug, Clone)]
pub struct DaemonArgs {
    // ...
    /// Automatically roll back working tree to milestone baseline on verification failure (default: true)
    #[arg(
        long,
        help = "Automatically roll back working tree to milestone baseline on verification failure (default: true)"
    )]
    pub auto_rollback: bool,

    /// Disable automatic git rollback on verification failure
    #[arg(
        long,
        help = "Disable automatic git rollback on verification failure"
    )]
    pub no_auto_rollback: bool,
}

impl DaemonArgs {
    pub fn use_auto_rollback(&self) -> bool {
        if self.no_auto_rollback {
            false
        } else if let Ok(val) = std::env::var("SUPERSTATE_DAEMON_AUTO_ROLLBACK") {
            val != "0" && !val.eq_ignore_ascii_case("false")
        } else {
            true
        }
    }
}
```

### 3.3 Command Dispatch (`crates/superstate-cli/src/commands/daemon.rs`)

In `dispatch_daemon`:
```rust
let auto_rollback = args.use_auto_rollback();
```
Wire `auto_rollback` directly into `DaemonConfig.auto_rollback`.

---

## 4. Test Strategy

1. **`crates/superstate-core/tests/test_daemon_rollback.rs`**:
   - Verify `DaemonConfig::default().auto_rollback == true`.
   - Verify `DaemonRunner::step` performs rollback by default when `DaemonConfig` is initialized with `DaemonConfig::default()`.
   - Verify setting `auto_rollback: false` preserves dirty working tree and untracked files.
2. **`crates/superstate-cli/tests/test_cli_rollback.rs`**:
   - Verify `superstate daemon` without `--auto-rollback` or `--no-auto-rollback` parses `use_auto_rollback() == true`.
   - Verify passing `--no-auto-rollback` parses `use_auto_rollback() == false`.
   - Verify setting `SUPERSTATE_DAEMON_AUTO_ROLLBACK=0` disables rollback.
   - Verify CLI execution tests confirm automatic rollback by default, and persistence of dirty state when `--no-auto-rollback` is supplied.

---

## 5. Security & Safety

- Deterministic rollback uses `git reset --hard HEAD` and `git clean -fd`.
- Only runs when `is_inside_git_repo(working_dir)` is true.
- As confirmed across EXP-058 and EXP-059, the verifier output and error traces are recorded in the epistemic ledger (`state.json`) *before* rollback, ensuring the agent retains complete error context without suffering file pollution.
