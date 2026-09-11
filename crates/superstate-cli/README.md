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

### 2. Feature Branch Targeting & Isolated Worktrees

When initialized within a git repository, Superstate discovers and binds the active feature branch (via `git rev-parse --abbrev-ref HEAD`):

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
```
