# Design Specification: Superstate MCP and CLI Complete Parity

- **Date:** 2026-09-12
- **Author:** Antigravity & Dan Fisher
- **Status:** Approved
- **Target Repository:** `climate-x-org/superstate` (`crates/superstate-mcp`, `crates/superstate-core`, `crates/superstate-worktree`)

---

## 1. Executive Summary & Objective

Superstate provides a deterministic state engine, Git worktree isolation, and verification gates designed for agentic runtimes. While the CLI (`superstate state ...` and `superstate worktree ...`) offers a complete set of inspection, recovery, maintenance, and execution commands, the Model Context Protocol (MCP) server currently exposes only a subset.

This specification closes the capability gap by establishing **100% complete parity** between the Superstate CLI command surface and the Superstate MCP tool surface. Any operation an operator or developer can execute via the shell can now be invoked directly and deterministically by an LLM agent over standard MCP JSON-RPC.

---

## 2. Architectural Boundaries & Principles

1. **Shared Core Execution**: MCP tool handlers must not duplicate CLI business logic. Instead, MCP handlers in `crates/superstate-mcp/src/server.rs` and CLI commands in `crates/superstate-cli/src/commands/` must delegate directly to shared routines in `superstate_core` and `superstate_worktree`.
2. **Branch-Scoped Resolution**: All state inspection and modification tools must respect branch-scoped state routing (`branch_scoped_state_path` and `find_branch_state_for_worktree`), allowing concurrent agents on separate branches to operate without mutual interference.
3. **Structured & Human-Readable Responses**: MCP responses should provide structured JSON for programmatic consumers, accompanied by clean markdown/text content in `content[0].text` suitable for agent conversational consumption.
4. **Preserved Invariants & Safety**: Rollback and clean operations must explicitly preserve `.agent-state/` and untracked telemetry files, preventing accidental wiping of error logs or progress records.

---

## 3. Tool Matrix & Parity Mapping

### 3.1 Existing Tools Parameter Alignment

| MCP Tool | Added Parameters | Purpose |
|---|---|---|
| `state_next` | `ignore_untracked_glob: Option<String>` | Enables passing glob patterns (e.g. `*.pyc`, `cache/**`) to ignore untracked files during `verify_git_clean`. |
| `state_prompt` | `with_graph: Option<bool>`, `task: Option<String>` | Enables injecting graph knowledge context and targeting specific tasks in the pending set by ID or title. |
| `state_init` | *(Already updated)* | Defaults to in-place (`no_worktree = true`), supports `--worktree` flag, direct milestones, and goal. |
| `superstate_handoff` | *(Already updated)* | Supports `worktree: bool`, `no_worktree: bool`, and multi-milestone arrays. |

### 3.2 State Inspection & Briefing Tools

#### `state_status`
- **CLI Equivalent:** `superstate state status [--json]`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "state": { "type": "string", "description": "Optional path to state JSON file" },
      "json": { "type": "boolean", "description": "Emit machine-readable JSON status" }
    }
  }
  ```
- **Returns:** Status summary containing `status`, `step`, `active_task`, completed task count, ledger note count, and recent notes.

#### `state_show`
- **CLI Equivalent:** `superstate state show [--json] [--markdown]`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "state": { "type": "string", "description": "Optional path to state JSON file" },
      "json": { "type": "boolean", "description": "Emit machine-readable JSON" }
    }
  }
  ```
- **Returns:** Formatted markdown or structured JSON representation of the entire state machine, active worktree, base branch, and verification invariant.

#### `state_resume`
- **CLI Equivalent:** `superstate state resume`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "state": { "type": "string", "description": "Optional path to state JSON file" }
    }
  }
  ```
- **Returns:** Executive briefing for resuming after context loss or window compaction: active task, recent Git commits, epistemic ledger notes, and gate status.

#### `state_metrics`
- **CLI Equivalent:** `superstate state metrics [--json]`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "state": { "type": "string", "description": "Optional path to state JSON file" },
      "json": { "type": "boolean", "description": "Emit machine-readable JSON" }
    }
  }
  ```
- **Returns:** Progress telemetry: `step`, `status`, `tasks_completed`, `tasks_total`, `completion_pct`, `tasks_remaining`, `notes_recorded`, `errors_recorded`.

---

### 3.3 Safety, Checkpointing & Atomic Execution Tools

#### `state_checkpoint`
- **CLI Equivalent:** `superstate state checkpoint`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "state": { "type": "string", "description": "Optional path to state JSON file" }
    }
  }
  ```
- **Returns:** Created Git checkpoint tag (e.g. `agent-checkpoint/step-1`).

#### `state_rollback`
- **CLI Equivalent:** `superstate state rollback [--reference <tag>]`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "state": { "type": "string", "description": "Optional path to state JSON file" },
      "reference": { "type": "string", "description": "Optional checkpoint reference tag to restore (defaults to current step)" }
    }
  }
  ```
- **Returns:** Aborts active worktree (if any), restores working tree to checkpoint tag, and removes untracked files while preserving `.agent-state`.

