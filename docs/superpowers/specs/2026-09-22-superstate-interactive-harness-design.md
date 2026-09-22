# Superstate: Unified Interactive & Benchmark Agent Harness Design

## Context & Motivation

Superstate is transitioning from a modular state machine library and agent prototype into a first-class agent harness. The user directive is clear:
1. **Unify Identity**: The tool and command surface must simply be called `superstate`. Legacy secondary binaries (like `state`) are retired.
2. **Harness Parity**: In frontier harness assessments (e.g. SWE-bench Verified, Terminal-bench, SKILL.state [arXiv:2608.26263v2](https://arxiv.org/html/2608.26263v2)), harnesses like Codex (`codex-exec`) and `dsh` outperform generic chat wrappers due to sandboxed execution seams, fail-closed verification gates, automatic git rollbacks, and bounded three-region state memory. Meanwhile, developer adoption requires the fluid interactive ergonomics of tools like Claude Code (live streaming, slash commands, REPL experience).

This design integrates both: an ergonomic, interactive Claude Code-style terminal REPL front-end powered by a frontier-grade, deterministic execution engine underneath.

---

## Architectural Overview

```
                        +---------------------------------------------+
                        |           superstate CLI Entrypoint         |
                        +---------------------------------------------+
                                       |             |
                     (subcommand)      |             |  (interactive or direct prompt)
                                       v             v
                    +--------------------+         +-------------------------------------+
                    | Existing Commands  |         |   Superstate Harness Engine         |
                    | (doctor, review,   |         |   - Interactive REPL Loop           |
                    |  mcp, worktree, ..)|         |   - Headless Task Solver            |
                    +--------------------+         +-------------------------------------+
                                                                     |
                                           +-------------------------+-------------------------+
                                           |                                                   |
                                           v                                                   v
                         +-----------------------------------+               +-----------------------------------+
                         |      superstate-agent             |               |      superstate-provider          |
                         | - Runner (turn loop, budgets)     |               | - Gemini / OpenAI / Anthropic     |
                         | - Three-Region State              |               | - Synchronous normalized I/O      |
                         | - Shell Seam (Local / Container)  |               +-----------------------------------+
                         | - Tools (read, write, edit, etc.) |
                         | - Fail-Closed Gate & Rollback     |
                         +-----------------------------------+
```

---

## Detailed Components

### 1. Binary Surface & Crate Unification
- **Single Binary**: `crates/superstate-cli/Cargo.toml` exposes only `[[bin]] name = "superstate"`. The secondary `state` binary entry is removed.
- **Dependency**: `crates/superstate-cli` depends directly on `superstate-agent = { path = "../superstate-agent", version = "0.1.0" }` and `superstate-provider = { path = "../superstate-provider", version = "0.1.0" }`.

### 2. CLI Dispatch Rules
In `crates/superstate-cli/src/main.rs`:
```rust
#[derive(Parser)]
#[command(
    name = "superstate",
    version = "0.1.0",
    about = "Superstate: Autonomous agent harness with deterministic state and verification gates"
)]
struct Cli {
    /// Direct task prompt to run in single-shot / headless mode
    #[arg(index = 1)]
    prompt: Option<String>,

    /// Verification gate command (e.g. 'pytest', 'cargo test')
    #[arg(short, long)]
    gate: Option<String>,

    /// Model identifier or provider selection
    #[arg(short, long)]
    model: Option<String>,

    /// Run inside an isolated container workspace instead of local host
    #[arg(long)]
    container: Option<String>,

    /// Maximum turn budget
    #[arg(long, default_value = "25")]
    max_turns: usize,

    /// Maximum verification gate attempts before failing
    #[arg(long, default_value = "5")]
    max_gates: usize,

    /// Enable automatic rollback to baseline git commit if gate fails
    #[arg(long, default_value_t = true)]
    rollback: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}
```

- If `command` is `Some(cmd)`: Dispatch to the specific utility subcommand (`Doctor`, `Review`, `Mcp`, etc.).
- If `command` is `None` and `prompt` is `Some(text)`: Run **Single-Shot Task Mode**.
- If `command` is `None` and `prompt` is `None`: Launch the **Interactive REPL**.

### 3. Interactive REPL UX (`src/repl.rs`)
- **Banner**: Displays ASCII logo, version, active repository, branch, probed environment utilities, and model.
- **Prompt**: Styled `superstate > ` prompt with history and line-editing.
- **Turn Event Streaming**:
  - Model text streamed cleanly.
  - Tool invocations formatted with visual indicators:
    - `● read src/main.rs`
    - `● edit src/lib.rs (lines 40-52)`
    - `● bash cargo check`
    - `● gate pytest -> PASS`
- **Slash Commands**:
  - `/help`: List slash commands and active keybindings.
  - `/diff`: Output `git diff` of uncommitted working tree modifications.
  - `/review`: Trigger Superstate's built-in adversarial review engine over current diff.
  - `/doctor`: Run the 5-step fail-closed verification handshake.
  - `/gate [cmd]`: Inspect or update the active verification gate command.
  - `/clear`: Reset conversational turn history while keeping file discoveries.
  - `/exit` or `/quit`: Terminate the harness cleanly.

### 4. Frontier Execution Engine Parity
Under the interactive shell, the harness runs the identical `Runner` from `superstate-agent`:
- **Process Group Isolation**: Shell commands execute in isolated process groups (`setpgid(0)`) with negative PGID signal dispatch on timeout.
- **Container Seam**: Seamlessly toggle between `LocalShell` and `ContainerShell` via `--container <id>`.
- **Three-Region Memory**: Avoids context window explosion by maintaining distilled environment observations and the action ledger.
- **Fail-Closed Gate & Automatic Rollback**: The harness enforces verification gates objectively without relying on the LLM's self-declaration. If a gate fails, state rolls back to the last known green commit.

---

## Testing & Verification Plan

1. **CLI Routing & Flag Tests**:
   - `test_cli_launches_subcommand_when_specified`: Verifies `superstate doctor` routes correctly.
   - `test_cli_single_shot_prompt_parsing`: Verifies `superstate "fix bug"` parses flags, gate, and prompt.
2. **Headless Solver End-to-End Test**:
   - Seeded repository with a failing test; runs single-shot `superstate "fix test" --gate "pytest"` to completion and verifies zero exit code.
3. **Interactive REPL Slash Command Tests**:
   - Programmatically feed commands (`/help`, `/diff`, `/gate`, `/exit`) through a pipe to verify command routing, state persistence, and clean exit.
4. **Workspace Cleanliness**:
   - Clippy, rustfmt, and workspace tests pass with zero warnings across Ubuntu and macOS.
