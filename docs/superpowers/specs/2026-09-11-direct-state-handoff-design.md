# Direct State Handoff & Plan-Free Engine Design

## Context & Motivation
Historically, agentic systems relied on markdown files (`plan.md`) written to disk to express task decomposition. While readable by humans in pull requests, using markdown files as the internal persistence and communication contract of an autonomous control plane introduces severe drawbacks:
1. **Unnecessary Round-Trip**: The system serializes structured intent into English prose with headers (`### Task N:`) only to parse it back with regexes into typed state (`state.json`).
2. **Git Clutter**: Markdown files dirty the working tree, requiring `.git/info/exclude` workarounds so `--verify-git-clean` does not fail on harness bookkeeping.
3. **Latency & Token Tax**: In benchmarks and single-objective tasks, running an upfront LLM turn to write `plan.md` adds 30–45 seconds of latency and thousands of tokens before code execution begins.
4. **Handoff Friction**: In tools like Claude Code and `agy`, humans and agents naturally discover requirements through conversation. What is needed is not a separate "harness over a harness" or an awkward new TUI, but a **direct handoff contract** that takes the agreed goal and verification invariant and executes it in an isolated, verified micro-turn.

## Goals
- Allow `superstate state init` to initialize state directly from `--goal` and `--verify-cmd` without requiring, creating, or reading any `plan.md` on disk.
- Provide `superstate state show` (and `--markdown`) to render human-readable markdown views of the active state on demand.
- Implement `superstate_preflight` and `superstate_handoff` in `superstate-mcp` with full MCP `tools/call` dispatch.
- Enable Claude Code and `agy` to use Superstate as a verified execution engine via MCP or direct CLI invocation.
- Update `superstate-eval` to utilize direct state initialization, cutting Phase 1 planning latency to 0ms on atomic tasks.

## Non-Goals
- We are NOT building an interactive TUI/REPL to replace Claude Code or `agy`. Developers stay in their preferred environment.
- We are NOT discarding multi-milestone planning; `superstate state init --plan <path>` remains supported for complex multi-module features.

---

## Architectural Specification

### 1. Direct State Initialization in `superstate-cli`

#### Command Interface
```bash
superstate state init \
  [--plan <path>] \
  [--goal <string>] \
  [--verify-cmd <string>] \
  [--files <comma_separated_paths>] \
  [--state <path>] \
  [--no-worktree]
```

#### Invariants & Validation
1. **Mutually Exclusive / Exhaustive Input**:
   - Either `--plan <path>` MUST be provided, OR (`--goal` AND `--verify-cmd`) MUST be provided.
   - If neither or an incomplete pair is provided, the CLI fails closed with an actionable error message.
2. **ExecutionState Construction**:
   When `--goal` and `--verify-cmd` are given:
   - `TaskSpec` is constructed directly:
     ```rust
     TaskSpec {
         id: "task-1".to_string(),
         title: goal.to_string(),
         depends_on: vec![],
         files_modify: parsed_files,
         verification_cmd: Some(verify_cmd.to_string()),
     }
     ```
   - `ExecutionState` is initialized with:
     - `active_task`: `Some(task)`
     - `pending_tasks`: `vec![]`
     - `completed_tasks`: `vec![]`
     - `step`: 1
     - `status`: `ExecutionStatus::InProgress`
     - `no_worktree`: user flag
   - Atomic write directly to `.agent-state/state.json`. Zero markdown files are created on disk.

### 2. Markdown On Demand (`superstate state show`)
For human inspection, debugging, or logging, `superstate state show` renders the state as formatted Markdown:
```bash
superstate state show [--markdown] [--json]
```
Outputs:
```markdown
# Superstate Execution Status
- **Status:** InProgress (Step 1)
- **Active Task:** task-1: Store Stripe event IDs in Redis with 24h TTL
- **Verification Command:** `pytest tests/test_webhooks.py -k test_idempotency`
- **Active Worktree:** `.wt/task-1` (branch `wt/task-1`)
```

### 3. MCP Server: Pre-Flight & Handoff (`crates/superstate-mcp`)

`superstate-mcp` is updated with full MCP `tools/call` handling and two dedicated tools:

#### A. `superstate_preflight`
- **Tool Schema**:
  - `verify_cmd` (string, required): Shell command to execute.
- **Implementation**:
  - Spawns the command with a 15-second timeout in the repository root.
  - Returns exit code, stdout snippet (capped at 500 chars), stderr snippet, and executable boolean.
  - Allows an LLM or human to verify that a test runner or script is valid before provisioning worktrees.

#### B. `superstate_handoff`
- **Tool Schema**:
  - `goal` (string, required): Concise statement of capability delivered.
  - `verify_cmd` (string, required): Shell command that must exit 0 to pass.
  - `files` (array of strings, optional): Target files to modify.
  - `no_worktree` (boolean, optional): If true, runs in-place without worktree isolation.
- **Implementation**:
  - Executes dry-run validation.
  - Calls `cmd_init` directly with goal and verify command.
  - If `no_worktree` is false, provisions the isolated worktree via `superstate_worktree`.
  - Calls `cmd_prompt` to generate the turnkey execution prompt.
  - Returns:
    ```json
    {
      "status": "ready",
      "task_id": "task-1",
      "worktree_path": ".wt/task-1",
      "branch": "wt/task-1",
      "turnkey_prompt": "..."
    }
    ```

#### C. `tools/call` Dispatch
Implement JSON-RPC handler for `tools/call` routing to:
- `superstate_preflight`
- `superstate_handoff`
- `state_init`
- `state_next`
- `state_prompt`
- `state_note`
- `superstate_review`

### 4. Evaluation Harness Integration (`superstate-eval`)
In `SuperstateAdapter`:
- Update `execute()`: when `zero_turn_planning=True`:
  - Directly invoke `superstate state init --goal "..." --verify-cmd "..." [--no-worktree]`.
  - Skip writing `plan.md` to disk entirely.
  - Record provenance: `"plan_source": "direct_state_handoff"`.

---

## Verification & Testing Strategy

1. **Unit & Integration Tests in `superstate-cli`**:
   - `test_state_init_with_goal_and_verify_cmd`: Asserts that `ExecutionState` is created with a single active task and verification command without any `plan.md` existing on disk.
   - `test_state_init_fails_without_plan_or_goal`: Asserts fail-closed validation when inputs are incomplete.
   - `test_state_show_renders_markdown_view`: Asserts markdown output on demand.
2. **MCP Integration Tests in `superstate-mcp`**:
   - `test_mcp_tools_call_preflight`: Tests pre-flight command execution.
   - `test_mcp_tools_call_handoff`: Tests direct handoff creating `.agent-state/state.json` and provisioning worktree.
3. **Adapter Tests in `superstate-eval`**:
   - `test_zero_turn_direct_state_init`: Asserts zero markdown files are written to workspace while state machine initializes and executes.
4. **End-to-End Benchmark Execution**:
   - Run paired benchmark on `terminal-bench/git-leak-recovery` and `openssl-selfsigned-cert` proving the ~30s speedup from zero-turn execution.
