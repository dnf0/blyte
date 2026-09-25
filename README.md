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
- **Honest telemetry.** `--report` writes a JSON record per run: outcome, turns, true cost, and
  per-call usage split into cache hits and misses.

## Evidence

**Paired shadow runs** (blyte and agy implement the same plan task from the same commit, run
together, both checked by the repository's full check). Blyte is developed this way: each of these
tasks was a change to blyte itself.

| task | agy | blyte | blyte ÷ agy (time, cost) |
|---|---|---|---|
| `read` returns whole files | 382 s, $0.47 | 266 s, $0.25 | 0.70×, 0.54× |
| gate reports every failure | 460 s, $0.35 | 368 s, $0.24 | 0.80×, 0.70× |
| read from the top returns the whole file | 320 s, $0.30 | 349 s, $0.16 | 1.09×, 0.55× |
| quick checks before stopping | 138 s, $0.16 | 89 s, $0.11 | 0.64×, 0.69× |

All four pairs produced byte-identical diffs and passed the check. Median: 0.75× the time, 0.60×
the cost.

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

Credentials: `GEMINI_API_KEY` (default provider), `ANTHROPIC_API_KEY`, or `OLLAMA_API_KEY`.
`BLYTE_MODEL` or `--model` picks the model; a `-low`/`-medium`/`-high` suffix sets Gemini's
thinking level (e.g. `gemini-3.8-flash-high`).

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
