# Superstate

**Superstate** is a high-performance, deterministic execution state engine, polyglot AST code intelligence platform, daemon-less adversarial code reviewer, and Model Context Protocol (MCP) server written in Rust.

> 📖 **Read [The Superstate Manifesto](../MANIFESTO.md)** to understand the architectural doctrine, the separation of Worker and Control planes, and the Five Invariants of autonomous execution.

## Features

- ⚡ **Deterministic State Engine (`superstate-core`):** DFA state machine with atomic file-locking (`fd-lock`), checkpoint/rollback safety, and append-only epistemic ledger.
- 🌳 **Polyglot Code Intelligence (`superstate-syntax`):** Native Tree-sitter AST symbol extractor across Rust, Python, TypeScript, and Go.
- 🛡️ **Daemon-less Adversarial Review (`superstate-review`):** Zero-noise static analysis and LLM review integration without background daemons.
- 📜 **Guidance Generator (`superstate-rules`):** Minijinja template renderer for `AGENTS.md`, `CLAUDE.md`, and Cursor rules.
- 🔌 **Detached Handoff Gateway (`superstate-mcp`):** Strict 4-tool JSON-RPC 2.0 stdio gateway connecting chat sessions into autonomous background worktrees (Cursor, Claude Code, Antigravity).
- 🚀 **Native Daemon Harness (`superstate state daemon`):** Autonomous, out-of-loop worker driver with OS verification gates, retry error-injection, and automatic worktree merge.
- 🩺 **5-Step Fail-Closed Doctor Handshake:** Automated end-to-end verification verifying binary integrity, MCP loopback, client configs, and file locks.

## Quick Start

### Installation

```bash
cargo install superstate-cli
```

### 5-Step Health Check

```bash
superstate doctor
```

### Initialize State Tracking

```bash
state init --plan docs/plan.md
```

### Advance Active Task

```bash
state next --verify-cmd "cargo test" --verify-git-clean
```
