# Blyte

A lean coding-agent harness in Rust. The model does the work; blyte runs the loop, the tools and
the verification gate, and spends as little as possible of its own time and the model's tokens
doing it.

Named after rust fungus (the blight Rust is named after), and bytes.

Why it is built this way: [MANIFESTO.md](MANIFESTO.md).

Blyte is built to be an **implementer**: give it a task and a check command, and it does the work
and proves it. Planning, splitting work and judging completeness belong to whoever drives it, a
person or an orchestrating agent.

## What it is

- **One loop, owned end to end** (`crates/blyte-agent`). The model's tool calls run synchronously
  under deadlines. No wrappers, hooks or background polling.
- **A sealed verification gate.** Completion is decided by a command you give (`--gate`) exiting
  0. The gate is not in the model's tool surface, so it cannot be skipped or self-graded. A gate
  of several lines is several steps: every step runs and all failures come back in one message.
  On failure the tree can roll back to the last verified commit (`--rollback`, default on).
- **Folded context** (default). The prompt only grows at the end. Past a budget (`--fold-budget`,
  48K tokens), the oldest turns fold into a one-line-per-call digest, so the prompt stays bounded
  and cached. `--context conversational` keeps everything, as a control.
- **Fast tools.**
  - Persistent shell sessions: about 0.7 ms of harness overhead per command. The model's `cd` and
    `export` carry over between commands, as in a terminal.
  - `grep` and `glob` run in-process on ripgrep's libraries, respecting `.gitignore`.
  - Container file I/O goes through the same session.
- **Bounded output.** No single tool result can flood the context (500-character lines, 60K
  characters per result). Command output keeps 400 lines; `read` returns whole files up to 2,000
  lines, because every extra turn re-sends the prompt.
- **Honest telemetry.** `--report` writes a JSON record per run: outcome, turns, true cost,
  per-call usage split into cache hits and misses, and the run's security settings. `--max-cost`
  stops a run once its list-price spend reaches a cap.
- **Sandboxed by default** (`--sandbox=off` to disable). Commands run on the host cannot:
  - write outside the workspace, its git directory, temp directories and tool caches (macOS
    Seatbelt; Linux Landlock);
  - read credential stores (`~/.ssh`, `~/.aws`, `~/.config/gh`, keychains, browser profiles, …) or
    any `.env` (macOS only; `--allow-read PATH` grants a path back);
  - see environment variables that look like credentials (`*_TOKEN`, `*_KEY`, URLs with passwords;
    `--pass-env NAME` keeps one);
  - with `--network=off`, connect out (macOS: localhost and DNS still work, Docker is refused;
    Linux 6.7+: every TCP connect is blocked).

## Evidence

**Paired shadow runs.** blyte and agy implement the same plan task from the same commit, side by
side, and both results are checked by the repository's full check and reviewed. Every change to
blyte (and to its evaluation repo) is made this way, and every run goes into a ledger.

