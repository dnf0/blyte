# Superstate CLI (`superstate` / `state`)

The command-line interface and state engine runner for Superstate.

## Installation

```bash
cargo install --path crates/superstate-cli
```

This installs both `superstate` (the unified CLI) and `state` (direct state runner alias).

## State Initialization

Superstate manages task execution DAGs with fail-closed verification gates.

### 1. Direct Multi-Milestone Handoff (In-Memory)

Initialize execution state directly from chat or scripts without writing intermediate markdown plan files to disk.

```bash
# Direct JSON array string
superstate state init --milestones '[
  {
    "title": "Task 1: Core Domain Models",
    "verify_cmd": "cargo test -p my-core",
    "files": ["crates/core/src/models.rs"]
  },
  {
    "title": "Task 2: CLI Wiring",
    "verify_cmd": "cargo test -p my-cli"
  }
]'

# Or stream JSON via stdin using '-'
cat milestones.json | superstate state init --milestones -
```

Each milestone in the array defines:
- `title` (*required*): A concise, descriptive title for the task.
- `verify_cmd` (*required*): The exact verification command that must exit with code 0 to pass the gate.
- `files` (*optional*): Target files associated with this milestone.

### 2. Feature Branch Targeting, Worktree Isolation & Branch-Scoped State

When initialized within a git repository, Superstate discovers and binds the active feature branch (via `git rev-parse --abbrev-ref HEAD`):

- **Branch-Scoped State Isolation:** Feature branches automatically resolve and persist state to `.agent-state/branches/<sanitized-branch>.json` (e.g., branch `feat/payment-integration` stores state in `.agent-state/branches/feat-payment-integration.json`). Default branches (`main`, `master`) and non-git environments continue to default to `.agent-state/state.json` (100% backward compatible).
- **Reverse Worktree Discovery:** When an agent or command runs from within an isolated worktree (`.wt/<task_slug>`), Superstate scans `.agent-state/branches/*.json` and automatically resolves the matching parent branch state file without requiring `--state`.
- **Turnkey Prompt State Contract:** Prompts rendered via `superstate state prompt` explicitly include:
  ```markdown
  - **State File:** `/path/to/repo/.agent-state/branches/feat-payment.json`
  - **Next Gate Command:** `superstate state next --state "/path/to/repo/.agent-state/branches/feat-payment.json"`
  ```
  ensuring agents and runners always advance against the exact branch state file.
- **Target Branch Safety:** Worktree merges target your current feature branch (e.g., `feat/my-feature`), **never `main`**.
- **Automatic Worktree Provisioning:** Superstate automatically provisions an isolated git worktree for the active milestone (`.wt/task-1` on branch `wt/task-1`), branched from your active feature branch.
- **Copy-on-Write Build Caches:** Shared build cache directories (e.g. `target/`, `node_modules/`) are reflinked to preserve build performance without contamination.
- **Sequential Worktree Progression:** Running `superstate state next --verify-cmd "..."` runs the verification command, auto-commits any uncommitted worktree changes, merges the worktree cleanly into your active feature branch, cleans up `.wt/task-1`, and automatically provisions `.wt/task-2` branched from the updated feature branch.

To disable worktree provisioning and run directly in the working tree, pass `--no-worktree`:
```bash
superstate state init --milestones '[...]' --no-worktree
```

### 3. Single Goal Handoff

```bash
superstate state init --goal "Implement user authentication" --verify-cmd "cargo test auth"
```

### 4. Markdown Plan Initialization

```bash
superstate state init --from-plan docs/plan.md
```

## Advancing State

Advance the active task through the verification gate:

```bash
superstate state next --verify-cmd "cargo test" --verify-git-clean
```

Or execute and advance in a single atomic step with automatic rollback on failure:

```bash
superstate state exec --verify-cmd "cargo test" --checkpoint --rollback-on-fail
```

## Inspecting State

```bash
# Render human-readable status including Base Branch and Active Worktree
superstate state show

# JSON status output for scripting and agents
superstate state status --json

# List tasks whose dependencies are satisfied
superstate state ready --json

# Render self-contained turnkey prompt for a task
superstate state prompt --task "task-1" --with-graph

# Render ASCII table of active and completed branch-scoped states
superstate state list

# Emit machine-readable JSON array of branch summaries for orchestrators
superstate state list --json
```

### Branch State Listing (`state list`)

The `state list` subcommand inspects `.agent-state/branches/*.json` (and `.agent-state/state.json`) across the repository:

```bash
superstate state list
```

```text
Active & Completed Branches:
  Branch                     Status       Active Task              Step   Base Branch         
  feat-payment               InProgress   task-1: Add Stripe       1/2    main                
  feat-analytics             InProgress   task-1: Export Analytics 1/1    main                
  main (default)             Completed    -                        3/3    main                
```

Or structured JSON via `--json`:

```bash
superstate state list --json
```

```json
[
  {
    "branch": "feat-payment",
    "status": "in_progress",
    "active_task": "task-1: Add Stripe",
    "step": 1,
    "total_steps": 2,
    "base_branch": "main",
    "state_file": "/path/to/repo/.agent-state/branches/feat-payment.json"
  }
]
```

## Multi-Agent Concurrency

Superstate CLI natively supports concurrent multi-agent and multi-session development:

- **Partitioned Execution States:** When parallel agents run on distinct feature branches (`feat/service-a`, `feat/service-b`), their state machines write independently to `.agent-state/branches/<slug>.json`. Neither agent can overwrite, corrupt, or block the other's state.
- **Isolated Git Worktrees:** Each milestone executes inside an isolated git worktree (`.wt/<task_slug>`), allowing concurrent compiles, edits, and test runs without git index collisions or working tree locks.
- **Worktree Reverse Lookup:** Subagents invoked inside `.wt/<task_slug>` can run commands like `superstate state show` or `superstate state next --verify-cmd "..."` directly without specifying `--state`. Superstate automatically inspects the candidate branch state files and binds to the worktree's owner.
- **Deterministic Turnkey Handoffs:** Prompts produced by `superstate state prompt` contain explicit `- **State File:**` paths and `- **Next Gate Command:**` lines, enabling multi-agent orchestrators to cleanly dispatch tasks to independent worker agents with complete execution contracts.
- **Fleet Visibility:** Run `superstate state list --json` to aggregate status and progression metrics across all parallel agent sessions in the repository.
