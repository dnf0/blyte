# Branch-Scoped State Isolation & Multi-Agent Concurrency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement branch-scoped state resolution, reverse worktree discovery, and multi-agent concurrency in Superstate so multiple agents can work in parallel in the same repository on distinct feature branches without state file collisions.

**Architecture:** Partition state files on feature branches into `.agent-state/branches/<sanitized-branch>.json` while retaining `.agent-state/state.json` on default branches. Worktrees discover their owning branch state via reverse lookup and explicit prompt contracts. Introduce `superstate state list` to inspect multi-branch states.

**Tech Stack:** Rust 2021, `superstate-core`, `superstate-worktree`, `superstate-cli`, `superstate-mcp`, `clap`, `serde_json`.

## Global Constraints

- No unwrap in library crates (`superstate-core`, `superstate-worktree`, `superstate-mcp`).
- Default branch (`main`, `master`) and explicit `--state` paths must maintain 100% backward compatibility.
- Zero extra git checkouts: worktrees remain lightweight links into `.git/objects`.
- All file writes to state files must be atomic via `save_state_atomic`.

---

### Task 1: Core Path Sanitization, Branch-Scoped Resolution, and Root Discovery (`superstate-core`)

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-core/src/state_machine.rs`
- Modify: `crates/superstate-core/src/lib.rs`
- Create: `crates/superstate-core/tests/test_branch_scoped_paths.rs`

**Interfaces:**
- Produces:
  - `pub fn sanitize_branch_slug(branch: &str) -> String`
  - `pub fn branch_scoped_state_path(repo_root: &Path, branch: &str) -> PathBuf`
  - `pub fn find_branch_state_for_worktree(repo_root: &Path, worktree_path: &Path, worktree_branch: Option<&str>) -> Option<PathBuf>`
  - Update `enclosing_state_root(start: &Path) -> Option<PathBuf>` to recognize `.agent-state/branches/*.json`

**Step-by-Step Instructions:**
1. Write failing unit tests in `crates/superstate-core/tests/test_branch_scoped_paths.rs`:
   - `test_sanitize_branch_slug`: test `feat/stripe-webhooks` -> `feat-stripe-webhooks`, `fix/issue#123` -> `fix-issue-123`, multiple slashes, whitespace, etc.
   - `test_branch_scoped_state_path`: test `feat/auth` returns `<root>/.agent-state/branches/feat-auth.json`, `main` returns `<root>/.agent-state/state.json`, `master` returns `<root>/.agent-state/state.json`.
   - `test_find_branch_state_for_worktree`: test matching a branch state by `active_worktree` path or `active_branch`.
   - `test_enclosing_state_root_with_branch_scoped_state`: test searching up from a nested directory where only a branch state exists.
2. Run `cargo test -p superstate-core --test test_branch_scoped_paths` to confirm failure.
3. In `crates/superstate-core/src/state_machine.rs`:
   - Implement `sanitize_branch_slug`: replace characters not matching `[a-zA-Z0-9._-]` with `-`, collapse consecutive hyphens, trim `-`.
   - Implement `branch_scoped_state_path`: if `branch == "main"` or `branch == "master"`, return `repo_root.join(STATE_DIR).join("state.json")`. Otherwise, return `repo_root.join(STATE_DIR).join("branches").join(format!("{}.json", sanitize_branch_slug(branch)))`.
   - Implement `find_branch_state_for_worktree`: read `.agent-state/branches/` directory, deserialize candidate state files, match `active_worktree` (canonicalized) or `active_branch`.
   - Update `enclosing_state_root`: also check if `current.join(STATE_DIR).join("branches")` exists and contains any `.json` files.
4. In `crates/superstate-core/src/lib.rs`: re-export `sanitize_branch_slug`, `branch_scoped_state_path`, `find_branch_state_for_worktree`.
5. Run `cargo test -p superstate-core` and verify all tests pass.
6. Commit: `git add crates/superstate-core && git commit -m "feat(core): add branch-scoped state paths and worktree reverse lookup"`

---

### Task 2: CLI Branch-Scoped Path Resolution, Turnkey Prompt Contract, and `superstate state list` (`superstate-cli`)

- **Depends on:** Task 1

**Files:**
- Modify: `crates/superstate-cli/src/state_cli.rs`
- Modify: `crates/superstate-cli/src/commands/state.rs`
- Create: `crates/superstate-cli/tests/test_cli_branch_scoped_isolation.rs`

**Interfaces:**
- Consumes: `branch_scoped_state_path`, `find_branch_state_for_worktree`, `sanitize_branch_slug` from `superstate-core`.
- Produces:
  - Subcommand `superstate state list [--json]`
  - Dynamic resolution of default state path when `--state` is omitted.
  - Turnkey prompt including explicit `- State File:` and `- Next Gate Command: superstate state next --state <path>`.

**Step-by-Step Instructions:**
1. Write failing integration tests in `crates/superstate-cli/tests/test_cli_branch_scoped_isolation.rs`:
   - `test_cli_init_on_feature_branch_creates_branch_scoped_state`: In a git repo on `feat/payment`, running `superstate state init --goal "Add Stripe" --verify-cmd "true"` without `--state` creates `.agent-state/branches/feat-payment.json` and does NOT create `.agent-state/state.json`.
   - `test_cli_worktree_reverse_lookup`: Running `superstate state show` or `next` inside `.wt/task-1` without `--state` auto-discovers `.agent-state/branches/feat-payment.json`.
   - `test_cli_state_list`: Initializing states on two branches and running `superstate state list` lists both branches with status and active task.
2. Run `cargo test -p superstate-cli --test test_cli_branch_scoped_isolation` to confirm failure.
3. In `crates/superstate-cli/src/state_cli.rs`:
   - Add `List(ListArgs)` to `StateCommands`.
   - Define `ListArgs` with `#[arg(long)] pub json: bool`.
4. In `crates/superstate-cli/src/commands/state.rs`:
   - Update `resolve_project_root`: traverse upward until finding `.agent-state`, return parent.
   - Implement `resolve_default_state_path(cwd: &Path) -> PathBuf`:
     - If `cwd` matches a worktree in `find_branch_state_for_worktree`, use it.
     - Else query `WorktreeManager::new(cwd).and_then(|m| m.current_branch())`. If on a feature branch (not `main`/`master` and not `wt/*`), return `branch_scoped_state_path`.
     - Else fallback to `cwd.join(".agent-state/state.json")`.
   - In `cmd_init`, `cmd_next`, `cmd_show`, `cmd_prompt`, `cmd_apply`, `cmd_checkpoint`: if `args.state` is `.agent-state/state.json` (the clap default), check if a branch-scoped default should be used instead.
   - In `render_turnkey_prompt` integration: ensure `- State File:` and `- Next Gate Command: superstate state next --state <path>` are present.
   - Implement `cmd_list`: scan `.agent-state/branches/*.json` (and `.agent-state/state.json` if present), deserialize and display tabular output or JSON.
5. Run `cargo test -p superstate-cli` and verify all tests pass.
6. Commit: `git add crates/superstate-cli && git commit -m "feat(cli): add branch-scoped state resolution and state list command"`

---

### Task 3: MCP Branch-Scoped State Resolution & Multi-Agent Concurrency (`superstate-mcp`)

- **Depends on:** Task 1, Task 2

**Files:**
- Modify: `crates/superstate-mcp/src/server.rs`
- Create: `crates/superstate-mcp/tests/test_mcp_branch_scoped_concurrency.rs`

**Interfaces:**
- Consumes: `branch_scoped_state_path`, `find_branch_state_for_worktree` from `superstate-core`.
- Produces:
  - Branch-scoped state targeting when `"state"` argument is omitted from MCP tools.

**Step-by-Step Instructions:**
1. Write failing integration tests in `crates/superstate-mcp/tests/test_mcp_branch_scoped_concurrency.rs`:
   - `test_mcp_handoff_creates_branch_scoped_state`: In a repo on `feat/service-a`, calling `superstate_handoff` without `"state"` creates `.agent-state/branches/feat-service-a.json`.
   - `test_mcp_concurrent_agents_independent_states`: Create branch `feat/service-a` and `feat/service-b`. Agent A initializes on `feat/service-a`. Agent B initializes on `feat/service-b`. Verify both states exist independently, neither modifies `.agent-state/state.json`, and advancing Agent A does not mutate Agent B.
2. Run `cargo test -p superstate-mcp --test test_mcp_branch_scoped_concurrency` to confirm failure.
3. In `crates/superstate-mcp/src/server.rs`:
   - Implement helper `resolve_mcp_state_path(repo_root: &Path, explicit_state: Option<&str>) -> PathBuf`.
   - In `superstate_handoff`, `superstate_preflight`, `state_next`, `state_get`, `state_step`: use `resolve_mcp_state_path`.
4. Run `cargo test -p superstate-mcp` and verify all tests pass.
5. Commit: `git add crates/superstate-mcp && git commit -m "feat(mcp): support branch-scoped state resolution for concurrent agents"`

---

### Task 4: End-to-End Concurrency Verification & Documentation

- **Depends on:** Tasks 1–3

**Files:**
- Modify: `README.md`
- Modify: `crates/superstate-cli/README.md`
- Run all workspace tests

**Step-by-Step Instructions:**
1. Run `cargo test --all` across the entire workspace.
2. Run `cargo clippy --all-targets --all-features -- -D warnings`.
3. Update `README.md` and `crates/superstate-cli/README.md`:
   - Document Branch-Scoped State Isolation (`.agent-state/branches/<branch>.json`).
   - Document `superstate state list` usage and output format.
   - Document multi-agent concurrency guidelines (parallel agents operating across multiple feature branches).
4. Commit: `git commit -am "chore(docs): update documentation for branch-scoped state isolation and concurrency"`
