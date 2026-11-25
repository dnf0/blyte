# Rename "orchestrator" to "the conversation model"

Date: 2026-11-24. Decided by the user: the model you talk to in the surface is called
**the conversation model**, and the old word goes everywhere: text, process files, the flag, the
JSON fields and the code names. Facts below were checked against `main` at df47b77d (research run RS6).

## Decisions

| Old | New |
|---|---|
| "the orchestrator" in any text shown or documented | "the conversation model" |
| flag `--orchestrate` | `--converse` (the old flag is not kept as an alias: clap rejects it as unknown) |
| `~/.config/blyte/orchestrator.md`, `$XDG_CONFIG_HOME/blyte/orchestrator.md`, `<repo>/.blyte/orchestrator.md` | `process.md` in the same three places; an old file that still exists is an error |
| `State::orchestrator: Orchestrator` (goal, check command, cost cap, instructions) | `State::brief: Brief` (it is the run's brief, not the conversation model) |
| JSON `orchestrator_usd` (`blyte cost --json`) | `conversation_usd` |
| JSON `orchestrator_message` (`POST /api/runs/{id}/decision`) | `conversation_message` |
| other identifiers `orchestrator_*`, `*_orchestrator`, `ORCHESTRATOR_*` | `conversation_*`, `*_conversation`, `CONVERSATION_*` |
| identifiers `orchestrate`, `orchestrate_*`, `*_orchestrate` | `converse`, `converse_*`, `*_converse` |
| `crates/blyte-agent/src/orchestrator.rs` | `conversation.rs` |
| `crates/blyte-cli/src/orchestrator_text.rs` | `conversation_text.rs` |
| `crates/blyte-cli/assets/orchestrator/` | `crates/blyte-cli/assets/conversation/` |
| `docs/orchestration.md` | `docs/workflow.md` (its title is already "The workflow") |
| `test_orchestrator_prompt.rs`, `test_orchestrator_instructions.rs`, `test_orchestrator_reference.rs`, `test_orchestrator_sandbox.rs` | `test_conversation_*.rs` |
| `crates/blyte-cli/tests/test_orchestrate.rs` | `test_converse.rs` |

Nothing blyte writes to disk contains the word (session entries, `.wt/*.json`, the usage ledger,
`.wt/surface.json`), so no saved file stops loading. No serde aliases and no compatibility code: no fallbacks.

### Task CM

**Goal.** Apply the table above across the repository in one commit, so the word "orchestrator" (and
"orchestrate", "orchestration") is gone from everything except the dated records listed under "Keep".

**Depends on:** none. **Siblings:** none.

**How (this touches about 80 files: do it with a script, not file by file).**

1. Move the files with `git mv` (the moves in the table; `assets/orchestrator` moves as a directory).
2. Save the script below as `rename.pl` inside the worktree (deleted before the
   commit) and run it over `git ls-files`, excluding `docs/plans`, `docs/specs`, `docs/superpowers`,
   `CHANGELOG.md`, `MANIFESTO.md`, `CLAUDE.md`, `docs/evidence.md`, `docs/adr/`, and binary files:

   ```perl
   #!/usr/bin/perl -pi
   # Lines that are dated records or guards: leave untouched.
   next if /reliability|orchestrate-mode|examples\/orchestrate|previously asserted|2026-09-27|measured basis/;
   s/--orchestrate\b/--converse/g;
   s/orchestrator\.md/process.md/g;
   s/assets\/orchestrator/assets\/conversation/g;
   s/orchestration(\.md|\/)/workflow$1/g;
   s/\bOrchestrator\b/Brief/g;                       # the State type and its label; check all ~10 by hand
   s/\.orchestrator\b/.brief/g;                      # state.orchestrator
   s/\borchestrator: /brief: /g;                     # the field definition and initialiser
   s/\["orchestrator"\]/["brief"]/g;
   s/ORCHESTRATOR_/CONVERSATION_/g;
   s/orchestrator_/conversation_/g;
   s/_orchestrator\b/_conversation/g;
   s/\borchestrator::/conversation::/g;
   s/\bmod orchestrator\b/mod conversation/g;
   s/orchestrate_/converse_/g;
   s/_orchestrate\b/_converse/g;
   s/\.orchestrate\b/.converse/g;
   s/\borchestrate: /converse: /g;
   s/"orchestrate"/"converse"/g;
   s/\ban orchestrator\b/a conversation model/g;
   s/\bAn orchestrator\b/A conversation model/g;
   s/\borchestrator\b/conversation model/g;          # what is left is prose
   ```

3. Then fix by hand what the script cannot: run
   `git grep -inE 'orchestrat|conversation model_|Brief' ` and read every hit. Expect: prose uses of
   "orchestrate", "orchestrating", "orchestrates", "orchestration" (reword in plain words, for example
   "plan, hand work out and review"; `blyte --converse` for the command); a prose "Orchestrator" at the
   start of a sentence that the script turned into "Brief" (should be "The conversation model");
   test function names; `scripts/` and `mkdocs.yml` references; identifiers where "conversation"
   reads wrongly next to an existing name (keep the mapping anyway unless it collides with an existing
   identifier; if it collides, stop and report it in the final message).
4. `cargo fmt --all`.

**What to build beyond the rename.**

- `crates/blyte-agent/src/conversation.rs`: `user_rules_path_from` and `repository_rules_path` build
  `process.md` paths. Add a check, run in `resolve_sources` before either file is loaded, for each of
  the two directories: if `<dir>/orchestrator.md` exists, return
  `Err("<old path> is no longer read: rename it to <new path>")`, with both full paths. This holds
  even when `process.md` also exists there. It must never be read as a stand-in, skipped silently, or
  turned into a warning. A directory with neither file behaves exactly as today (the built-in process).
- `--orchestrate` gets no alias and no hidden flag.
- The surface page's `validateDecisionResponse` requires `conversation_message` and the server sends
  it; they change together.
- `/state` prints `Brief:` where it printed `Orchestrator:`; the `/help` line reads `(Brief, Runtime)`.

**Keep (do not change these; they are dated records or guards).**

- `crates/blyte-cli/tests/test_public_names.rs` line with `concat!("reliability", "-orchestrator")`.
- `crates/blyte-cli/src/plans.rs` `include_str!("../../../docs/superpowers/plans/2026-10-07-orchestrate-mode.md")`.
- `docs/evidence.md`, `docs/adr/0002-*.md`, `docs/adr/0003-*.md`, `MANIFESTO.md`, `CLAUDE.md` (its heading is about the outside harness), everything under
  `docs/plans`, `docs/specs`, `docs/superpowers`, and `CHANGELOG.md`.
- The dated measurement note at the top of `assets/conversation/process.md` (its two lines naming the 2026-11-21 measurement).
- The dated comments at `crates/blyte-agent/src/state.rs` ("previously asserted"), and the second
  line of `crates/blyte-cli/tests/test_blyte_toml.rs`, `test_dispatch.rs` and `test_status.rs`.

**Tests.** Existing tests keep their meaning, with names and strings following the table. Add:

1. In the agent crate's test file that covers `resolve_sources` today: a repository directory holding
   `.blyte/orchestrator.md` makes `resolve_sources` return an error whose text contains both
   `.blyte/orchestrator.md` and `.blyte/process.md`; the same for the user directory; the same when
   `process.md` also exists; and `.blyte/process.md` alone is loaded as the repository's rules.
2. In `crates/blyte-cli/tests/test_converse.rs`: `blyte --orchestrate` exits non-zero and stderr names
   `--orchestrate` as unexpected; `blyte --converse --gate x` is still refused (the existing conflict test, renamed).
3. The existing page test for the decision response asserts the key `conversation_message`.

**Check that proves the rename is complete** (run it; paste its output in the final message). The
only files allowed to print are the Keep files, `crates/blyte-agent/src/conversation.rs`, the agent
test file from test 1, and `crates/blyte-cli/tests/test_converse.rs`:

```bash
git grep -il orchestrat -- . ':!docs/plans' ':!docs/specs' ':!docs/superpowers' ':!CHANGELOG.md'
git ls-files | grep -i orchestrat | grep -vE '^docs/(plans|specs|superpowers)/'   # must print nothing
```

Also run `mkdocs build --strict` if `mkdocs` is installed (the nav entry and every link to the moved
page must agree); if it is not installed, say so in the final message instead of skipping silently.

**Constraints.** No fallbacks: no aliases, no reading the old file name, no accepting the old JSON key.
No behaviour change other than the names and the old-file error. Not a long job; nothing to resume.
Do not touch files outside this repository.

**Commit message (exact):**

```
feat: rename the orchestrator to the conversation model

The flag is --converse, the process file is process.md (an existing
orchestrator.md is an error naming the new path), and the JSON fields are
conversation_usd and conversation_message.
```