#### `state_exec`
- **CLI Equivalent:** `superstate state exec -v <cmd> [--checkpoint] [--rollback-on-fail] [--no-advance] [--verify-git-clean]`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "verify_cmd": { "type": "string", "description": "Shell command to verify" },
      "state": { "type": "string", "description": "Optional path to state JSON file" },
      "verify_git_clean": { "type": "boolean", "description": "Require clean git working tree" },
      "checkpoint": { "type": "boolean", "description": "Create checkpoint before running verification" },
      "rollback_on_fail": { "type": "boolean", "description": "Restore checkpoint if verification fails" },
      "advance": { "type": "boolean", "description": "Advance state machine on verification success (default: true)" }
    },
    "required": ["verify_cmd"]
  }
  ```
- **Returns:** Atomic verification result: gate pass/fail, whether state advanced, and rollback outcome.

---

### 3.4 Maintenance, Reconciliation & DAG Tools

#### `state_ready`
- **CLI Equivalent:** `superstate state ready [--json]`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "state": { "type": "string", "description": "Optional path to state JSON file" },
      "json": { "type": "boolean", "description": "Emit machine-readable JSON array of ready task titles" }
    }
  }
  ```
- **Returns:** Tasks in DAG whose dependencies have been satisfied and are ready for execution.

#### `state_sync`
- **CLI Equivalent:** `superstate state sync --plan <plan.md>`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "plan": { "type": "string", "description": "Path to revised implementation plan markdown" },
      "state": { "type": "string", "description": "Optional path to state JSON file" },
      "out": { "type": "string", "description": "Optional destination path for synced state" }
    },
    "required": ["plan"]
  }
  ```
- **Returns:** Sync report: added tasks, removed tasks, preserved completed tasks.

#### `state_doctor`
- **CLI Equivalent:** `superstate state doctor [--plan <plan.md>] [--repair]`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "state": { "type": "string", "description": "Optional path to state JSON file" },
      "plan": { "type": "string", "description": "Optional path to plan markdown file" },
      "repair": { "type": "boolean", "description": "Attempt automated repair of detected issues" },
      "json": { "type": "boolean", "description": "Emit machine-readable JSON" }
    }
  }
  ```
- **Returns:** Health report: `is_healthy`, issues list, warnings list, and repaired actions list.

#### `state_validate`
- **CLI Equivalent:** `superstate state validate`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "state": { "type": "string", "description": "Optional path to state JSON file" }
    }
  }
  ```
- **Returns:** Validation outcome: valid or list of schema violations.

#### `state_apply`
- **CLI Equivalent:** `superstate state apply [--patch <patch.json>] [--note <note>]`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "state": { "type": "string", "description": "Optional path to state JSON file" },
      "patch": { "type": "object", "description": "JSON patch object to apply to state" },
      "note": { "type": "string", "description": "Optional finding or note to append to epistemic ledger" },
      "out": { "type": "string", "description": "Optional path to write patched state" }
    }
  }
  ```
- **Returns:** Confirmation that patch was validated and written, and/or note recorded.

#### `state_list`
- **CLI Equivalent:** `superstate state list [--json]`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "state": { "type": "string", "description": "Optional path to state file or directory to scan" },
      "json": { "type": "boolean", "description": "Emit machine-readable JSON" }
    }
  }
  ```
- **Returns:** List of active branch-scoped state files and associated worktree directories.

---

### 3.5 Worktree Lifecycle Tools

#### `worktree_list`
- **CLI Equivalent:** `superstate worktree list`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "repo": { "type": "string", "description": "Repository root path (defaults to current directory)" }
    }
  }
  ```
- **Returns:** List of active worktrees with branches, paths, and HEAD commits.

#### `worktree_remove`
- **CLI Equivalent:** `superstate worktree remove <branch> [--force]`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "branch": { "type": "string", "description": "Branch name of worktree to remove" },
      "force": { "type": "boolean", "description": "Force removal even if worktree has uncommitted changes" },
      "repo": { "type": "string", "description": "Repository root path" }
    },
    "required": ["branch"]
  }
  ```
- **Returns:** Confirmation that worktree and branch were removed.

#### `worktree_clean`
- **CLI Equivalent:** `superstate worktree clean`
- **Schema:**
  ```json
  {
    "type": "object",
    "properties": {
      "repo": { "type": "string", "description": "Repository root path" }
    }
  }
  ```
- **Returns:** List of orphaned or dangling worktrees cleaned up.

---

## 4. Verification Plan

1. **Unit & Integration Tests in `crates/superstate-mcp/tests/`**:
   - `test_mcp_state_inspection.rs`: tests `state_status`, `state_show`, `state_resume`, `state_metrics`.
   - `test_mcp_state_safety.rs`: tests `state_checkpoint`, `state_rollback`, `state_exec` with both clean-tree pass and dirty-tree rollback on fail.
   - `test_mcp_maintenance_and_dag.rs`: tests `state_ready`, `state_sync`, `state_doctor`, `state_validate`, `state_apply`, `state_list`.
   - `test_mcp_worktree_lifecycle.rs`: tests `worktree_list`, `worktree_remove`, `worktree_clean`.
   - `test_mcp_tool_parameter_parity.rs`: tests `state_next` with `ignore_untracked_glob` and `state_prompt` with `task` and `with_graph`.
2. **End-to-End Handshake**:
   - Verify all tools appear in `tools/list` response from `run_stdio_mcp_server`.
   - Ensure clean compilation: `cargo test --all` and `cargo clippy --all-targets -- -D warnings`.
