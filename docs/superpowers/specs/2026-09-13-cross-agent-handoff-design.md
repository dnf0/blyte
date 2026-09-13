# Design Specification: Cross-Agent Handoff Protocol & Driver Presets for Claude and Agy

- **Date**: 2026-09-13
- **Status**: Approved
- **Target Components**: `superstate-core`, `superstate-cli`, `superstate-mcp`

---

## 1. Context & Motivation

Superstate provides stateful task tracking, isolated worktree lifecycles, and autonomous background daemon runners. Developers and agents using Claude Code and Google Antigravity (`agy`) frequently interact with Superstate in two distinct workflows:
1. **Autonomous Detached Execution**: An interactive agent finishes planning or task specification and delegates multi-step milestone execution to an autonomous background daemon.
2. **Interactive Cross-Agent Session Transitions**: An interactive session in Claude Code or Antigravity hands off an active milestone to another interactive session (e.g. Claude -> Agy, Agy -> Claude, or Agent -> Human developer).

### Problems Identified
1. **Permission Deadlocks in Headless Mode**:
   - Running bare `claude` non-interactively blocks on file writes with:
     `"The write was blocked — the harness requested permission to write <file> and it wasn't granted, and this session is non-interactive so there's no prompt to approve it."`
   - Non-interactive execution requires `--dangerously-skip-permissions` (or `--permission-mode bypassPermissions`) and `-p`.
   - Running `agy` non-interactively requires `-p` (or `--print`) and `--dangerously-skip-permissions`.
2. **Context Loss Between Interactive Sessions**:
   - When switching between Claude Code and Antigravity, the receiving agent does not know the active worktree path, state file location, active milestone, verification invariant, or earlier discoveries recorded in the epistemic ledger.
3. **CLI & MCP Asymmetry**:
   - `superstate_handoff` exists in the MCP server, but there is no CLI equivalent (`superstate state handoff`), forcing shell scripts and CLI users to manually construct state and worktree arguments.

---

## 2. Architectural Design

```
+-----------------------------------------------------------------------------------+
|                            Cross-Agent Handoff Flow                               |
+-----------------------------------------------------------------------------------+
                                       |
                   Initiate Handoff (CLI or MCP)
         `superstate state handoff` or `superstate_handoff`
                                       |
                   +-------------------+-------------------+
                   |                                       |
        [Mode 1: Detached Daemon]              [Mode 2: Interactive Session]
                   |                                       |
        Resolve Driver Presets                  Generate Handoff Artifact
     - claude -> `claude -p --...`            `.agent-state/handoff.md`
     - agy    -> `agy -p --...`                            |
                   |                            Print Tailored Resumption
        Spawn Background Daemon               - Claude: `cd <wt> && claude`
        Stream Prompt to Stdin                - Agy:    `cd <wt> && agy`
        Run Step Advancer & Gates             - Native: `superstate state resume`
```

---

## 3. Component Specifications

### A. Autonomous Driver Preset Engine (`superstate-core::daemon`)

#### 1. Function Signature
```rust
pub fn resolve_driver_command(driver: &str) -> String
```

#### 2. Expansion Rules
- **Claude Code (`claude`)**:
  - Exact match `"claude"` expands to:
    `claude -p --dangerously-skip-permissions`
  - Starts with `"claude "`:
    - Injects `-p` if neither `-p` nor `--print` is present.
    - Injects `--dangerously-skip-permissions` if neither `--dangerously-skip-permissions` nor `--permission-mode bypassPermissions` is present.
    - Preserves all other user-supplied arguments (e.g. `claude --model sonnet-4` -> `claude --model sonnet-4 -p --dangerously-skip-permissions`).
- **Antigravity CLI (`agy`)**:
  - Exact match `"agy"` expands to:
    `agy -p --dangerously-skip-permissions`
  - Starts with `"agy "`:
    - Injects `-p` if neither `-p` nor `--print` is present.
    - Injects `--dangerously-skip-permissions` if not present.
    - Preserves user-supplied arguments (e.g. `agy --model gemini-3.8-pro` -> `agy --model gemini-3.8-pro -p --dangerously-skip-permissions`).
- **Custom Commands**:
  - Any command not starting with `claude` or `agy` is returned unmodified.

#### 3. Execution Environment & Stdin Streaming
`DaemonRunner::run_step` ensures:
- Working directory is set to `.wt/<slug>` (when worktree isolation is active) or repository root.
- Environment variables exposed to child:
  - `SUPERSTATE_PROMPT`: Turnkey prompt markdown.
  - `SUPERSTATE_PROMPT_FILE`: Path to `.agent-state/prompts/<slug>.md`.
  - `SUPERSTATE_ACTIVE_TASK`: Active milestone ID.
  - `SUPERSTATE_WORKTREE_PATH`: Path to isolated worktree.
