# Roadmap

What is planned next for blyte, what is waiting on something, and what is parked. Dated items are
decisions; the evidence behind them is in `docs/evidence.md` and the plans in `docs/superpowers/plans/`.

## In progress

Nothing at the moment; see Next.

## Next

- **Codex as the planner (2026-09-29).** `blyte skill install --harness codex` exists but is untested
  end to end. Step 1: a smoke test with a Codex login (a paid ChatGPT plan or an OpenAI API key):
  install, delegate one small real task, confirm the skill, the sandbox rule and the run. Then drop
  "(untested)". Step 2: a Codex driver in the eval's offload arm, to compare Codex + blyte with
  Claude Code + blyte.
- **Fewer, larger dispatches as the default guidance.** Gemini makes one tool call per turn, and each
  dispatch re-reads the code it touches. On scc (2026-09-29), one dispatch was the fastest and
  cheapest; splitting one feature cost 30–160% more Gemini, even with the parts running at once.
  Split only independent tasks of similar size; the skill says so, and the eval's tools now do too.

## Done

Kept for about the last four weeks; older items are in the git history and the evidence page.

- **2026-09-30**
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
- **Docs hosting and the licence line:** parked until the decision to go public.
