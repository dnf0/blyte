# Getting started

## Install

blyte is a single Rust binary. With a recent stable Rust toolchain:

```bash
cargo install --git https://github.com/climate-x-org/superstate blyte-cli --locked
blyte --version        # blyte 0.1.0 (<commit>)
```

Or from a checkout:

```bash
git clone https://github.com/climate-x-org/superstate.git blyte
cd blyte
cargo install --path crates/blyte-cli --locked
```

blyte runs on macOS and Linux. The command sandbox uses Seatbelt on macOS and Landlock on Linux
(kernel 5.13 or later; 6.7 or later for `--network=off`); see [Security](security.md).

## Pick a model

The model name picks the provider. The default is Gemini:

```bash
export GEMINI_API_KEY=...                  # default model: gemini-3.8-flash-high
blyte --model claude-sonnet-5 ...          # needs ANTHROPIC_API_KEY
blyte --model qwen3:14b ...                # a local Ollama model
```

Every provider and its credentials are on [Providers](providers.md). Credentials are read by blyte
itself and never reach the commands the model runs.

## Your first run

Run blyte from the root of a git repository. Give it the task, and the command that proves it is
done:

```bash
blyte "handle empty input in parse() and add a test" --gate "cargo test"
```

blyte works until the gate exits 0, or a budget runs out, and exits with the result:

| exit | meaning |
|---|---|
| `0` | verified: the gate passed (ungated: the model said it was finished) |
| `1` | the gate failed, a budget ran out, or an error stopped the run |

Without `--gate` the model decides when it is finished and the run is **ungated**. That is useful
for exploration, but a gated run is the one to trust.

!!! tip "Give the gate every check you care about"
    A gate of several lines is several steps. Every step runs and every failure comes back in one
    message, so a formatting error cannot hide a test failure:

    ```bash
    blyte "..." --gate 'cargo fmt --all --check
    cargo clippy --all-targets -- -D warnings
    cargo test'
    ```

## Interactive use

`blyte` with no prompt starts a REPL. Type tasks; the commands are `/help`, `/model`, `/gate`,
`/state`, `/diff`, `/workspace`, `/clear` and `/exit`.

## Next

- [Running tasks](usage.md): budgets, containers, rollback, reports.
- [How it works](how-it-works.md): what happens inside a run.