- Stdin is streamed on a background thread to prevent pipe deadlocks.
- Non-zero driver exit codes are logged as notes (`[driver-warn] ...`) without killing the daemon, letting the verification gate command evaluate completion.

---

### B. Standardized Handoff Artifact (`.agent-state/handoff.md`)

When a handoff is created, Superstate writes `.agent-state/handoff.md` and `.agent-state/handoffs/<slug>.md`:

```markdown
# 🤝 Superstate Handoff Package

- **Generated**: YYYY-MM-DD HH:MM:SS
- **Source Agent**: claude | agy | human | unknown
- **Target Agent**: agy | claude | daemon | any
- **Job Slug**: <slug>
- **State File**: `.agent-state/state.json` (or `.agent-state/state-<slug>.json`)
- **Active Task**: `<task_id>`: <title>
- **Working Directory**: `.wt/<slug>-<task_id>` (or repo root)

---

## 🎯 Global Objective
<Global task objective and requirements>

## 📋 Active Milestone Specification
- **Task ID**: `<task_id>`
- **Title**: <title>
- **Verification Invariant**: `<verify_cmd>`
- **Files to Modify**:
  - <path>

## 🧠 Epistemic Ledger (Discoveries & Guardrails)
- [<author>] <note>

---

## 🚀 Ready-to-Run Resumption

### Option A: If continuing with Claude Code
\`\`\`bash
cd <working_directory> && claude
# Prompt: "Review .agent-state/handoff.md and implement active milestone <task_id>."
\`\`\`

### Option B: If continuing with Antigravity (agy CLI / IDE)
\`\`\`bash
cd <working_directory> && agy
# Or use MCP tool state_prompt to load turnkey instructions
\`\`\`

### Option C: If delegating to background daemon
\`\`\`bash
superstate state daemon --state <state_file> --driver claude --detach
\`\`\`
```

---

### C. CLI & MCP Interfaces

#### 1. CLI: `superstate state handoff`
```bash
superstate state handoff [OPTIONS]
```
Options:
- `--goal <GOAL>`: High-level goal.
- `--verify-cmd <CMD>`: Verification invariant.
- `--plan <PATH>`: Optional plan file to import milestones.
- `--from <AGENT>`: `claude`, `agy`, or `human` (default: auto-detected or `human`).
- `--to <AGENT>`: `agy`, `claude`, `daemon`, or `any` (default: `any`).
- `--detach`: Spawn background daemon runner immediately.
- `--driver <DRIVER>`: Driver preset or custom command.
- `--worktree` / `--no-worktree`: Worktree toggle (default: true).
- `--json`: Format output as JSON.

#### 2. MCP: `superstate_handoff`
Input schema additions:
- `from_agent`: Optional string (`"claude"`, `"agy"`, `"human"`).
- `to_agent`: Optional string (`"claude"`, `"agy"`, `"daemon"`, `"any"`).
- `driver`: Preset name (`"claude"`, `"agy"`) or custom command.

Response payload additions:
- `handoff_file`: Path to `.agent-state/handoff.md`.
- `target_resumption_command`: Tailored command for the receiving agent.
- `turnkey_prompt`: Full markdown prompt.

#### 3. State Resume Enhancement (`superstate state resume` / `state_resume`)
- If `.agent-state/handoff.md` exists, `state resume` displays a highlighted **🤝 Active Handoff** section with target resumption commands.

---

## 4. Verification & Testing Strategy

1. **Core Unit Tests (`crates/superstate-core`)**:
   - `test_resolve_driver_preset_claude`: Test exact `claude` and args expansion with `-p` and `--dangerously-skip-permissions`.
   - `test_resolve_driver_preset_agy`: Test exact `agy` and args expansion with `-p` and `--dangerously-skip-permissions`.
   - `test_resolve_driver_custom`: Verify non-preset commands remain unmodified.
   - `test_render_handoff_markdown`: Verify generation of `.agent-state/handoff.md` with all required sections.
2. **CLI Tests (`crates/superstate-cli`)**:
   - `test_cli_handoff_interactive`: Interactive handoff from Claude to Agy.
   - `test_cli_handoff_detached_preset`: Detached handoff using `--driver claude`.
   - `test_cli_resume_shows_handoff`: Verify `state resume` highlights handoff.
3. **MCP Tests (`crates/superstate-mcp`)**:
   - `test_mcp_handoff_claude_to_agy`: Verify tool returns `handoff_file` and tailored resumption command.
4. **End-to-End Simulation (`tests/e2e_cross_agent_handoff.sh`)**:
   - Full simulated workflow across Claude Code and Agy personas.
