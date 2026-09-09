# Superstate

Deterministic execution state engine, polyglot AST code intelligence, daemon-less adversarial reviewer, and native Model Context Protocol (MCP) server for autonomous coding agents.

## Why Superstate?

Autonomous agents tackling complex multi-step refactoring, migration, and implementation pipelines suffer from context rot, non-deterministic drift, and lack of verifiable verification gates. **Superstate** addresses these challenges by replacing unbounded agent memory with a deterministic, fail-closed state machine, AST-level code intelligence, and native stdio MCP integration.

## Key Capabilities

- ⚡ **Deterministic State Engine (`superstate-core`)**: Mathematical DFA execution engine backed by `fd-lock` atomic file locks, checkpointing, and an append-only epistemic ledger.
- 🌳 **Polyglot Code Intelligence (`superstate-syntax`)**: High-speed symbol and AST hierarchy extraction powered by Tree-sitter across Rust, Python, TypeScript, and Go.
- 🛡️ **Zero-Noise Code Review (`superstate-review`)**: Daemon-less, git diff-driven adversarial code review system returning structured JSON findings.
- 📜 **Guidance & Rule Engine (`superstate-rules`)**: Deterministic Minijinja templates for generating universal agent guidance (`AGENTS.md`, `.cursor/rules`).
- 🔌 **Native Stdio MCP Server (`superstate-mcp`)**: Lightweight JSON-RPC 2.0 stdio server connecting state engine tools directly into Cursor, Claude Code, and Antigravity.
- 🩺 **5-Step Fail-Closed Doctor Handshake**: Automated verification probe ensuring binary integrity, active MCP loopback, client configurations, and file locks.

## Installation

```bash
# Clone the repository
git clone git@github.com:climate-x-org/superstate.git
cd superstate

# Install binaries locally
cargo install --path crates/superstate-cli
```

## Quick Start

### 1. Verify Installation
```bash
superstate doctor
```

### 2. Initialize Plan State Tracking
```bash
state init --plan docs/plan.md
```

### 3. Check State Status
```bash
state status
```

### 4. Advance State with Multi-Gate Verification
```bash
state next --verify-cmd "cargo test --workspace" --verify-git-clean
```

### 5. Record Epistemic Ledger Notes
```bash
state note "[discovery] Observed cache invalidation boundary in parser"
```

## Development & Verification

Ensure you have a modern Rust toolchain (2024 edition compatible):

```bash
# Run all workspace tests
cargo test --workspace --all-targets

# Check formatting
cargo fmt --all --check

# Run linter
cargo clippy --all-targets -- -D warnings
```

## Architecture

```
superstate/
├── crates/
│   ├── superstate-core/    # DFA state machine, atomic persistence, plan parser, verifier
│   ├── superstate-syntax/  # Polyglot Tree-sitter AST symbol extractor
│   ├── superstate-review/  # Adversarial code reviewer and git diff engine
│   ├── superstate-rules/   # Minijinja rules generator
│   ├── superstate-mcp/     # Native stdio JSON-RPC MCP server
│   └── superstate-cli/     # Unified CLI (superstate) and state runner (state)
```

## Contributing

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details on our code of conduct, conventional commit guidelines, and PR workflow.

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.