| task | agy | blyte | blyte ÷ agy (time, cost) | result |
|---|---|---|---|---|
| `read` returns whole files | 382 s, $0.47 | 266 s, $0.25 | 0.70×, 0.54× | identical diffs |
| gate reports every failure | 460 s, $0.35 | 368 s, $0.24 | 0.80×, 0.70× | identical diffs |
| read from the top returns the whole file | 320 s, $0.30 | 349 s, $0.16 | 1.09×, 0.55× | identical diffs |
| quick checks before stopping | 138 s, $0.16 | 89 s, $0.11 | 0.64×, 0.69× | identical diffs |
| build stamp in `--version` and reports | 160 s, $0.21 | 89 s, $0.18 | 0.56×, 0.83× | identical diffs |
| eval: remove the MCP arms (−2,680 lines) | 481 s, $0.98 | 288 s, $0.76 | 0.60×, 0.78× | blyte's code better, wrong commit message |
| eval: remove the daemon and relay arms (−3,510) | 492 s, $0.71 | 231 s, $0.47 | 0.47×, 0.66× | blyte better (agy dropped valid tests) |
| eval: remove the state-engine arms (−2,780) | 1030 s, $1.64 | 300 s, $0.84 | 0.29×, 0.51× | blyte merged; agy stopped on a plan gap |
| run telemetry in the report | 394 s, $0.77 | 237 s, $0.42 | 0.60×, 0.55× | same logic; agy's merged (doc comments) |
| eval: remove 34 dead launch scripts | 249 s, $0.36 | 116 s, $0.26 | 0.47×, 0.72× | identical |
| OpenAI-compatible provider | 563 s, $0.73 | 256 s, $0.56 | 0.45×, 0.77× | equal; blyte's merged |
| `read` keeps long lines | 292 s, $0.42 | 113 s, $0.19 | 0.39×, 0.45× | identical diffs |
| commit footer (an experiment, later removed) | 732 s, $1.10 | 266 s, $0.52 | 0.36×, 0.47× | equal |
| Anthropic prompt caching | 372 s, $0.51 | 130 s, $0.24 | 0.35×, 0.47× | near-identical |
| eval: time in the harness comparison | 1661 s, $0.74 | 3024 s, $0.47 | 1.82×, 0.64× | agy's merged: blyte's run cleared a host cache after a gate failure |
| remove the commit footer | 216 s, $0.30 | 112 s, $0.19 | 0.52×, 0.63× | blyte's clean revert merged |
| blyte states the workspace boundary | 300 s, $0.41 | 109 s, $0.16 | 0.36×, 0.39× | identical |
| keep failed gate output in the report | 840 s, $1.36 | 365 s, $1.00 | 0.43×, 0.74× | equal; both needed a plan fix |
| eval: CI workflow | 561 s, $0.46 | 125 s, $0.21 | 0.22×, 0.45× | equal |
| eval: offline agent, online grader | 572 s, $0.98 | 303 s, $0.73 | 0.53×, 0.74× | equal |
| honest completion prompt when ungated | 255 s, $0.34 | 118 s, $0.15 | 0.46×, 0.44× | equal |
| ChatGPT-login provider | 714 s, $1.12 | 407 s, $0.80 | 0.57×, 0.72× | equal |
| macOS write sandbox | 767 s, $1.51 | 1414 s, $4.30 | 1.84×, 2.85× | agy's merged: blyte's session broke when its temp directory was deleted (a harness bug, fixed next) |
| session recreates its temp directory | 253 s, $0.34 | 291 s, $0.44 | 1.15×, 1.29× | equal |
| `--max-cost` | 514 s, $1.01 | 400 s, $0.98 | 0.78×, 0.97× | agy's merged: blyte edited an existing test |
| sandbox refuses credential reads | 453 s, $0.72 | 186 s, $0.46 | 0.41×, 0.63× | equal; both needed a compatibility fix |
| withhold credential-looking env vars | 549 s, $0.91 | 237 s, $0.54 | 0.43×, 0.60× | equal |
| Linux Landlock sandbox | 632 s, $1.18 | 288 s, $0.71 | 0.46×, 0.60× | equal |
| quota fails fast; login errors | 398 s, $0.60 | 207 s, $0.39 | 0.52×, 0.64× | equal |
| `--network=off` | 907 s, $1.51 | 293 s, $0.79 | 0.32×, 0.52× | equal; blyte's commit message fixed |
| security settings in the report | 476 s, $0.81 | 177 s, $0.37 | 0.37×, 0.46× | equal |

Thirty-one pairs, two repositories, Rust and Python (the table shows a selection; the ledger has
all of them): blyte took a median **0.47× the time and 0.63× the cost**, was cheaper in 29 of 31
and faster in 27. Its result was mergeable as-is in 24 of 31 (agy: 26 of 31; "mergeable" includes
equivalent results that were not merged because the other was faster). Quality has been level since
the two prompts were put on identical rules; blyte's two serious misses (a host-cache detour, and a
session broken by a deleted temp directory) each led to a harness fix now built into blyte.

