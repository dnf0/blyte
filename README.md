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

### Build and Test

```bash
# Build the workspace
cargo build

# Run all unit and integration tests
cargo test
```

### Running an Evaluation Task

```bash
# Run the end-to-end solve runner against a sample task
cargo run -p superstate-agent --release --example solve
```

---

## References

- Badhe et al. (2026). *SKILL.state: Scalable Long-Horizon Agent Skills.* [arXiv:2608.26263v2](https://arxiv.org/html/2608.26263v2).
