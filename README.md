<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/blyte-banner-dark.svg">
    <img src="docs/assets/blyte-banner-light.svg" alt="blyte: a lean coding-agent harness" width="100%">
  </picture>
</p>

<p align="center">
  <a href="https://github.com/climate-x-org/superstate/actions/workflows/ci.yml"><img src="https://github.com/climate-x-org/superstate/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-3E6D64" alt="License: Apache-2.0 OR MIT">
  <img src="https://img.shields.io/badge/rust-stable-A6441A" alt="Rust stable">
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-56616A" alt="macOS and Linux">
</p>

**blyte** is a coding-agent harness in Rust. The model does the work; blyte runs the loop, the
tools and a **sealed verification gate**, and spends as little as possible of its own time and the
model's tokens doing it. Give it a task and a check command: it does the work and proves it, and
the check, not the model, decides when it is done.

```bash
blyte "handle empty input in parse() and add a test" --gate "cargo test"
```

| | |
|---|---|
| **0.47×** the time, **0.63×** the cost | median, against agy (Google's Antigravity CLI) on the same 31 tasks, same model, same commit |
| **19 / 27** FrontierHarness tasks | on Gemini 3.8 Flash; the best published Kimi K3 configuration passed 17 / 27 |

Both come with caveats (one model, our own repositories, a single run on a different model from
the leaderboard's); [the evidence page](docs/evidence.md) has the method and all of them.

Named after rust fungus (the blight Rust is named after), and bytes.

## Why blyte

- **"Done" means something.** `--gate` is a command, not a tool: the model cannot call it, skip it
  or grade itself. Every step of a multi-line gate runs and every failure comes back at once. A
  failed attempt can roll back to where the run started.
- **The turn is the unit of cost, so blyte saves turns.** Whole-file reads, all gate failures in
  one message, quick checks before the slow ones, and an append-only prompt that keeps the
  provider's cache valid, with old turns folded into a harness-written digest.
- **The harness gets out of the way.** A persistent shell session costs about 0.7 ms per command;
  `grep` and `glob` run in-process on ripgrep's libraries; no tool result can flood the context.
- **Sandboxed by default.** Commands cannot write outside the workspace (macOS Seatbelt, Linux
  Landlock), read credential stores or `.env` files (macOS), or see credential-looking environment
  variables; `--network=off` keeps code on the machine.
- **Honest numbers.** `--report` writes one JSON record per run: outcome, turns, true cost,
  per-call usage, where the time went, and the run's security settings.
- **Small on purpose.** About 7,200 lines of Rust. Features that did not win in measurement (a
  daemon, an MCP server, a planner, a TDD mode, LLM summaries, …) were deleted.
  [The manifesto](MANIFESTO.md) explains the rules.

blyte is an **implementer**: one task, done and proven. Planning, splitting work and reviewing
belong to whoever drives it, a person or an orchestrating agent.

## Install

```bash
cargo install --git https://github.com/climate-x-org/superstate blyte-cli --locked
blyte --version
```

## Quick start

```bash
export GEMINI_API_KEY=...          # or pick another provider with --model

# A gated run: exits 0 only when the check passes
blyte "fix the off-by-one in the lexer" --gate "cargo test -p lexer"

# A gate of several steps: all run, all failures are reported together
blyte "handle empty input in parse()" --gate 'cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test'

# Budgets and a report
blyte "..." --gate "pytest" --max-seconds 1800 --max-cost 2 --report run.json

# Inside a running container
blyte "make the migration idempotent" --gate "pytest tests/test_migrations.py" \
  --container my-dev --container-workdir /app

# Keep the code on the machine
blyte "..." --gate "cargo test --offline" --network=off

# Interactive
blyte
```

> [!WARNING]
> With rollback on (the default), a failed gate resets the tree to the commit the run started
> from, discarding uncommitted changes. Start from a clean tree, or run blyte in its own worktree.

## Providers

The model name picks the provider:

| model name | provider | credentials |
|---|---|---|
| `gemini-3.8-flash-high` (default) | Google Gemini (`-low`/`-medium`/`-high` thinking) | `GEMINI_API_KEY` |
| `claude-…`, `anthropic/…` | Anthropic, with prompt caching | `ANTHROPIC_API_KEY` |
| `openai/<model>` | any OpenAI-compatible API, including local vLLM / LM Studio | `OPENAI_API_KEY`, `OPENAI_BASE_URL` |
| `openrouter/<vendor>/<model>` | OpenRouter | `OPENROUTER_API_KEY` |
| `<name>:<tag>`, `ollama/…` | Ollama, local or cloud | `OLLAMA_HOST` or `OLLAMA_API_KEY` |
| `chatgpt/<model>` | a ChatGPT subscription, via the Codex CLI's login | `~/.codex/auth.json` (read only) |

Credentials are read by blyte and never reach the commands the model runs. Details:
[docs/providers.md](docs/providers.md).

## Documentation

| | |
|---|---|
| [Getting started](docs/getting-started.md) | install, pick a model, the first gated run |
| [Running tasks](docs/usage.md) | the gate, budgets, rollback, containers, driving blyte from an orchestrator |
| [Security and the sandbox](docs/security.md) | what commands can and cannot reach, per platform |
| [Run reports](docs/reports.md) | every field of `--report` |
| [How it works](docs/how-it-works.md) | the loop, folded context, the tools |
| [Evidence](docs/evidence.md) | the paired runs, FrontierHarness, and their caveats |
| [Manifesto](MANIFESTO.md) | fast and good, no bunk |

The docs are also a site: `uvx --with mkdocs-material mkdocs serve`.

## Contributing

Contributions are welcome; read [CONTRIBUTING.md](CONTRIBUTING.md) first. The short version: the
checks are `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test
--workspace --all-targets`, and a claim that something is faster needs a paired measurement.
Security reports: [SECURITY.md](SECURITY.md).

## License

Licensed under either of [Apache License 2.0](LICENSE) or MIT, at your option.
