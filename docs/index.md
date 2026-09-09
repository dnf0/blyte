# Superstate

**Superstate** is a high-performance, deterministic execution state engine, polyglot AST code intelligence platform, daemon-less adversarial code reviewer, and Model Context Protocol (MCP) server written in Rust.

## Features

- ⚡ **Deterministic State Engine (`superstate-core`):** DFA state machine with atomic file-locking (`fd-lock`), checkpoint/rollback safety, and append-only epistemic ledger.
- 🌳 **Polyglot Code Intelligence (`superstate-syntax`):** Native Tree-sitter AST symbol extractor across Rust, Python, TypeScript, and Go.
- 🛡️ **Daemon-less Adversarial Review (`superstate-review`):** Zero-noise static analysis and LLM review integration without background daemons.
- 📜 **Guidance Generator (`superstate-rules`):** Minijinja template renderer for `AGENTS.md`, `CLAUDE.md`, and Cursor rules.
- 🔌 **Native Stdio MCP Server (`superstate-mcp`):** Zero-dependency JSON-RPC 2.0 stdio server providing seamless integration with Cursor, Claude Code, and Antigravity.
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
