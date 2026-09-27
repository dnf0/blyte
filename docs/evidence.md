# Evidence

Two kinds of evidence, with different strengths. Neither is a benchmark leaderboard entry.

- The **paired runs** measure blyte **as it is meant to be used**: Claude planned every task and
  reviewed every result, and blyte implemented on a cheap model. They compare blyte with another
  implementer, agy, in that same position.
- The **FrontierHarness run** measures blyte **on its own**: no plan, a cheap model and the raw
  task, against harnesses that plan and implement for themselves. It is a hard test of the
  implementer, not a picture of the intended use.
- **Not yet measured:** the pattern against the frontier model doing the whole task alone. The
  orchestrator's planning and review cost frontier tokens too; an experiment is planned.

## Paired shadow runs against agy

**Method.** Every change to blyte, and to the repository that evaluates it, was implemented twice
from the same commit, side by side on one machine: once by blyte and once by agy (Google's
Antigravity CLI), both on Gemini 3.8 Flash, both given the same plan and the same rules. Both
results were checked with the repository's full check command and reviewed; the better one was
merged. Every run, check and verdict is kept in a ledger. Only these paired runs count as
comparisons.

<div class="b-stats">
  <div class="b-stat"><b>0.47<small>×</small></b><span>agy's time, median</span><i>faster in 27 of 31</i></div>
  <div class="b-stat"><b>0.63<small>×</small></b><span>agy's cost, median</span><i>cheaper in 29 of 31 · list price</i></div>
  <div class="b-stat"><b>24<small>/31</small></b><span>blyte's result mergeable as-is</span><i>agy: 26 of 31</i></div>
</div>

**What it shows.** On real tasks (new providers, the sandbox, report fields, deleting thousands of
lines of dead code, CI), the same model under blyte produced work of the same quality as under agy
in about half the time, for about two thirds of the cost.

**What it does not show.**

- The tasks are in two of our own repositories (Rust and Python), and the plans were written by an
  orchestrating agent, often with the code spelled out. Open-ended tasks may behave differently.
- "Mergeable as-is" is a reviewer's judgement, and it counts an equivalent result that was not
  merged because the other was faster.
- Two runs were much slower for blyte (1.8× each). In one it wandered off to clear a host cache
  after a gate failure; in the other a deleted session temp directory broke every later command.
  Both led to fixes now in blyte: the workspace boundary and the write sandbox, and a
  self-healing session.
- One model. The comparison says nothing about blyte against agy on other models.

