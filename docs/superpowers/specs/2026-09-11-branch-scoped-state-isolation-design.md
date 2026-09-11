# Branch-Scoped State Isolation & Multi-Agent Concurrency Design Specification

- **Date:** 2026-09-11
- **Status:** Draft
- **Authors:** Google DeepMind Team & Daniel Fisher
- **Repository:** `superstate`

---

## 1. Executive Summary

Superstate currently defaults to storing execution state at `<repo_root>/.agent-state/state.json`. When multiple agents (or human developers working with autonomous agents) operate concurrently in the same repository on different feature branches, they collide on this single state file, corrupting progress tracking and failing verification gates.

This specification introduces **Branch-Scoped State Isolation**:
1. When running without an explicit `--state` path, execution states for feature branches are automatically partitioned into `<repo_root>/.agent-state/branches/<sanitized-branch>.json`.
2. Default branches (`main`, `master`) and non-git environments continue to default to `.agent-state/state.json` for 100% backward compatibility.
3. Subagents running inside isolated task worktrees (`.wt/<task_slug>`) auto-discover their branch-scoped state file via reverse worktree matching and turnkey prompt binding.
4. An operator-facing CLI command `superstate state list` is added to inspect all active and completed branch state machines across the repository.

---

## 2. Directory Layout & Storage Architecture

```
<repo-root>/
├── .agent-state/
│   ├── state.json                       # Default branch (main/master) or non-git fallback
│   └── branches/
│       ├── feat-stripe-webhooks.json    # Agent 1 state machine & task DAG
│       ├── feat-profile-export.json     # Agent 2 state machine & task DAG
│       └── fix-auth-race.json           # Agent 3 state machine & task DAG
├── .wt/
│   ├── task-1/                          # Worktree for Agent 1 (branch: wt/task-1)
│   └── export-1/                        # Worktree for Agent 2 (branch: wt/export-1)
└── src/
```

### 2.1 Branch Name Sanitization
Git branch names often contain characters unsuitable for flat filenames (e.g., `feat/stripe-webhooks`, `fix/issue#123`, `user@domain/test`).
The sanitization function `sanitize_branch_slug(branch: &str) -> String` transforms branch names:
- Forward slashes (`/`), backslashes (`\`), colons (`:`), spaces, and control characters are replaced with `-`.
- Consecutive hyphens are collapsed into a single hyphen.
- Leading and trailing hyphens/dots are trimmed.
- Example: `feat/stripe-webhooks` -> `feat-stripe-webhooks`.

---

## 3. State Path Resolution Protocol

When a state operation (`superstate state init`, `next`, `show`, `prompt`, `checkpoint`, `rollback`, `mcp:superstate_handoff`, `mcp:state_next`) is invoked:

### 3.1 Precedence Chain
1. **Explicit Argument:** If `--state <path>` (CLI) or `"state": "<path>"` (MCP) is provided, resolve it against CWD using upward ancestor search (`resolve_state_path`).
2. **Worktree Reverse-Lookup:** If CWD is located inside an isolated worktree directory (e.g. `.wt/<task_id>`):
   - Locate the repository `.agent-state/branches/` directory.
   - Scan all `.json` files in `.agent-state/branches/`.
   - Match if `state.active_worktree == canonical_cwd` OR `state.active_branch == git_current_branch`.
   - If matched, return the path to that branch-scoped state file.
3. **Feature Branch Auto-Detection:**
   - Detect the current git branch using `WorktreeManager::current_branch(&cwd)`.
   - If branch is detected and is NOT a default branch (`main`, `master`) and does NOT start with `wt/` or `superstate/`:
     - Return `<repo_root>/.agent-state/branches/<sanitized_branch>.json`.
4. **Fallback:**
   - Return `<repo_root>/.agent-state/state.json`.

---

## 4. Worktree Turnkey Prompt Integration

When `superstate` initializes state (`superstate state init`) or advances to the next task (`superstate state next`), it renders a turnkey prompt contract for the subagent dispatched into `.wt/<task_slug>`.

To prevent ambiguity and provide O(1) direct access, the rendered prompt will explicitly include the resolved state path and advancement command:

```markdown
## 🌲 Active Isolated Worktree & State Contract
- **Worktree Path:** `/path/to/repo/.wt/task-1`
- **Worktree Branch:** `wt/task-1`
- **Target Base Branch:** `feat/stripe-webhooks`
- **State File:** `/path/to/repo/.agent-state/branches/feat-stripe-webhooks.json`
- **Next Gate Command:** `superstate state next --state .agent-state/branches/feat-stripe-webhooks.json`
```

---

## 5. Repository Root Discovery (`resolve_project_root`)

Currently, `resolve_project_root` checks if `state_path.parent()` is `.agent-state`.
For branch-scoped states:
- `state_path`: `/path/to/repo/.agent-state/branches/feat-auth.json`
- `parent`: `.../.agent-state/branches`
- `grandparent`: `.../.agent-state`
- `great-grandparent`: `/path/to/repo`

The helper will be updated to walk upward through directory components until finding `.agent-state`, returning its parent as the repository root.

---

## 6. CLI `superstate state list`

A new CLI subcommand `superstate state list` is introduced to provide a multi-branch status view.

### Output Example
```
Active & Completed Branches:
  Branch                     Status       Active Task          Step  Base Branch
  feat-stripe-webhooks       InProgress   task-2 (Stripe SDK)  2/3   feat/stripe-webhooks
  feat-profile-export        InProgress   task-1 (CSV export)  1/2   feat/profile-export
  fix-auth-race              Completed    -                    1/1   fix/auth-race
```
Flags:
- `--json`: Output full machine-readable array of branch summaries.

---

## 7. State Retention & Concurrency Safety

1. **State Retention:** Completed branch states are retained in-place with `status: Completed`. This preserves full auditability, ledger entries, and test records.
2. **Atomic Writes:** All state mutations continue to use `save_state_atomic` (`write to .tmp.<pid>.<uuid>` followed by atomic file rename).
3. **Filesystem Isolation:** Each agent runs in its own worktree directory (`.wt/<task_slug>`), guaranteeing separate build caches, separate index locks (`.git/worktrees/.../index.lock`), and zero workspace interference.

---

## 8. Verification & Acceptance Criteria

1. **Unit Tests (`superstate-core`):**
   - Test `sanitize_branch_slug` with nested slashes, special characters, and edge cases.
   - Test branch-scoped path resolution for feature branches vs default branches.
   - Test worktree reverse-lookup matching `active_worktree`.
2. **Integration Tests (`superstate-cli`):**
   - Test initializing on `feat/branch-a` creates `.agent-state/branches/feat-branch-a.json`.
   - Test initializing on `feat/branch-b` concurrently creates `.agent-state/branches/feat-branch-b.json` without collision.
   - Test advancing both branches independently.
   - Test `superstate state list` output formatting.
3. **MCP Integration Tests (`superstate-mcp`):**
   - Test `superstate_handoff` automatically targets `.agent-state/branches/<branch>.json` on feature branches.
   - Test `state_next` advances branch-scoped state cleanly.
4. **End-to-End Workspace Health:**
   - 100% test pass across all crates.
   - Zero clippy warnings.
