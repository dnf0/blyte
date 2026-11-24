# Open items: internal commands out of the help, and the items that need facts first

Date: 2026-11-24. One pull request for all tasks. Tasks for the surface starting without a
terminal, the Review count, and sending a review's concerns back to a run are added to this plan
once research runs RS4 and RS5 have checked the facts.

## Rules for every task

- No fallbacks: when something is missing or does not match, fail with an error that names it.
- Existing tests pass unmodified unless the task says otherwise.
- Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings`, then the tests the
  task names.
- No new dependencies.

### Task HELP

**Goal.** `blyte --help` lists the commands a person or an orchestrating model uses. The five
commands that only blyte itself calls still work but are not listed.

**Depends on:** none.

**Siblings.** Later tasks in this plan change `serve.rs`, `repl.rs`, `main.rs`, `dispatch.rs` and
`queue_actions.rs`. Do not touch those files.

**Files.** `crates/blyte-cli/src/cli.rs`, new `crates/blyte-cli/tests/test_help_commands.rs`,
`docs/reference.md` only if it has a list of commands that needs a note.

**What to build.** In the subcommand enum in `cli.rs`, add `#[command(hide = true)]` to exactly
these variants: `Comments`, `Instructions`, `Mock`, `EditStart`, `EditFinish`. Nothing else
changes: the commands parse and run as before, and `blyte <command> --help` still prints their
own help. Do not hide, rename or remove any other command. If the top-level help text has an
`after_help` or a hand-written command list that names any of the five, remove them from that
list too.

**Tests** (`test_help_commands.rs`, running the built binary as the other CLI tests do, for
example `test_cli_smoke.rs`).

- `the_help_does_not_list_internal_commands`: in the output of `blyte --help`, no line of the
  commands list starts (after leading spaces) with `comments`, `instructions`, `mock`,
  `edit-start` or `edit-finish`.
- `the_help_still_lists_the_commands_people_use`: the commands list has lines starting with
  `dispatch`, `review`, `wait`, `status`, `plan`, `cost`, `clean`, `init` and `update`. First check
  each of these exists in the enum; if one does not, leave it out of the test and say so in your
  report, do not add a command.
- `hidden_commands_still_run`: `blyte edit-start --help`, `blyte edit-finish --help`,
  `blyte comments --help`, `blyte instructions --help` and `blyte mock --help` each exit 0 and
  print a usage line.

Run: `cargo test -p blyte --test test_help_commands --test test_cli_smoke`.

**Commit message.** `feat(cli): leave blyte's internal commands out of the help`
