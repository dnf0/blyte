# Roadmap

What is planned next for blyte, what is waiting on something, and what is parked. Dated items are
decisions; the evidence behind them is in `docs/evidence.md` and the plans in `docs/superpowers/plans/`.

## In progress

- **The AWS profile variables are withheld** with the other credentials. The sandbox denies
  `~/.aws`, so a passed `AWS_PROFILE` made boto fail, and blyte edited `conftest.py` to work around it
  (premise-lookup, 2026-09-29). Plan: `2026-09-29-withhold-aws-profile.md`.
- **Parallel dispatches in the eval** (blyte-eval): each hand-off gets its own copy of the task
  repository, runs at the same time as the others, and is merged back when it ends. This makes a
  parallel blyte-vs-Claude-alone comparison possible (chosen 2026-09-29, option B).
- **Research mode for blyte.** Read-only research (surveys, lookups) still goes to agy
  (`dispatch.sh --research`), so it is outside the sampled A/B and the ledger. blyte needs the same
  entry point, read-only rules, a "report exists" gate and ledger records.

## Next

- **Codex as the planner (2026-09-29).** `blyte skill install --harness codex` exists but is untested
  end to end. Step 1: a smoke test with a Codex login (a paid ChatGPT plan or an OpenAI API key):
  install, delegate one small real task, confirm the skill, the sandbox rule and the run. Then drop
  "(untested)". Step 2: a Codex driver in the eval's offload arm, to compare Codex + blyte with
  Claude Code + blyte.
- **Fewer, larger dispatches.** Gemini makes one tool call per turn whatever the prompt says, and
  each dispatch pays its setup again, so fewer, larger dispatches may cut cost. A small test on a
  real task.

## Information gathering (2026-09-29 to about 2026-10-27)

- blyte is the implementer in every local repository, with a sampled A/B against agy
  (`dispatch-auto.sh`). Check usage across repositories and sessions around 2026-10-06.
- The decider shadow trial (a local model classifying failed gates) runs alongside, testing only.
  Decide whether it earns a place around 2026-10-20 to 10-27.

## Parked

- **Open-sourcing blyte** and the brief for Jack: on hold until the information gathering ends.
- **Docs hosting and the licence line:** parked until the decision to go public.
