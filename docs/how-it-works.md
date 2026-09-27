# How it works

blyte is three crates and one loop.

```
crates/
  blyte-agent/     the loop, context strategies, tools, shell and file-system seams, sessions,
                   search, the sandbox
  blyte-provider/  Gemini, Anthropic, OpenAI-compatible, Ollama and ChatGPT clients; pricing
  blyte-cli/       the `blyte` binary: headless runs, the REPL, run reports
```

## The loop

Each turn, blyte sends the prompt to the model, runs the tool calls it gets back, appends the
results, and goes again. Tool calls run **synchronously, under deadlines**: there are no
background tasks, hooks or polling to wait on. The loop ends when:

1. the model says it is finished and the **gate** passes (`verified`);
2. the gate has failed `--max-gates` times (`gate_failed`);
3. a budget runs out: turns, seconds or cost (`budget_exhausted`);
4. something unrecoverable happens (`error`), for example an exhausted provider quota.

Transient provider errors (a burst rate limit, a 5xx, a dropped connection) are waited out with
backoff, but never past the run's deadline.

## The gate

The gate is a command, not a tool. The model is told a check exists and that it decides when the
work is done, but it cannot call it, pass arguments to it or see it run early. When the model says
it is finished, blyte runs every step of the gate, collects every failure, and either ends the run
or hands all the failures back at once. With rollback on, the tree is reset to the run's starting
commit first, and the model is told its edits are gone.

The model is asked to run the **quick** checks itself (formatting, lint, the affected tests)
before it says it is finished, because a failed gate costs a whole test suite.

## Context: folded, append-only

Every turn re-sends the prompt, so the prompt's shape decides the bill. blyte's default context
(`folded`) keeps the prompt append-only, which keeps the provider's prompt cache valid:

```
[opening: system prompt + task] [digest 1] … [digest k] [verbatim recent turns]
```

When the verbatim part passes the fold budget (48K tokens by default), its older half becomes one
digest: a line per tool call (``- `bash` `cargo test` -> exit 101``), written by the harness, not
summarised by a model. Digests never change once written, so everything before the
newest fold stays cached.

## Tools

| tool | what it does | bounds |
|---|---|---|
| `read` | a file, or a line range | whole files up to 2,000 lines; lines to 2,000 characters |
| `write` | create or replace a file | |
| `edit` | replace an exact string in a file | the match must be unique |
| `bash` | a command in the persistent shell session | 5-minute deadline; 400 lines kept, head and tail |
| `grep` | regex search, in-process (ripgrep's libraries), respecting `.gitignore` | 200 matches |
| `glob` | find files by pattern, respecting `.gitignore` | 200 matches |

No single result can exceed 60K characters. `read` returns whole files because every extra turn
re-sends the prompt: paging through a 650-line file once cost 15 turns.

**Persistent shell.** Commands run in one long-lived `bash` session (on the host or in a
container), so `cd` and `export` carry over, as in a terminal, at about 0.7 ms of harness time per
command. If the session dies, blyte starts a new one; if its temp directory disappears, it is
recreated.

## The sandbox

On the host, the session runs under macOS Seatbelt (`sandbox-exec` with a generated profile) or
Linux Landlock (applied in the child process before `bash` starts). See
[Security](security.md) for what each allows.

## Measuring the harness itself

```bash
cargo run --release -p blyte-agent --example harness_bench
```

measures blyte's own per-command and per-turn overhead with no model involved. Across a
200-turn run the harness spends about 2 seconds of its own time; the rest is the model and the
commands it chose.

For the full walk through the code, see the [architecture deep dive](architecture.md).
