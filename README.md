# Superstate

A high-performance, deterministic Rust agent harness implementing the **[SKILL.state](https://arxiv.org/html/2608.26263v2)** architecture ([arXiv:2608.26263v2](https://arxiv.org/html/2608.26263v2)) for autonomous software engineering.

---

## The Problem: Conversational Agent Decay

Standard agent runtimes (ReAct, conversational scratchpads) treat software engineering as an extended chat session: every turn, observation, tool result, and intermediate reasoning trace is appended to an ever-growing prompt.

On real codebases, this collapses:

1. **Quadratic Token Explosion ($\mathcal{O}(T^2)$):** Because prompt size $|\mathcal{C}_t|$ grows linearly with turn count $t$, cumulative tokens grow quadratically $\mathcal{O}(T^2)$. A single long run can consume millions of redundant tokens re-reading obsolete history.
2. **Context Poisoning & Attention Drag:** Failed attempts, noisy background telemetry (e.g. 25,000-line build logs), and discarded hypotheses linger in context forever, degrading reasoning and causing agents to wander in circles.
3. **The Failure of External Wrappers:** Attempting to supervise an agent from outside its loop (via Model Context Protocol or `PreToolUse` hooks) fails at the systems boundary:
   - **47.6% of tokens** were burned on turns doing nothing but waiting for background command polling (`FINDINGS 13.29`).
   - **89–91%** of hook-denied file reads were immediately re-requested on the next turn.
   - You cannot fix execution from outside the loop. The harness must own execution from inside.

---

## The Solution: SKILL.state Architecture

Superstate replaces append-only conversational history with **explicit, mutable execution state transitions**:

```
         (P, Σ_t, O_t) ---> [ Language Model ] ---> (R_t, ΔΣ_t, a_t)
                                                        |        |
                                                        |        v
                                                        |   [ Execution Seam ]
                                                        |   (Local / Container)
                                                        |        |
                                                        v        v
                                                 Σ_{t+1} = Σ_t ⊕ ΔΣ_t    O_{t+1}
```

At every step $t$:
- **$P$ (Immutable Procedural Specification):** The constant prompt: persona, goal, verification command, environment boundaries, and tool specifications.
- **$\Sigma_t$ (Structured Execution State):** The current structured state. Separated by authority:
  - `Orchestrator`: The task goal and verification command (**immutable to the worker**).
  - `Runtime`: Facts observed by the harness (attempts, gate failures, rollbacks).
  - `Agent`: Working notes, tested hypotheses, discovered files, and domain slots (**owned by the worker**).
- **$O_t$ (Latest Observation):** The output of the immediately preceding action. *Zero past observations or previous tool outputs are replayed.*
- **$R_t$ (Chain-of-Thought Reasoning):** Multi-step reasoning generated during the turn. **Permanently discarded** after the state transition is validated and applied.
- **$\Delta \Sigma_t$ (State Patch):** JSON update merged into state via $\Sigma_{t+1} = \Sigma_t \oplus \Delta \Sigma_t$ with null-deletion semantics.
- **$a_t$ (Synchronous Action):** Tool call executed synchronously via the harness.

### Complexity Guarantee
- **Prompt Size:** $|\mathcal{C}_t| = \mathcal{O}(1)$ (bounded, flat across hundreds of steps).
- **Cumulative Tokens:** $\mathcal{O}(T)$ (strictly linear in steps, defeating the quadratic $\mathcal{O}(T^2)$ trap).

---

## Core Systems Invariants

Superstate enforces three hard systems invariants:

### Invariant I: Native Execution Ownership (No Wrapper Tax)
The turn loop runs directly in compiled Rust (`crates/superstate-agent`). Tool calls execute synchronously with strict wall-clock deadlines. There are no background polling turns, no uninterceptable external loops, and no async sleep loops.

### Invariant II: The Sealed Verification Gate (No LLM Self-Grading)
An LLM must never grade its own homework. A gate the model can invoke is a gate it can decline to invoke, skip, or report a fake passing verdict for.
- Completion is decided solely by an external command (`verification_cmd`) that exits 0.
- The gate is **completely absent from the model's tool surface**. The runtime evaluates the gate only when the model stops calling tools.

### Invariant III: Verified Checkpoint Rollback
A failing attempt's edits must not accumulate across retries. When a gate evaluation fails:
- The workspace is deterministically rolled back via `git reset --hard` to the baseline checkpoint.
- The failure telemetry is preserved and passed into $\Sigma_{t+1}$.
- The agent begins each retry from a clean working tree armed with the exact error output of the previous attempt.

---

## Execution Seams: Shell & Fs Parity

To ensure the harness can run anywhere without changing the agent's behavior, execution and file access are abstracted into seams:

- **`Shell` Seam (`LocalShell`, `ContainerShell`):**
  - **Process Groups (`setpgid(0)` + `kill(-pgid, SIGKILL)`):** All spawned commands run in their own process group. When a deadline expires, the entire process tree is terminated, eliminating leaked grandchild processes (e.g. runaway `find /`) and pipe-drain deadlocks.
  - **Seam Parity:** Both `LocalShell` and `ContainerShell` enforce deadlines, drain stdout/stderr pipes concurrently, return exit code 124 on timeout, and preserve partial output.
- **`Fs` Seam (`LocalFs`, `ContainerFs`):**
  - Decouples file reading and writing from command execution so containerized or remote sandboxes do not suffer the split where an agent edits the host while verifying in a container.

---

## Workspace Structure

```
superstate/
├── crates/
│   ├── superstate-agent/     # Core SKILL.state runtime, turn loop, tools, shell/fs seams
│   ├── superstate-provider/  # Provider client layer (Anthropic, Gemini, OpenAI)
│   ├── superstate-core/      # State schemas, dictionary merge operators, serialization
│   └── superstate-cli/       # Command-line inspection and utilities
├── examples/                 # End-to-end evaluation harness runners
└── tests/                    # Integration and regression test suites
```

---

## Quick Start

### Build and Install

```bash
# Build the workspace
cargo build --release

# Run all unit and integration tests
cargo test
```

### Interactive Agent Harness (REPL)

Running `superstate` without a prompt launches the interactive agent harness REPL with live turn event streaming, tool outcome indicators, gate verification, and slash commands:

```bash
# Start interactive REPL in the current repository
superstate

# Launch REPL configured with a specific verification gate and model
superstate --gate "cargo test" --model gemini-3.8-flash --context state-only
```

### Headless Single-Shot Solver

Provide a prompt argument to run Superstate autonomously in headless mode. The harness attempts the task, evaluates the external verification gate, automatically rolls back on failure, and exits 0 only when the gate passes:

```bash
# Autonomous bug fix with fail-closed gate verification
superstate "fix parsing bug in token lexer" --gate "cargo test -p my-crate --test lexer"

# Run with SKILL.state O(1) bounded memory and container isolation
superstate "refactor database migration" \
  --gate "pytest tests/test_migrations.py" \
  --context state-only \
  --container alpine-dev \
  --max-turns 30 \
  --max-gates 3
```

---

## Command-Line Interface (CLI)

### CLI Execution Flags

| Flag | Description | Default |
|---|---|---|
| `[PROMPT]` | Direct task prompt for single-shot / headless execution | `None` (launches interactive REPL) |
| `-g, --gate <CMD>` | External verification gate command (e.g. `'cargo test'`, `'pytest'`) | `None` (fail-closed disabled) |
| `--context <STRATEGY>` | Context management strategy: `state-only` ($\mathcal{O}(1)$ prompt bounded memory) or `conversational` (append-all chat) | `conversational` |
| `-m, --model <MODEL>` | Model identifier or provider selection override | `gemini-3.8-flash` |
| `--container <ID>` | Run execution and filesystem seams inside an isolated container workspace | Local host |
| `--max-turns <N>` | Maximum turn budget per attempt | `25` |
| `--max-gates <N>` | Maximum verification gate retry attempts before failing | `5` |
| `--rollback[=<BOOL>]` | Deterministic rollback to baseline git checkpoint on gate failure | `true` |

### Interactive REPL Slash Commands

Inside the interactive REPL (`superstate`), the following slash commands control session execution and state:

| Slash Command | Description |
|---|---|
| `/help` | Display available slash commands and usage reference |
| `/model [name\|idx]` | Discover available cloud models and switch active model dynamically |
| `/state` | Inspect active SKILL.state partitions (`Orchestrator`, `Runtime`, `Agent`) |
| `/diff` | Display uncommitted git diff in the current working tree |
| `/review` | Run the adversarial code review engine over uncommitted changes |
| `/doctor` | Run 6-step system verification handshake and health check |
| `/gate [cmd]` | Inspect current verification gate or update it dynamically (e.g. `/gate cargo test`) |
| `/clear` | Clear session conversation history and reset runtime working state |
| `/exit`, `/quit` | Exit the interactive REPL |

### Cloud Model Providers & Model Discovery

Superstate natively integrates with multiple cloud AI providers without external proxy wrappers. Set credentials in your environment to enable automatic discovery and routing:

| Provider | Supported Identifiers | Environment Variables | Description |
|---|---|---|---|
| **Google Gemini** | `gemini-3.8-flash` (default), `gemini-2.5-flash`, `gemini-2.5-pro` | `GEMINI_API_KEY` (or `GOOGLE_API_KEY`) | Generative Language API v1beta with cryptographic thought signatures |
| **Anthropic Claude** | `claude-3-7-sonnet-latest`, `claude-3-5-sonnet-latest`, `claude-3-5-haiku-latest` | `ANTHROPIC_API_KEY` | Anthropic Messages API with prompt caching awareness |
| **Ollama Cloud & Remote** | `qwen2.5-coder:32b`, `llama3.3:70b`, `deepseek-r1:32b`, etc. | `OLLAMA_API_KEY`, optional `OLLAMA_HOST` | Ollama Cloud (`https://ollama.com/api`) or custom remote/local daemon |

#### Dynamic Model Selection (`/model`)

Run `/model` inside the REPL to probe configured cloud credentials and inspect available models:

```text
superstate > /model
Discovering available cloud models...

[Google Gemini] (via GEMINI_API_KEY)
  [1] gemini-3.8-flash (active) - Next-generation multimodal fast model
  [2] gemini-2.5-flash          - Agentic reasoning model
  [3] gemini-2.5-pro            - Deep reasoning model

[Anthropic Claude] (via ANTHROPIC_API_KEY)
  [4] claude-3-7-sonnet-latest  - Frontier hybrid reasoning & coding

[Ollama Cloud] (via OLLAMA_API_KEY at https://ollama.com/api)
  [5] qwen2.5-coder:32b         - Cloud-hosted coding model

Active model: gemini-3.8-flash
To switch: /model <number> or /model <name> (e.g. /model 1 or /model claude-3-7-sonnet-latest)
```

Switch active models by numeric index or model identifier:

```text
superstate > /model 4
Switched active model to: claude-3-7-sonnet-latest (anthropic)

superstate > /model gemini-2.5-pro
Switched active model to: gemini-2.5-pro (Gemini)
```

### Core Subcommands

| Subcommand | Description |
|---|---|
| `superstate doctor` | Run 6-step verification handshake and health check |
| `superstate mcp` | Launch standard JSON-RPC Model Context Protocol (MCP) server over stdio |
| `superstate review` | Run adversarial code review engine over git diff or commit ranges |
| `superstate state` | Manage SKILL.state lifecycle (`init`, `apply`, `checkpoint`, `resume`, `show`) |
| `superstate worktree` | Manage isolated git worktree environments and COW caches |
| `superstate daemon` | Run autonomous background daemon over milestones with DAG dependencies |
| `superstate cancel` | Cancel running background daemon processes |
| `superstate logs` | Inspect background daemon logs and telemetry |
| `superstate handoff` | Generate standardized cross-agent handoff packages |
| `superstate shunt` | Execute commands with output shunting distillation for token efficiency |
| `superstate inspect` | Inspect AST symbols, structural outlines, and code slices |
| `superstate guard` | Intercept and evaluate agent tool invocations before execution |

---

## References

- Badhe et al. (2026). *SKILL.state: Scalable Long-Horizon Agent Skills.* [arXiv:2608.26263v2](https://arxiv.org/html/2608.26263v2).

