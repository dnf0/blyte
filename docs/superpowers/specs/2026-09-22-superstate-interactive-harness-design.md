# Superstate: Unified Interactive & Benchmark Agent Harness Design

## Context & Motivation

Superstate is transitioning from a modular state machine library and agent prototype into a first-class agent harness. The user directive is clear:
1. **Unify Identity**: The tool and command surface must simply be called `superstate`. Legacy secondary binaries (like `state`) are retired.
2. **Implementation of SKILL.state ([arXiv:2608.26263v2](https://arxiv.org/html/2608.26263v2))**: Badhe et al. (2026) establish that conversational agent architectures collapse on real-world engineering due to quadratic token explosion $\mathcal{O}(T^2)$ and context poisoning from failed attempts and voluminous telemetry. SKILL.state proves that structured state transitions:
   $$(P, \Sigma_t, O_t) \longrightarrow (R_t, \Delta \Sigma_t, a_t), \quad \Sigma_{t+1} = \Sigma_t \oplus \Delta \Sigma_t$$
   achieve **16.2x token reduction** while maintaining bounded context size $|\mathcal{C}_t| = \mathcal{O}(1)$ and linear cumulative tokens $\mathcal{O}(T)$.
3. **Harness Parity & Ergonomics**: Frontier benchmark harnesses (e.g. `dsh`, `codex-exec`) outperform generic chat wrappers on SWE-bench Verified because of hermetic execution seams, fail-closed verification gates, automatic git rollbacks, and explicit state tracking. Meanwhile, developer adoption requires the fluid interactive ergonomics of tools like Claude Code (live streaming, slash commands, REPL experience).

This design unites both: an ergonomic Claude Code-style terminal REPL front-end powered by the exact SKILL.state execution engine underneath.

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
                         | - Three-Region State (P, Σ, O)    |               | - Synchronous normalized I/O      |
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

    /// Context strategy: 'state-only' (SKILL.state O(1) prompt) or 'conversational' (append-all chat)
    #[arg(long, default_value = "conversational")]
    context: String,

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
    - `● note (patching state: hypothesis = "...")`
    - `● gate pytest -> PASS`
- **Slash Commands**:
  - `/help`: List slash commands and active keybindings.
  - `/state`: Inspect active SKILL.state regions (`Orchestrator`, `Runtime`, `Agent`).
  - `/diff`: Output `git diff` of uncommitted working tree modifications.
  - `/review`: Trigger Superstate's built-in adversarial review engine over current diff.
  - `/doctor`: Run the 5-step fail-closed verification handshake.
  - `/gate [cmd]`: Inspect or update the active verification gate command.
  - `/clear`: Reset conversational turn history while keeping file discoveries.
  - `/exit` or `/quit`: Terminate the harness cleanly.

### 4. Implementation of SKILL.state Principles
Under both interactive and headless modes, the harness strictly enforces:
- **Three-Region Memory ($P, \Sigma_t, O_t$)**:
  - `Orchestrator`: Task goal and verification command. Completely immutable to the agent.
  - `Runtime`: Execution facts recorded strictly by the harness (attempts, gate failures, rollbacks, tool discovery).
  - `Agent`: Working notes, hypotheses, and state slots updated via `note` using null-deletion patch semantics ($\Sigma_{t+1} = \Sigma_t \oplus \Delta \Sigma_t$).
- **Bounded Context**: In `state-only` mode, intermediate reasoning $R_t$ and past observations are discarded, keeping prompt size $|\mathcal{C}_t| = \mathcal{O}(1)$ and defeating the quadratic token trap.
- **Sealed Verification Gate**: The gate command is external and excluded from the agent's tool surface; the harness independently evaluates it only when the agent finishes calling tools.
- **Fail-Closed Checkpoint Rollback**: On gate failure, working tree state is reset to the baseline checkpoint via `git reset --hard`, armed with the exact failure output in $\Sigma_{t+1}$.
- **Process Group Sandboxing**: All tool execution runs under process groups (`setpgid(0)`) with negative PGID killing on deadline expiry, preventing orphan leaks and pipe deadlocks.

---

## Testing & Verification Plan

1. **CLI Routing & Flag Tests**:
   - `test_cli_launches_subcommand_when_specified`: Verifies `superstate doctor` routes correctly.
   - `test_cli_single_shot_prompt_parsing`: Verifies `superstate "fix bug"` parses flags, gate, context strategy, and prompt.
2. **Headless Solver End-to-End Test**:
   - Seeded repository with a failing test; runs single-shot `superstate "fix test" --gate "pytest"` to completion and verifies zero exit code.
3. **Interactive REPL Slash Command Tests**:
   - Programmatically feed commands (`/help`, `/state`, `/diff`, `/gate`, `/exit`) through a pipe to verify command routing, state inspection, and clean exit.
4. **Workspace Cleanliness**:
   - Clippy, rustfmt, and workspace tests pass with zero warnings across Ubuntu and macOS.