**FrontierHarness tasks** (one run, 2026-09-27): on the 27 of FrontierHarness v1.0's 30 tasks that
could be matched, blyte with Gemini 3.8 Flash passed **19/27 (70%)**; the best published Kimi K3
configuration, Codex, passed 17/27 (63%), and the others 12–16. It was a single run on a different
model, so read it as "competitive with the best", not "better". Details and caveats:
[docs/research/2026-09-27-frontierharness-gemini.md](docs/research/2026-09-27-frontierharness-gemini.md).

**Benchmark tasks**, measured with `superstate-eval` (same model, tasks and sealed grader), 3 runs
per cell:

| task | agy | blyte (`folded`) |
|---|---|---|
| build-cython-ext | 3/3, $1.45–1.70, 816–902 s | 3/3, **$1.32–1.43, 539–592 s** |
| scc-bounded-memory-spilling | **0/3** (314/317) | **2/3** (317/317), $2.34–3.07, 772–1263 s |

The harness's own overhead per 200 turns (`harness_bench`, model time excluded) is 4.2 s in a
container and 1.7–2.0 s locally, down from 36–43 s and 7.9 s. See
`docs/research/2026-09-24-harness-research.md` for what was measured and why things are the way
they are.

## Quick start

```bash
cargo install --path crates/blyte-cli     # installs `blyte`

# Headless: exits 0 only when the gate passes
blyte "fix the off-by-one in the lexer" --gate "cargo test -p lexer"

# A gate of several steps: all of them run, all failures are reported together
blyte "handle empty input in parse()" --gate 'cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test'

# In a running container
blyte "make the migration idempotent" --gate "pytest tests/test_migrations.py" \
  --container my-dev --container-workdir /app

# Interactive
blyte
```

**Providers.** `BLYTE_MODEL` or `--model` picks the model, and its name picks the provider:

| model name | provider | credentials |
|---|---|---|
| `gemini-3.8-flash-high` (default) | Google Gemini; `-low`/`-medium`/`-high` sets the thinking level | `GEMINI_API_KEY` |
| `claude-…` or `anthropic/…` | Anthropic | `ANTHROPIC_API_KEY` |
| `openai/<model>` | any OpenAI-compatible API: OpenAI, or a local vLLM / LM Studio server | `OPENAI_API_KEY`, `OPENAI_BASE_URL` (default `https://api.openai.com/v1`) |
| `openrouter/<vendor>/<model>` | OpenRouter (Claude, Muse Spark, free models, …) | `OPENROUTER_API_KEY` |
| `<name>:<tag>` or `ollama/…` | Ollama, local or cloud; thinking off unless the name ends in `+think` | `OLLAMA_HOST` (e.g. `http://localhost:11434`) or `OLLAMA_API_KEY` |
| `chatgpt/<model>` (e.g. `chatgpt/gpt-5.5`) | a ChatGPT subscription, through the Codex CLI's login | `~/.codex/auth.json` from `codex login` (read only, never refreshed) |

A local model needs its context set above blyte's fold budget: start Ollama with
`OLLAMA_CONTEXT_LENGTH=32768` and run blyte with `--fold-budget 16000`. Provider credentials never
reach the commands the model runs.

Run `blyte --help` for every flag. REPL commands: `/help`, `/model`, `/gate`, `/state`, `/diff`,
`/workspace`, `/clear`, `/exit`.

## Workspace

```
crates/
  blyte-agent/     the loop, context strategies, tools, shell/fs seams, sessions, search
  blyte-provider/  Gemini, Anthropic and Ollama clients, pricing
  blyte-cli/       the `blyte` binary: headless runs, the REPL, run reports
```

`cargo run --release -p blyte-agent --example harness_bench` measures the harness's own time with
no model.
