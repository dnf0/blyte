# Roadmap

What is planned next for blyte, what is waiting on something, and what is parked. Dated items are
decisions; the evidence behind them is in `docs/evidence.md` and the design plans.

## In progress

Nothing at the moment; see Next.

## Next

- **The Claude usage check (about 2026-10-06):** regenerate the usage report from the dispatch
  ledger and compare Claude usage with and without blyte across repositories and sessions.
- **The decider shadow trial (about 2026-10-20):** read its summary and keep it or remove it.
- **The open-source decision (about 2026-10-27):** if yes, the items under Parked come next.

## Done

Kept for about the last four weeks; older items are in the git history and the evidence page.

- **2026-10-02**
  - Decided: `blyte.toml` stays per user and untracked; each person runs `blyte init` in their
    checkout. Gates differ by machine (offline uv caches, a pinned uv, local pre-commit
    environments), and the gate-drift warning tells each person when theirs falls behind CI.
  - Dispatch time checked after the v0.7.6 rule change: tools time stayed low, and the blyte-only
    canary on v0.13.0 matched 27 September on tokens and cost (51.2M / $8.51 against 51.9M / $8.81),
    with the pass rate the same or better; the extra wall time was Gemini's latency.
  - A run's work counts as merged once it is on the branch it was dispatched from (`--base`, now
    recorded in the start file), not only the default branch, so runs on a long-lived feature branch
    are cleaned up when merged into it. `blyte clean` reports other worktrees (sessions' sibling
    `repo--x` worktrees) whose work is merged and that are clean, with the `git worktree remove`
    command; it never removes them.
  - Plans view: a plan file dispatched as the task is the run's plan, wherever it lives (sibling
    worktrees included), read from git history once that worktree is gone, and matched by the run id
    when there is no `--plan-task` (v0.14.0 and after).
  - v0.16.0: `blyte dispatch` warns when the gate is missing checks the repository's CI runs,
    matched by tool; `blyte init` also recognises `make typecheck`/`validate`, eslint, vitest, jest,
    tsc; `skill install`/`update` warn about a superseded `blyte-research` skill; skill guidance on
    real-data checks, shared registration files and plan files as task files.
  - v0.17.0: follow-up chains and squash-merged runs count as merged (cherry-pick record), and the
    dispatch-time clean is cheap; the first `blyte clean --all` on the author's machine freed 21 GB.
- **2026-10-01**
  - `blyte clean` (also run quietly by every `blyte dispatch`): a run's worktree goes once its
    work is on the default branch (local or `origin/`, including squash merges of several runs),
    it was redone or made no changes, its research report is saved, or 14 days have passed; never
    with uncommitted changes, and the run's record files stay. Branches go only when merged. 29
    worktrees held 15 GB on the author's machine, 21 of them for reviewed runs.
  - Honest outcomes: a gate that fails the same way before the change ends the run as `blocked`
    (exit 5) instead of burning the budget; the deadline watchdog says what was running when it
    stopped a run (v0.11.0). Gate slots default to a quarter of the cores (at least 2), time
    waiting for one no longer counts against the run's deadline, and research skips them; a long
    failed gate keeps the test runner's summary lines (v0.11.1).
  - Needs attention lists only runs nobody has dealt with (no verdict, not superseded by a later
    run, worktree still there, under three days old); the Plans view lists plans with a running
    task, keeps each node's state colour when selected, shows a task's merged run before its
    latest, and draws follow-up tasks dispatched with `--plan` but not written in the plan as
    "added during the work" (v0.11.2). The two watchdog tests take a second instead of 32 s.
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

- blyte is the implementer in every local repository (the A/B against agy ended 2026-09-30).
  Check usage across repositories and sessions around 2026-10-06.
- The decider shadow trial (a local model classifying failed gates) runs alongside, testing only.
  Decide whether it earns a place around 2026-10-20 to 10-27.

## Parked

- **A shared Cargo build directory for worktrees.** Measured 2026-10-01: a warm build is 15 s of a
  gate of about two minutes, which the test run dominates (its two slowest tests were then cut from
  32 s to 1 s each). Worth it only if builds come to dominate; it would need a sandbox allowance,
  and parallel dispatches would queue on cargo's lock.
- **Codex as the planner, step 2.** The blocker (Codex's 5-minute MCP tool timeout) is fixed in
  blyte-eval #93; rerunning the Codex + blyte against Claude Code + blyte comparison is parked
  (2026-10-02): the failures were likely down to the Codex subscription level.

- **Open-sourcing blyte** and a brief for the decision: on hold until the information gathering ends.
- **Docs hosting:** GitHub Pages, deployed by a workflow on merge to `main` (MkDocs Material builds
  already run in CI), at `<user>.github.io/blyte`. A custom domain (about $10-50 a year; no reliable
  free one) keeps links stable if the repository moves; a free `*.github.io` address is fine to start.
- **Scrub before publishing (2026-10-02 count):** 54 of 378 tracked files mention internal names
  (work repositories, the AWS account, people, internal artifact links), 33 of them plans under
  `docs/superpowers/`; also `README.md`, `SECURITY.md`, `LICENSE`, `CONTRIBUTING.md`, `Cargo.toml`,
  `mkdocs.yml`, `docs/evidence.md`, two ADRs, two source files and six tests. Decide per file: publish,
  rewrite with generic examples, or keep out. The git history carries the same names: publish a fresh
  repository (one squashed initial commit) rather than this one, after a secrets scan.
- **When open source is approved** (these need a public repository and a crates.io release):
  - `blyte update` updates the way blyte was installed: `cargo install blyte --locked` (or
    `cargo binstall blyte`) for a cargo install, detected from `~/.cargo/.crates2.json`, and the
    release binary over plain HTTPS otherwise, with no GitHub CLI needed. Today it always downloads
    the release binary through `gh`.
  - Publish the crates to crates.io (`cd.yml`, manual, dry run first).
  - Contributions: a DCO sign-off or a CLA, decided before accepting outside
    pull requests.
