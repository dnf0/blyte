---
hide:
  - navigation
---

# blyte documentation

blyte is a coding-agent harness: you give it a task and a check command, a model does the work
through blyte's tools, and the check, not the model, decides when it is done. It runs on macOS and
Linux, headless or as a REPL, with any of six model providers.

## Install and first run

```bash
cargo install --git https://github.com/climate-x-org/superstate blyte-cli --locked

export GEMINI_API_KEY=...     # or another provider: see Providers
cd your-repo                  # a git repository with a clean working tree
blyte "handle empty input in parse() and add a test" --gate "cargo test"
```

blyte exits `0` when the gate passes. [Getting started](getting-started.md) walks through this in
more detail.

## Find what you need

<div class="b-cards">
<a class="b-card" href="getting-started/"><strong>Get set up</strong><span>Install, choose a model, run a first gated task, use the REPL.</span></a>
<a class="b-card" href="usage/"><strong>Run tasks well</strong><span>Gates with several steps, budgets, rollback, containers, driving blyte from an orchestrator.</span></a>
<a class="b-card" href="providers/"><strong>Choose a model</strong><span>Gemini, Anthropic, OpenAI-compatible, OpenRouter, Ollama, ChatGPT; credentials, retries, cost.</span></a>
<a class="b-card" href="security/"><strong>Know what commands can touch</strong><span>The sandbox on macOS and Linux, credential reads, the environment filter, <code>--network=off</code>.</span></a>
<a class="b-card" href="reports/"><strong>Read a run's results</strong><span>Every field of <code>--report</code>: outcome, turns, cost, timing, security settings.</span></a>
<a class="b-card" href="reference/cli/"><strong>Look up a flag</strong><span>The full <code>blyte --help</code> and the environment variables blyte reads.</span></a>
<a class="b-card" href="how-it-works/"><strong>Understand the design</strong><span>The loop, the sealed gate, folded context, the tools and their limits.</span></a>
<a class="b-card" href="contributing/"><strong>Contribute</strong><span>Set-up, the checks CI runs, how changes are measured and reviewed.</span></a>
</div>

## Key ideas

**The gate.** `--gate` is a command, not a tool the model can call. When the model says it is
finished, blyte runs every step of the gate; exit 0 is `verified`, anything else goes back to the
model with all the failures at once. See [Running tasks](usage.md#the-gate).

**Rollback.** By default a failed gate resets the tree to the commit the run started from,
discarding uncommitted changes. Start from a clean tree or a dedicated worktree.

**Budgets.** Every run is bounded by turns (`--max-turns`) and gate attempts (`--max-gates`), and
optionally by wall-clock time (`--max-seconds`) and spend (`--max-cost`).

**The sandbox.** On by default: commands cannot write outside the workspace, and on macOS cannot
read credential stores. See [Security](security.md).

**One job.** blyte implements one task per run. Planning, splitting work and review belong to
whoever drives it, a person or another agent.

## Background

- [Manifesto](manifesto.md): the design rules and why they exist.
- [Evidence](evidence.md): how blyte has been measured, and the caveats.
- [Architecture decisions](adr/0001-progressive-staged-verification.md) and
  [research notes](research/2026-09-24-harness-research.md).
