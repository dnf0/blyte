# blyte

The `blyte` command: a lean coding-agent harness in Rust, built to be the implementer under a
frontier model. Plan with Claude Code (or another frontier harness), hand each well-specified task
to blyte on a cheap model, and let the repository's own check, run as a sealed gate, decide when
it is done.

```bash
cargo install blyte --locked
blyte skill install          # the Claude Code skill for the plan, delegate, review workflow
```

| command | what it does |
|---|---|
| `blyte "<task>" --gate "<check>"` | run one task in the current repository until the check passes or a budget runs out |
| `blyte dispatch <id> <task-file> --gate "<check>"` | run one task in its own worktree (`.wt/<id>`) and print a JSON summary for the orchestrator |
| `blyte review <id> [--diff]` | show what a dispatched run did |
| `blyte skill install [--project]` / `blyte skill print` | install or print the Claude Code skill |
| `blyte` | the REPL |

Full documentation: the [repository README](https://github.com/dnf0/blyte#readme)
and the docs (`docs/` in the repository).

This crate is the binary; the loop, tools and sandbox are in `blyte-agent`, the model providers
in `blyte-provider`.
