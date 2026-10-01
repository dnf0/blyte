# Roadmap

What is planned next for blyte, what is waiting on something, and what is parked. Dated items are
decisions; the evidence behind them is in `docs/evidence.md` and the plans in `docs/superpowers/plans/`.

## In progress

Nothing at the moment; see Next.

## Next

- **Watch dispatch time after v0.7.6.** The rules now tell the model not to run the full suite
  itself (8 of 9 runs did under v0.7.5, at 2-3 minutes each). The first runs on v0.7.6 spent 84 s
  and 50 s on tools, against 198 s before; check the next few, including work repositories.
- **A shared Cargo build directory for worktrees**, measured first: each `.wt/<id>` compiles the
  workspace from scratch. Needs a sandbox allowance for that directory, and parallel dispatches
  would queue on cargo's lock.
- **Codex as the planner, step 2 (2026-09-30).** Step 1, the end-to-end smoke test, passed (#164).
  Next: a Codex driver in the eval's offload arm, to compare Codex + blyte with Claude Code + blyte
  on the same task. Uses the ChatGPT plan's Codex allowance; ask before running. The first katex
  comparison (2026-09-30) was stopped: both Codex runs failed to hand off to blyte (one had no key,
  one sat silent for 17 minutes); find out why before running it again.
- **Commit `blyte.toml` in team repositories** once a second person uses blyte in one, so nobody has
  to run `blyte init` (it is in the global git ignore on the user's machine for now).

## Done

Kept for about the last four weeks; older items are in the git history and the evidence page.

- **2026-10-01**
  - The dashboard's **Plans view**: each plan as a swimlane flow of its tasks (lanes by area of code,
    columns by dependency), coloured by state (merged, verified, running, failed, redone, ready,
    waiting), with the selected task's details, the plan's activity and decisions, all from files
    blyte already writes (`/api/plans`). `blyte dispatch --plan/--plan-task` links a dispatch to its
    plan task; older dispatches are matched by tag, task text and their `Plan:` line.
  - Research runs read the repository with the `read` and `grep` tools, default to 300 turns, and
    keep their report current; every run is warned ten turns before its turn budget ends.
  - `blyte dispatch --research`: read-only research as a dispatch (a scratch workspace under `.wt/`,
    gate "report.md written", the report copied beside the run), so it gets the registry, status,
    dashboard and review like any dispatch; the skill says when to use it. Run reports record the
    model's effort level (`effort`), which `model_id` hides.
  - The dashboard names a dispatch after its repository, not its worktree's folder (v0.7.8).
  - Dispatch rules tell the model not to run the full suite itself (v0.7.6): tools time had risen
    from a median of 44 s to 198 s after the switch to plain `blyte dispatch`.
  - The running dashboard sees repositories registered after it started; `blyte init` recognises
    `python -m pytest`/`unittest`, `tox` and `nox` (v0.7.5), and keeps one of two pytest steps that
    test the same files.
  - The sandbox lets a worktree write the tool caches of an enclosing `node_modules` (vitest's
    `.vite-temp`), never its packages.
  - Linked worktrees use the main checkout's `blyte.toml`; `init` falls back to the repository's
    own checks when CI is stale, and calls a cold uv cache an environment problem (v0.7.4).
  - The repository registry writes one whole line per dispatch and skips temp directories (v0.7.3).
  - The default Gemini model runs at high effort, as the docs said (v0.7.2); a run never outlives
    `--max-seconds` by more than 30 s and always leaves a report (v0.7.1).
  - The operator's local dispatch scripts and their sampled A/B against agy are retired (74 paired
    tasks: 0.55x agy's time, 0.64x its cost). Every repository on the operator's machine now
    dispatches with plain `blyte dispatch`.
- **2026-09-30**
  - blyte owns the whole dispatch workflow, so the operator's local wrapper scripts can retire:
    `blyte review --verdict` records the reviewer's verdict; `review` flags commands that reached
    outside the worktree; `blyte.toml` takes `[dispatch.env]` and `fold_budget`; dispatch saves its
    summary and names the gate's quick steps; `blyte init` takes the gate from CI, sets what the
    environment needs and proves each step in a fresh sandboxed worktree; the dashboard watches
    every repository blyte has dispatched in; the skill has tasks declare their dependencies.
  - `blyte dashboard`: a local page (127.0.0.1) of running and recent jobs across repositories and
    run directories, with time, cost, turns, the task's section of its plan, and clickable tags
    (repo, plan, harness, A/B) that group related jobs. `blyte dispatch --tag` adds tags.
  - A release on every merge (commitizen, semantic versioning, #157/#159/#162): a release PR, a
    `vX.Y.Z` tag, the GitHub release with its changelog, and static binaries for macOS and Linux.
  - `blyte update` installs the latest release in place, with `--check` and `--rollback` (#169).
  - blyte loads the repository's rules for agents itself on every run (#160): `.blyte/rules.md`,
    else `.claude/agy-rules.md`, else `AGENTS.md`, however it is started.
  - Codex CLI run end to end (#164): the skill names the escalation Codex must request; the
    "(untested)" labels are gone.
  - Apache-2.0 only (#166), and a dependency licence check in CI (`cargo deny`, #168): every
    dependency is permissive.
  - Fewer, larger dispatches as the default guidance (docs, skill and the eval's tools).
  - `blyte skill install --no-default` removes an earlier default block, so switching from on to
    "only when asked" needs no uninstall first.
  - Every blyte run is recorded for reporting, including ones started with `blyte dispatch`
    directly, with a check of whether each one's work reached main (local tooling).
- **2026-09-29**
  - More evidence on katex: v2 repeated (Claude usage down 86%), and a parallel test that showed
    splitting one feature does not pay (evidence page, #154; blyte-eval #88).
  - Provider keys from `~/.config/blyte/env` when they are not exported (#151): a session started
    without the key failed every dispatch at turn 0.
  - `AWS_PROFILE` and `AWS_DEFAULT_PROFILE` withheld with the credentials (#150): blyte had edited a
    repository's `conftest.py` to work around a profile the sandbox cannot read.
  - Research mode (local dispatch scripts): `dispatch-auto.sh --research`, read-only, with a "report
    exists" gate, the sampled A/B and ledger records.
  - The eval's dispatch tool runs dispatches in parallel, each in its own copy of the repository
    (blyte-eval #83), with start-then-wait tools (#85), because Claude Code runs an MCP server's
    calls one after another. Results on scc: blyte-eval #86.

## Information gathering (2026-09-29 to about 2026-10-27)

- blyte is the implementer in every local repository, with a sampled A/B against agy
  (`dispatch-auto.sh`). Check usage across repositories and sessions around 2026-10-06.
- The decider shadow trial (a local model classifying failed gates) runs alongside, testing only.
  Decide whether it earns a place around 2026-10-20 to 10-27.

## Parked

- **Open-sourcing blyte** and the brief for Jack: on hold until the information gathering ends.
- **Docs hosting:** parked until the decision to go public.
- **When open source is approved** (these need a public repository and a crates.io release):
  - `blyte update` updates the way blyte was installed: `cargo install blyte --locked` (or
    `cargo binstall blyte`) for a cargo install, detected from `~/.cargo/.crates2.json`, and the
    release binary over plain HTTPS otherwise, with no GitHub CLI needed. Today it always downloads
    the release binary through `gh`.
  - Publish the crates to crates.io (`cd.yml`, manual, dry run first).
  - Contributions: a DCO sign-off or a CLA, decided with Jack and legal, before accepting outside
    pull requests.
