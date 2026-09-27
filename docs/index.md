---
hide:
  - navigation
  - toc
---

# blyte { .b-hidden }

<div class="b-hero" markdown>
<p class="b-eyebrow">A coding-agent harness · Rust · Apache-2.0 OR MIT</p>
<p class="b-wordmark">blyte<span>.</span></p>
<p class="b-thesis">The model does the work. The harness decides the bill, the wait, and whether <em>done</em> means anything.</p>
</div>

blyte runs the loop, the tools and a **sealed verification gate** around any capable model, and
spends as little as possible of its own time and the model's tokens doing it. Give it a task and a
check command; it does the work and proves it, and the check, not the model, decides when it is
finished.

<div class="b-stats">
  <div class="b-stat"><b>0.47<small>×</small></b><span>agy's time, median, on the same tasks</span><i>31 paired runs · same model · faster in 27</i></div>
  <div class="b-stat"><b>0.63<small>×</small></b><span>agy's cost, median</span><i>cheaper in 29 of 31 · list price</i></div>
  <div class="b-stat"><b>19<small>/27</small></b><span>FrontierHarness tasks passed on Gemini 3.8 Flash</span><i>best Kimi K3 configuration: 17/27 · one run</i></div>
</div>

!!! note "Read the numbers with their caveats"
    The paired runs are 31 real tasks in two of our own repositories, implemented by blyte and by
    agy (Google's Antigravity CLI) on the same model from the same commit. The FrontierHarness
    result is one run, on a different model from the leaderboard's, over the 27 of 30 tasks that
    could be matched. [Evidence](evidence.md) has the method and every caveat.

```bash
cargo install --git https://github.com/climate-x-org/superstate blyte-cli --locked
export GEMINI_API_KEY=...
blyte "fix the off-by-one in the lexer" --gate "cargo test -p lexer"
```

<div class="b-cards">
<a class="b-card" href="getting-started/"><strong>Getting started</strong><span>Install, pick a model, run your first gated task.</span></a>
<a class="b-card" href="how-it-works/"><strong>How it works</strong><span>One loop, a sealed gate, folded context, fast bounded tools.</span></a>
<a class="b-card" href="security/"><strong>Security and the sandbox</strong><span>What commands can and cannot touch, on macOS and Linux.</span></a>
<a class="b-card" href="manifesto/"><strong>Manifesto</strong><span>Fast and good. No bunk. Why blyte is built this way.</span></a>
</div>

## What blyte is for

blyte is an **implementer**. It takes one task, does it, and proves it with the gate. Deciding what
to build, splitting work and judging whether a result is complete belong to whoever drives it: a
person at the terminal, or an orchestrating agent that dispatches tasks to blyte and reviews the
results. That narrow job is why it has no planner, no review mode and no memory across runs.

## What is in it

- **A sealed gate.** `--gate` is a command; in a gated run, its exit 0 is the only way to
  `verified`. The model cannot call it, skip it or grade itself. A failed attempt can roll back to the commit the run started from.
- **Folded context.** The prompt only grows at the end, so it stays cached; past a budget the
  oldest turns fold into a one-line-per-call digest written by the harness, not a model.
- **Fast, bounded tools.** `read`, `write`, `edit`, `bash`, `grep`, `glob`. A persistent shell
  session costs about 0.7 ms per command; search runs in-process on ripgrep's libraries; no single
  result can flood the context.
- **Sandboxed by default.** No writes outside the workspace, no reads of credential stores, no
  credential-looking environment variables, and optionally no network.
- **Honest reports.** `--report` writes one JSON record per run: outcome, turns, true cost,
  per-call usage, where the time went, and the run's security settings.
- **Six providers.** Gemini, Anthropic, any OpenAI-compatible API, OpenRouter, Ollama, and a
  ChatGPT subscription through the Codex CLI's login.