??? info "All 31 paired runs, task by task"

    | task | agy | blyte | blyte ÷ agy (time, cost) | result |
    |---|---|---|---|---|
    | `read` returns whole files | 382 s, $0.47 | 266 s, $0.25 | 0.70×, 0.54× | identical diffs |
    | gate reports every failure | 460 s, $0.35 | 368 s, $0.24 | 0.80×, 0.70× | identical diffs |
    | read from the top returns the whole file | 320 s, $0.30 | 349 s, $0.16 | 1.09×, 0.55× | identical diffs |
    | quick checks before stopping | 138 s, $0.16 | 89 s, $0.11 | 0.64×, 0.69× | identical diffs |
    | build stamp in `--version` and reports | 160 s, $0.21 | 89 s, $0.18 | 0.56×, 0.83× | identical diffs |
    | eval: remove the MCP arms (−2,680 lines) | 481 s, $0.98 | 288 s, $0.76 | 0.60×, 0.78× | blyte's code better, wrong commit message |
    | eval: remove the daemon and relay arms (−3,510) | 492 s, $0.71 | 231 s, $0.47 | 0.47×, 0.66× | blyte better (agy dropped valid tests) |
    | eval: remove the state-engine arms (−2,780) | 1030 s, $1.64 | 300 s, $0.84 | 0.29×, 0.51× | blyte merged; agy stopped on a plan gap |
    | run telemetry in the report | 394 s, $0.77 | 237 s, $0.42 | 0.60×, 0.55× | same logic; agy's merged (doc comments) |
    | eval: remove 34 dead launch scripts | 249 s, $0.36 | 116 s, $0.26 | 0.47×, 0.72× | identical |
    | OpenAI-compatible provider | 563 s, $0.73 | 256 s, $0.56 | 0.45×, 0.77× | equal; blyte's merged |
    | `read` keeps long lines | 292 s, $0.42 | 113 s, $0.19 | 0.39×, 0.45× | identical diffs |
    | commit footer (an experiment, later removed) | 732 s, $1.10 | 266 s, $0.52 | 0.36×, 0.47× | equal |
    | Anthropic prompt caching | 372 s, $0.51 | 130 s, $0.24 | 0.35×, 0.47× | near-identical |
    | eval: time in the harness comparison | 1661 s, $0.74 | 3024 s, $0.47 | 1.82×, 0.64× | agy's merged: blyte's run cleared a host cache after a gate failure |
    | remove the commit footer | 216 s, $0.30 | 112 s, $0.19 | 0.52×, 0.63× | blyte's clean revert merged |
    | blyte states the workspace boundary | 300 s, $0.41 | 109 s, $0.16 | 0.36×, 0.39× | identical |
    | keep failed gate output in the report | 840 s, $1.36 | 365 s, $1.00 | 0.43×, 0.74× | equal; both needed a plan fix |
    | eval: CI workflow | 561 s, $0.46 | 125 s, $0.21 | 0.22×, 0.45× | equal |
    | eval: offline agent, online grader | 572 s, $0.98 | 303 s, $0.73 | 0.53×, 0.74× | equal |
    | honest completion prompt when ungated | 255 s, $0.34 | 118 s, $0.15 | 0.46×, 0.44× | equal |
    | ChatGPT-login provider | 714 s, $1.12 | 407 s, $0.80 | 0.57×, 0.72× | equal |
    | macOS write sandbox | 767 s, $1.51 | 1414 s, $4.30 | 1.84×, 2.85× | agy's merged: blyte's session broke when its temp directory was deleted (a harness bug, fixed next) |
    | session recreates its temp directory | 253 s, $0.34 | 291 s, $0.44 | 1.15×, 1.29× | equal |
    | `--max-cost` | 514 s, $1.01 | 400 s, $0.98 | 0.78×, 0.97× | agy's merged: blyte edited an existing test |
    | sandbox refuses credential reads | 453 s, $0.72 | 186 s, $0.46 | 0.41×, 0.63× | equal; both needed a compatibility fix |
    | withhold credential-looking env vars | 549 s, $0.91 | 237 s, $0.54 | 0.43×, 0.60× | equal |
    | Linux Landlock sandbox | 632 s, $1.18 | 288 s, $0.71 | 0.46×, 0.60× | equal |
    | quota fails fast; login errors | 398 s, $0.60 | 207 s, $0.39 | 0.52×, 0.64× | equal |
    | `--network=off` | 907 s, $1.51 | 293 s, $0.79 | 0.32×, 0.52× | equal; blyte's commit message fixed |
    | security settings in the report | 476 s, $0.81 | 177 s, $0.37 | 0.37×, 0.46× | equal |

## FrontierHarness tasks

[FrontierHarness](https://frontierharness.org/) runs 12 harness configurations on Kimi K3 over 30
tasks (21 Terminal-Bench 2, 9 DeepSWE), one attempt each, graded by each task's held-out tests.
blyte ran the same tasks once, on **Gemini 3.8 Flash**, locally, offline, graded the same way.

On the 27 tasks that could be matched, blyte passed **19/27 (70%)**. The best published
configuration, Codex on Kimi K3, passed 17/27 (63%); the others passed 12 to 16. blyte solved two
tasks no configuration solved.

**Read it as "competitive with the best harnesses", not "better".** The model differs, the
infrastructure differs, each task ran once (about ±9 points of noise on 27 tasks), and three tasks
were excluded because their offline versions could not be reproduced. A same-model comparison
needs blyte on Kimi K3.

The full write-up, with per-task results and every caveat:
[blyte on the FrontierHarness tasks](research/2026-09-27-frontierharness-gemini.md).

## What was measured along the way

[Harness research](research/2026-09-24-harness-research.md) records what was tried, what the
literature says, and what was removed because it did not pay for itself: a daemon, an MCP server,
a worktree engine, advisory review, a TDD mode, LLM-written fold notes, four losing context
strategies, and more.
