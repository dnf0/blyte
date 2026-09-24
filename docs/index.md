# Blyte

**Blyte** is a high-performance, deterministic Rust agent harness implementing the **[SKILL.state](https://arxiv.org/html/2608.26263v2)** architecture ([arXiv:2608.26263v2](https://arxiv.org/html/2608.26263v2)) for autonomous software engineering.

It replaces append-only conversational history with explicit, mutable execution state transitions $(P, \Sigma_t, O_t) \to (R_t, \Delta \Sigma_t, a_t)$, maintaining a strictly bounded $\mathcal{O}(1)$ prompt footprint and linear $\mathcal{O}(T)$ cumulative token complexity across long-horizon execution.

## Core Features

- ⚡ **SKILL.state Runtime (`blyte-agent`):** Discrete state transitions where intermediate reasoning traces ($R_t$) are discarded after each turn, preventing prompt growth.
- 🛡️ **Sealed Verification Gate:** Completion is decided strictly by runtime evaluation of an external command (`verification_cmd`); the gate is absent from the model's tool surface to prevent self-grading.
- 🔄 **Verified Checkpoint Rollback:** On verification failure, git working trees are automatically reset to baseline while preserving failure telemetry in the state.
- 🐚 **Seam Parity (`LocalShell` & `ContainerShell`):** Synchronous command execution with Unix process groups (`setpgid(0)` + `kill(-pgid, SIGKILL)`), preventing orphaned grandchildren and pipe-drain deadlocks.
- 📦 **Pluggable Providers (`blyte-provider`):** Streaming provider interfaces for Anthropic, Gemini, and OpenAI.

## Quick Start

### Build and Test

```bash
cargo build
cargo test
```

### Run an Evaluation Task

```bash
cargo run -p blyte-agent --release --example solve
```
