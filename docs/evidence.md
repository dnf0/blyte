# Evidence

Two kinds of evidence, with different strengths. Neither is a benchmark leaderboard entry.

!!! warning "Read these before the numbers"
    - **One model, mostly our own work.** Every paired run used Gemini 3.8 Flash. 60 of the 66
      tasks are in blyte's own two repositories (Rust and Python); 6 are from work repositories
      that now use blyte. Plans were written by an orchestrating agent, often with the code
      spelled out.
    - **FrontierHarness is one run on a different model.** blyte ran on Gemini 3.8 Flash; the
      leaderboard runs on Kimi K3. Local infrastructure, one attempt per task, 27 of 30 tasks
      matched: about ±9 points of noise.
    - **"Mergeable" is a reviewer's judgement.**
    - **Against a frontier model working alone, two tasks so far:** [see below](#against-claude-doing-the-work-itself).

- The **paired runs** measure blyte **as it is meant to be used**: Claude planned every task and
  reviewed every result, and blyte implemented on a cheap model. They compare blyte with another
  implementer, agy, in that same position.
- The **FrontierHarness run** measures blyte **on its own**: no plan, a cheap model and the raw
  task, against harnesses that plan and implement for themselves. It is a hard test of the
  implementer, not a picture of the intended use.

## Paired shadow runs against agy

**Method.** Every change to blyte, and to the repository that evaluates it, was implemented twice
from the same commit, side by side on one machine: once by blyte and once by agy (Google's
Antigravity CLI), both on Gemini 3.8 Flash, both given the same plan and the same rules. Both
results were checked with the repository's full check command and reviewed; the better one was
merged. Every run, check and verdict is kept in a ledger. Only these paired runs count as
comparisons.

<div class="b-stats">
  <div class="b-stat"><b>0.53<small>×</small></b><span>agy's time, median</span><i>faster in 54 of 63</i></div>
  <div class="b-stat"><b>0.64<small>×</small></b><span>agy's cost, median</span><i>cheaper in 58 of 65 · list price</i></div>
  <div class="b-stat"><b>49<small>/66</small></b><span>blyte's result mergeable as-is</span><i>agy: 51 of 66 · both passed the check in 62 of 66</i></div>
</div>
  <div class="b-stat"><b>0.63<small>×</small></b><span>agy's cost, median</span><i>cheaper in 35 of 37 · list price</i></div>
  <div class="b-stat"><b>30<small>/37</small></b><span>blyte's result mergeable as-is</span><i>agy: 32 of 37 · both passed the check in 36 of 37</i></div>
</div>

<div class="b-chart" markdown="0"><h4>Time: blyte ÷ agy, per task</h4><div class="b-sub">63 paired tasks · median 0.53× · below the line in 54 of 63</div><div class="b-cols"><div class="ref" style="bottom: 50.0%"><span>1.0 = same as agy</span></div><div class="col" style="height: 6.7%" title="the Vertex backend for the Gemini provider: 0.13×"></div><div class="col" style="height: 11.1%" title="The CI workflow: 0.22×"></div><div class="col" style="height: 14.6%" title="Remove the state-engine arms: 0.29×"></div><div class="col" style="height: 15.6%" title="`activate` and `status` seed modes: 0.31×"></div><div class="col" style="height: 16.2%" title="Offline profiles and the --network flag: 0.32×"></div><div class="col" style="height: 17.5%" title="Mark cache breakpoints and count cache writes: 0.35×"></div><div class="col" style="height: 18.2%" title="The boundary in the system prompt and the failed-gate message: 0.36×"></div><div class="col" style="height: 18.2%" title="Append the repository state after a successful commit: 0.36×"></div><div class="col" style="height: 18.6%" title="Five new report fields: 0.37×"></div><div class="col" style="height: 18.8%" title="rename `superstate` to `blyte` in the eval code: 0.38×"></div><div class="col" style="height: 19.3%" title="A longer line cap for `read`: 0.39×"></div><div class="col" style="height: 20.5%" title="Deny credential reads, --allow-read grants: 0.41×"></div><div class="col" style="height: 20.8%" title="Failing steps first on a retry: 0.42×"></div><div class="col" style="height: 21.6%" title="Withhold credential-looking variables, --pass-env keeps one: 0.43×"></div><div class="col" style="height: 21.7%" title="Keep failed gate output in the report: 0.43×"></div><div class="col" style="height: 22.0%" title="`.wt/<id>.start.json` and `blyte status`: 0.44×"></div><div class="col" style="height: 22.7%" title="The OpenAI-compatible adapter: 0.45×"></div><div class="col" style="height: 22.8%" title="Profile enum and a Landlock launcher: 0.46×"></div><div class="col" style="height: 23.1%" title="An honest completion section for ungated runs: 0.46×"></div><div class="col" style="height: 23.3%" title="`blyte.toml`, `blyte init`, and dispatch defaults: 0.47×"></div><div class="col" style="height: 23.3%" title="Remove the dead launchers: 0.47×"></div><div class="col" style="height: 23.5%" title="Remove the daemon and relay arms and the MCP client: 0.47×"></div><div class="col" style="height: 24.4%" title="`--harness claude|codex|all`: 0.49×"></div><div class="col" style="height: 24.8%" title="rename `superstate` to `blyte` in the eval code: 0.50×"></div><div class="col" style="height: 24.9%" title="`blyte dispatch` and `blyte review`: 0.50×"></div><div class="col" style="height: 25.3%" title="Modes and the per-run report: 0.51×"></div><div class="col" style="height: 25.5%" title="plan-first mode: 0.51×"></div><div class="col" style="height: 25.7%" title="show the model how to batch: 0.51×"></div><div class="col" style="height: 25.9%" title="Remove the footer: 0.52×"></div><div class="col" style="height: 26.0%" title="quota_exhausted and ProviderError::Auth: 0.52×"></div><div class="col" style="height: 26.2%" title="Slots off by default in the library; nested runs skip them: 0.52×"></div><div class="col" style="height: 26.5%" title="Internal network for the agent, bridge for grading: 0.53×"></div><div class="col" style="height: 27.0%" title="Restore the inherited PATH after the login profile: 0.54×"></div><div class="col" style="height: 27.3%" title="Match-centred excerpts, and a clip note that points to them: 0.55×"></div><div class="col" style="height: 27.7%" title="Machine-wide gate slots: 0.55×"></div><div class="col" style="height: 27.8%" title="Stamp the build: 0.56×"></div><div class="col" style="height: 28.1%" title="frontier and implementer cost in the comparison: 0.56×"></div><div class="col" style="height: 28.5%" title="The ChatGPT-login adapter: 0.57×"></div><div class="col" style="height: 29.2%" title="`--default`, `blyte skill uninstall`, and the new skill text: 0.58×"></div><div class="col" style="height: 29.9%" title="Remove the MCP arms: 0.60×"></div><div class="col" style="height: 30.1%" title="Time the run: 0.60×"></div><div class="col" style="height: 30.9%" title="`Profile::LandlockConfined` and the read allowlist: 0.62×"></div><div class="col" style="height: 30.9%" title="Default on, `--no-default`, parallel-first wording: 0.62×"></div><div class="col" style="height: 31.3%" title="no subagents or wake-ups in the claude arms: 0.63×"></div><div class="col" style="height: 32.0%" title="128 workers, more spot types, bigger head, pre-warm one node per partition: 0.64×"></div><div class="col" style="height: 32.2%" title="Quick checks in the system prompt: 0.64×"></div><div class="col" style="height: 32.7%" title="`blyte skill install`: 0.65×"></div><div class="col" style="height: 34.8%" title="A larger line budget for `read`: 0.70×"></div><div class="col" style="height: 35.8%" title="One sentence in the ungated completion text: 0.72×"></div><div class="col" style="height: 37.3%" title="head and tail: 0.75×"></div><div class="col" style="height: 38.0%" title="Per-build cost report (`build_cost.json`): 0.76×"></div><div class="col" style="height: 38.9%" title="The cap: 0.78×"></div><div class="col" style="height: 40.0%" title="Multi-step gate: 0.80×"></div><div class="col" style="height: 41.0%" title="mark stripped guidance skip-worktree: 0.82×"></div><div class="col over" style="height: 54.5%" title="Whole file for a read from the top: 1.09×"></div><div class="col over" style="height: 57.5%" title="Recreate the temp directory before each command: 1.15×"></div><div class="col over" style="height: 71.8%" title="Re-base the straggler clock on the current attempt: 1.44×"></div><div class="col over" style="height: 79.9%" title="`grep -E` in the fallback: 1.60×"></div><div class="col over" style="height: 81.7%" title="`read` takes several paths (added 2026-09-28): 1.63×"></div><div class="col over" style="height: 82.8%" title="`dispatch_blyte_start` and `dispatch_blyte_wait`: 1.66×"></div><div class="col over" style="height: 91.0%" title="Time in the comparison and the blyte metrics: 1.82×"></div><div class="col over" style="height: 92.2%" title="The sandbox: 1.84×"></div><div class="col over" style="height: 100.0%" title="Carry read errors from Redis onto `RecordCollection`: 8.00×"></div></div><div class="b-axis"><span>each bar is one task, sorted; hover for the task</span><span>scale 0 to 2× (taller is capped)</span></div></div>

<div class="b-chart" markdown="0"><h4>Cost: blyte ÷ agy, per task</h4><div class="b-sub">65 paired tasks · median 0.64× · below the line in 58 of 65 · list price</div><div class="b-cols"><div class="ref" style="bottom: 50.0%"><span>1.0 = same as agy</span></div><div class="col" style="height: 10.1%" title="`activate` and `status` seed modes: 0.20×"></div><div class="col" style="height: 19.8%" title="The boundary in the system prompt and the failed-gate message: 0.40×"></div><div class="col" style="height: 21.7%" title="Modes and the per-run report: 0.43×"></div><div class="col" style="height: 22.2%" title="An honest completion section for ungated runs: 0.44×"></div><div class="col" style="height: 22.3%" title="The CI workflow: 0.45×"></div><div class="col" style="height: 22.3%" title="A longer line cap for `read`: 0.45×"></div><div class="col" style="height: 22.5%" title="rename `superstate` to `blyte` in the eval code: 0.45×"></div><div class="col" style="height: 23.0%" title="Five new report fields: 0.46×"></div><div class="col" style="height: 23.0%" title="Mark cache breakpoints and count cache writes: 0.46×"></div><div class="col" style="height: 23.0%" title="frontier and implementer cost in the comparison: 0.46×"></div><div class="col" style="height: 23.6%" title="Append the repository state after a successful commit: 0.47×"></div><div class="col" style="height: 24.2%" title="Restore the inherited PATH after the login profile: 0.48×"></div><div class="col" style="height: 24.4%" title="show the model how to batch: 0.49×"></div><div class="col" style="height: 25.2%" title="Per-build cost report (`build_cost.json`): 0.50×"></div><div class="col" style="height: 25.5%" title="Remove the state-engine arms: 0.51×"></div><div class="col" style="height: 26.0%" title="plan-first mode: 0.52×"></div><div class="col" style="height: 26.1%" title="Offline profiles and the --network flag: 0.52×"></div><div class="col" style="height: 27.0%" title="A larger line budget for `read`: 0.54×"></div><div class="col" style="height: 27.4%" title="Whole file for a read from the top: 0.55×"></div><div class="col" style="height: 27.7%" title="Time the run: 0.55×"></div><div class="col" style="height: 27.9%" title="`blyte skill install`: 0.56×"></div><div class="col" style="height: 29.4%" title="Failing steps first on a retry: 0.59×"></div><div class="col" style="height: 29.4%" title="`blyte.toml`, `blyte init`, and dispatch defaults: 0.59×"></div><div class="col" style="height: 29.8%" title="Withhold credential-looking variables, --pass-env keeps one: 0.60×"></div><div class="col" style="height: 30.2%" title="`--default`, `blyte skill uninstall`, and the new skill text: 0.60×"></div><div class="col" style="height: 30.2%" title="Profile enum and a Landlock launcher: 0.60×"></div><div class="col" style="height: 31.2%" title="the `claude+blyte[<model>]` harness: 0.62×"></div><div class="col" style="height: 31.3%" title="Remove the footer: 0.63×"></div><div class="col" style="height: 31.6%" title="Time in the comparison and the blyte metrics: 0.63×"></div><div class="col" style="height: 31.7%" title="Deny credential reads, --allow-read grants: 0.63×"></div><div class="col" style="height: 32.0%" title="quota_exhausted and ProviderError::Auth: 0.64×"></div><div class="col" style="height: 32.0%" title="`.wt/<id>.start.json` and `blyte status`: 0.64×"></div><div class="col" style="height: 32.1%" title="128 workers, more spot types, bigger head, pre-warm one node per partition: 0.64×"></div><div class="col" style="height: 32.8%" title="Remove the daemon and relay arms and the MCP client: 0.66×"></div><div class="col" style="height: 33.0%" title="`--harness claude|codex|all`: 0.66×"></div><div class="col" style="height: 33.0%" title="Quick checks in the system prompt: 0.66×"></div><div class="col" style="height: 33.1%" title="One sentence in the ungated completion text: 0.66×"></div><div class="col" style="height: 33.2%" title="Match-centred excerpts, and a clip note that points to them: 0.66×"></div><div class="col" style="height: 34.1%" title="rename `superstate` to `blyte` in the eval code: 0.68×"></div><div class="col" style="height: 34.1%" title="head and tail: 0.68×"></div><div class="col" style="height: 34.9%" title="Multi-step gate: 0.70×"></div><div class="col" style="height: 34.9%" title="no subagents or wake-ups in the claude arms: 0.70×"></div><div class="col" style="height: 35.8%" title="The ChatGPT-login adapter: 0.72×"></div><div class="col" style="height: 36.2%" title="Default on, `--no-default`, parallel-first wording: 0.72×"></div><div class="col" style="height: 36.5%" title="`dispatch_blyte` in the container MCP server: 0.73×"></div><div class="col" style="height: 36.6%" title="Remove the dead launchers: 0.73×"></div><div class="col" style="height: 36.8%" title="Keep failed gate output in the report: 0.74×"></div><div class="col" style="height: 37.1%" title="Machine-wide gate slots: 0.74×"></div><div class="col" style="height: 37.2%" title="Internal network for the agent, bridge for grading: 0.74×"></div><div class="col" style="height: 37.4%" title="`blyte dispatch` and `blyte review`: 0.75×"></div><div class="col" style="height: 37.7%" title="mark stripped guidance skip-worktree: 0.75×"></div><div class="col" style="height: 38.8%" title="The OpenAI-compatible adapter: 0.78×"></div><div class="col" style="height: 39.0%" title="Remove the MCP arms: 0.78×"></div><div class="col" style="height: 39.1%" title="`Profile::LandlockConfined` and the read allowlist: 0.78×"></div><div class="col" style="height: 41.6%" title="Stamp the build: 0.83×"></div><div class="col" style="height: 46.7%" title="Slots off by default in the library; nested runs skip them: 0.93×"></div><div class="col" style="height: 48.4%" title="The cap: 0.97×"></div><div class="col" style="height: 49.1%" title="Re-base the straggler clock on the current attempt: 0.98×"></div><div class="col over" style="height: 62.4%" title="the Vertex backend for the Gemini provider: 1.25×"></div><div class="col over" style="height: 64.4%" title="Recreate the temp directory before each command: 1.29×"></div><div class="col over" style="height: 72.1%" title="`dispatch_blyte_start` and `dispatch_blyte_wait`: 1.44×"></div><div class="col over" style="height: 100.0%" title="`grep -E` in the fallback: 2.12×"></div><div class="col over" style="height: 100.0%" title="`read` takes several paths (added 2026-09-28): 2.17×"></div><div class="col over" style="height: 100.0%" title="The sandbox: 2.85×"></div><div class="col over" style="height: 100.0%" title="Carry read errors from Redis onto `RecordCollection`: 6.31×"></div></div><div class="b-axis"><span>each bar is one task, sorted; hover for the task</span><span>scale 0 to 2× (taller is capped)</span></div></div>

<div class="b-charts-2"><div class="b-chart" markdown="0"><h4>Total wall time, all paired tasks</h4><div class="b-sub">63 tasks, same model, same plans</div><div class="b-hbars"><div><span class="lbl">agy</span></div><div class="trk"><div class="bar" style="width: 100.0%"></div></div><div class="val">10.8 h</div><div class="me"><span class="lbl">blyte</span></div><div class="trk me"><div class="bar me" style="width: 60.2%"></div></div><div class="val">6.5 h</div></div></div><div class="b-chart" markdown="0"><h4>Total model cost, all paired tasks</h4><div class="b-sub">65 tasks · Gemini 3.8 Flash list price</div><div class="b-hbars"><div><span class="lbl">agy</span></div><div class="trk"><div class="bar" style="width: 100.0%"></div></div><div class="val">$56.44</div><div class="me"><span class="lbl">blyte</span></div><div class="trk me"><div class="bar me" style="width: 71.1%"></div></div><div class="val">$40.12</div></div></div></div>

**What it shows.** On real tasks (new providers, the sandbox, report fields, deleting thousands of
lines of dead code, CI), the same model under blyte produced work of the same quality as under agy
in about half the time, for about two thirds of the cost: 6.5 hours and $40.12 in total against
10.8 hours and $56.44. Three runs are left out of the time figures: a gate-slot bug (fixed the
next day) made blyte wait up to 27 minutes, or deadlocked both runs.

**What it does not show.**

- Most tasks are in two of our own repositories (Rust and Python), with 6 from work repositories,
  and the plans were written by an orchestrating agent, often with the code spelled out.
  Open-ended tasks may behave differently.
- "Mergeable as-is" is a reviewer's judgement, and it counts an equivalent result that was not
  merged because the other was faster.
- Two runs were much slower for blyte (1.8× each). In one it wandered off to clear a host cache
  after a gate failure; in the other a deleted session temp directory broke every later command.
  Both led to fixes now in blyte: the workspace boundary and the write sandbox, and a
  self-healing session. Other ratios above 1.5× are mostly runs where agy gave up early or made no
  changes (up to 8× when agy's run failed after 53 s), so they flatter agy, not blyte.
- In four pairs only agy's result was unusable (no commit, or a crashed run); in two only blyte's:
  the sandbox (above) and a flaky timing test that blyte loosened until it passed, without saying
  so. In three neither was: the gate-slot bug, and two attempts at a 479-edit rename that a
  reviewed script then did.
- One model. The comparison says nothing about blyte against agy on other models.

??? info "All 66 paired runs, task by task"

    | task | agy | blyte | blyte ÷ agy (time, cost) | result |
    |---|---|---|---|---|
    | `read` returns whole files | 382 s, $0.47 | 266 s, $0.25 | 0.70×, 0.54× | identical diffs |
    | gate reports every failure | 460 s, $0.35 | 368 s, $0.24 | 0.80×, 0.70× | identical diffs |
    | read from the top returns the whole file | 320 s, $0.30 | 349 s, $0.16 | 1.09×, 0.55× | identical diffs |
    | quick checks before stopping | 138 s, $0.16 | 89 s, $0.11 | 0.64×, 0.66× | identical diffs |
    | build stamp in `--version` and reports | 160 s, $0.21 | 89 s, $0.18 | 0.56×, 0.83× | identical diffs |
    | eval: remove the MCP arms (−2,680 lines) | 481 s, $0.98 | 288 s, $0.76 | 0.60×, 0.78× | blyte's code better, wrong commit message |
    | eval: remove the daemon and relay arms (−3,510) | 492 s, $0.71 | 231 s, $0.47 | 0.47×, 0.66× | blyte better (agy dropped valid tests) |
    | eval: remove the state-engine arms (−2,780) | 1030 s, $1.64 | 300 s, $0.84 | 0.29×, 0.51× | blyte merged; agy stopped on a plan gap |
    | run telemetry in the report | 394 s, $0.77 | 237 s, $0.42 | 0.60×, 0.55× | same logic; agy's merged (doc comments) |
    | eval: remove 34 dead launch scripts | 249 s, $0.36 | 116 s, $0.26 | 0.47×, 0.73× | identical |
    | OpenAI-compatible provider | 563 s, $0.73 | 256 s, $0.56 | 0.45×, 0.78× | equal; blyte's merged |
    | `read` keeps long lines | 292 s, $0.42 | 113 s, $0.19 | 0.39×, 0.45× | identical diffs |
    | commit footer (an experiment, later removed) | 732 s, $1.10 | 266 s, $0.52 | 0.36×, 0.47× | equal |
    | Anthropic prompt caching | 372 s, $0.51 | 130 s, $0.24 | 0.35×, 0.46× | near-identical |
    | eval: time in the harness comparison | 1661 s, $0.74 | 3024 s, $0.47 | 1.82×, 0.63× | agy's merged: blyte's run cleared a host cache after a gate failure |
    | remove the commit footer | 216 s, $0.30 | 112 s, $0.19 | 0.52×, 0.63× | blyte's clean revert merged |
    | blyte states the workspace boundary | 300 s, $0.41 | 109 s, $0.16 | 0.36×, 0.40× | identical |
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
    | `blyte dispatch` and `blyte review` | 700 s, $1.14 | 349 s, $0.86 | 0.50×, 0.75× | equal; blyte's merged |
    | `blyte skill install` | 307 s, $0.52 | 201 s, $0.29 | 0.65×, 0.56× | equal; blyte's merged |
    | login shell keeps the inherited PATH | 328 s, $0.48 | 177 s, $0.23 | 0.54×, 0.48× | equal; blyte's merged |
    | skill `--default`, parallel dispatch | 481 s, $0.74 | 281 s, $0.44 | 0.58×, 0.60× | equal; blyte's merged |
    | skill default on, parallel first | 340 s, $0.48 | 210 s, $0.35 | 0.62×, 0.72× | equal; blyte's merged |
    | skill for Codex | 503 s, $0.83 | 245 s, $0.55 | 0.49×, 0.66× | equal; blyte's merged |
    | `blyte.toml` and `blyte init` | 584 s, $1.05 | 272 s, $0.62 | 0.47×, 0.59× | equal; blyte's merged |
    | Linux read allowlist | 494 s, $0.83 | 305 s, $0.65 | 0.62×, 0.78× | equal; blyte's merged |
    | machine-wide gate slots | 564 s, $1.00 | 313 s, $0.74 | 0.55×, 0.74× | equal; blyte's merged |
    | `grep` shows match-centred excerpts | 674 s, $1.07 | 368 s, $0.71 | 0.55×, 0.66× | equal; blyte's merged (the plan's approach needed a follow-up) |
    | failing gate steps first on a retry | 559 s, $0.75 | 232 s, $0.44 | 0.42×, 0.59× | equal; blyte's merged |
    | `blyte status` | 615 s, $0.94 | 271 s, $0.60 | 0.44×, 0.64× | equal; blyte's merged (agy's run ended in an error) |
    | one sentence in the ungated text | 173 s, $0.24 | 124 s, $0.16 | 0.72×, 0.66× | equal; blyte's merged |
    | eval: the `dispatch_blyte` tool | 456 s, $0.92 | 1872 s, $0.67 | –, 0.73× | equal; blyte's merged. Its time is not counted: 27 min waiting on a gate-slot bug, fixed next |
    | eval: the `claude+blyte` harness | 854 s, $1.35 | 1905 s, $0.84 | –, 0.62× | blyte's merged (more thorough). Its time is not counted: the same bug |
    | `grep -E` fallback, first try | 1923 s, $0.16 | 2084 s, – | –, – | neither: the gate-slot bug deadlocked both runs (not counted for time) |
    | gate slots off in nested runs | 395 s, $0.49 | 207 s, $0.46 | 0.52×, 0.93× | equal; blyte's merged |
    | `grep -E` fallback | 182 s, $0.21 | 291 s, $0.45 | 1.60×, 2.12× | same fix; blyte's merged, and it found a real test bug |
    | eval: cost columns | 242 s, $0.39 | 136 s, $0.18 | 0.56×, 0.46× | equal; blyte's merged |
    | eval: no subagents in the claude arms | 299 s, $0.43 | 187 s, $0.30 | 0.63×, 0.70× | equal; blyte's merged |
    | eval: plan-first mode | 491 s, $0.96 | 250 s, $0.50 | 0.51×, 0.52× | blyte's merged after adding a log line agy had |
    | a batching example in the prompt | 284 s, $0.36 | 146 s, $0.17 | 0.51×, 0.49× | identical; blyte's merged |
    | Vertex backend for Gemini | 3187 s, $0.73 | 425 s, $0.91 | 0.13×, 1.25× | blyte's merged (one fix); agy produced nothing in 53 min |
    | `read` takes several paths | 216 s, $0.27 | 353 s, $0.58 | 1.63×, 2.17× | blyte's merged; agy made no changes |
    | eval: rename to blyte, first try | 2497 s, $4.63 | 939 s, $3.16 | 0.38×, 0.68× | neither finished 479 hand edits |
    | gate failures keep head and tail | 619 s, $1.00 | 462 s, $0.68 | 0.75×, 0.68× | blyte's merged; agy made no commit |
    | work repo: 128 workers, bigger head | 1443 s, $1.52 | 923 s, $0.98 | 0.64×, 0.64× | same YAML; blyte's merged after removing an out-of-scope test edit |
    | work repo: a per-build cost report | 1341 s, $1.80 | 1019 s, $0.90 | 0.76×, 0.50× | both needed fixes; blyte's merged |
    | eval: rename to blyte, second try | 2861 s, $5.26 | 1417 s, $2.37 | 0.50×, 0.45× | neither finished; a reviewed script did it |
    | eval: stripped guidance stays stripped | 144 s, $0.24 | 118 s, $0.18 | 0.82×, 0.75× | identical; blyte's merged |
    | work repo: straggler clock | 376 s, $0.40 | 540 s, $0.39 | 1.44×, 0.98× | identical code; agy's merged (blyte added an out-of-scope test fixture) |
    | eval: start-then-wait dispatch | 258 s, $0.31 | 427 s, $0.45 | 1.66×, 1.44× | agy's merged: blyte loosened a flaky timing test and did not say so |
    | a service: carry Redis read errors through | 53 s, $0.12 | 424 s, $0.73 | 8.00×, 6.31× | blyte's merged; agy's run failed with a server error |
    | work repo: modes and a per-run report | 807 s, $1.25 | 408 s, $0.54 | 0.51×, 0.43× | equal; blyte's merged |
    | work repo: seed modes | 760 s, $0.73 | 237 s, $0.15 | 0.31×, 0.20× | blyte's merged |

## In work repositories

Since 29 September blyte has also been the implementer in the author's work repositories, planned and
reviewed by Claude Code, with an A/B against agy on a sample of tasks (the six work-repository
pairs are in the table above).

- **Reviewed runs (the dispatch ledger): 14 runs in three repositories, all reviewed: 8 merged
  as-is, 6 needed a fix, none redone.** Four of the six fixes were the same out-of-scope edit (an
  `AWS_PROFILE` workaround in a test fixture, because the sandbox denies `~/.aws` but passed the
  profile variable); blyte now withholds that variable, and no run has made the edit since. The
  other two: a missed `try/except` the plan asked for, and a wrong commit message.
- **Unreviewed runs:** 20 more dispatches in four repositories were started with `blyte dispatch`
  directly, outside the ledger, so they have no review verdict. By blyte's own reports, 18 passed
  their gate and 2 ended in an error (a missing API key, before the key file); about $15.70 of
  Gemini in total, a median of 7 minutes each. New sessions now route every task through the
  ledger.

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

<div class="b-chart" markdown="0"><h4>FrontierHarness: tasks passed, 27 matched tasks</h4><div class="b-sub">bars scaled to all 27 tasks · one attempt each · blyte on a different model from the leaderboard's · see the caveats above</div><div class="b-hbars"><div class="me"><span class="lbl">blyte · Gemini 3.8 Flash</span></div><div class="trk me"><div class="bar me" style="width: 100.0%"></div></div><div class="val">19/27</div><div><span class="lbl">Codex · Kimi K3</span></div><div class="trk"><div class="bar" style="width: 89.5%"></div></div><div class="val">17/27</div><div><span class="lbl">DSH Creator · Kimi K3</span></div><div class="trk"><div class="bar" style="width: 84.2%"></div></div><div class="val">16/27</div><div><span class="lbl">Claude Code · Kimi K3</span></div><div class="trk"><div class="bar" style="width: 84.2%"></div></div><div class="val">16/27</div><div><span class="lbl">Pi · Kimi K3</span></div><div class="trk"><div class="bar" style="width: 78.9%"></div></div><div class="val">15/27</div><div><span class="lbl">DSH PTC · Kimi K3</span></div><div class="trk"><div class="bar" style="width: 78.9%"></div></div><div class="val">15/27</div><div><span class="lbl">DSH Standard · Kimi K3</span></div><div class="trk"><div class="bar" style="width: 78.9%"></div></div><div class="val">15/27</div><div><span class="lbl">Oh My Pi · Kimi K3</span></div><div class="trk"><div class="bar" style="width: 73.7%"></div></div><div class="val">14/27</div><div><span class="lbl">Kimi Code · Kimi K3</span></div><div class="trk"><div class="bar" style="width: 73.7%"></div></div><div class="val">14/27</div><div><span class="lbl">DSH Minimal · Kimi K3</span></div><div class="trk"><div class="bar" style="width: 73.7%"></div></div><div class="val">14/27</div><div><span class="lbl">Exo · Kimi K3</span></div><div class="trk"><div class="bar" style="width: 68.4%"></div></div><div class="val">13/27</div><div><span class="lbl">Hermes · Kimi K3</span></div><div class="trk"><div class="bar" style="width: 68.4%"></div></div><div class="val">13/27</div><div><span class="lbl">OpenCode · Kimi K3</span></div><div class="trk"><div class="bar" style="width: 63.2%"></div></div><div class="val">12/27</div></div></div>

The full write-up, with per-task results and every caveat:
[blyte on the FrontierHarness tasks](research/2026-09-27-frontierharness-gemini.md).

## What was measured along the way

[Harness research](research/2026-09-24-harness-research.md) records what was tried, what the
literature says, and what was removed because it did not pay for itself: a daemon, an MCP server,
a worktree engine, advisory review, a TDD mode, LLM-written fold notes, four losing context
strategies, and more.

## Against Claude doing the work itself

**The question.** If Claude plans and reviews and blyte implements, how much Claude does that save,
and what does it cost in quality and time, compared with Claude doing the whole task?

**Method.** Two real-repository tasks from FrontierHarness, graded by held-out tests:
`python-statemachine-state-data-scoping` (Python) and `katex-multicolumn-array-spans` (TypeScript).
Claude Sonnet in Claude Code, two runs of each setup, interleaved. On the first task, three setups:
Claude implementing a written plan; Claude dispatching the same plan to blyte (Gemini 3.8 Flash)
and reviewing; and Claude on the raw task with no plan. One planning run wrote the plan both planned
setups used, so its cost falls on neither. Claude's own setup and plugins were kept out of every run.

**What it showed.**

- **Planning decided the outcome.** Both setups with the plan passed every run; Claude alone on the
  raw task failed both, a few target tests short (one also broke three existing tests). Who implemented did not change the result.
- **blyte cut Claude's usage by two thirds on the first task and by 86% on the second.** With
  blyte implementing, Claude spent a third as much, then about an eighth as much ($0.13 against
  $0.90), as when it implemented the plan itself. Both differences were larger than the spread
  between runs, and blyte's setup passed every run (Claude alone on the second task: one of two).
- **The total cost depends on the task.** On the first task, where Claude implementing cost about
  $5.60, the hand-off was also about a quarter cheaper in total, counting blyte's Gemini cost. On
  the second, Claude alone was quick and cheap (under $1 in 6–8 minutes), so the Gemini cost made
  the hand-off about three times dearer in total. The Claude saving holds either way.
- **blyte was slower,** two and a half to three and a half times per task. Gemini works in many
  small steps. In our own work, separate tasks dispatched at once finished about twice as fast
  overall as they would have one after another (nearly three times with three or four at once);
  Claude's part stays its plan and its review.
- **Splitting one feature into parallel dispatches did not pay** (two tasks, six runs with a split).
  The parts were rarely independent and of similar size, and each dispatch re-reads the code it
  touches, so splitting cost 30–160% more Gemini and was slower, even with the parts running at
  once. One dispatch per feature was the fastest and cheapest. Parallelism pays across separate
  tasks, not within one. (Claude Code runs one MCP server's tool calls one after another; the
  evaluation's dispatch tool now starts dispatches in the background so they can overlap.)

**Who writes the plan, and is the review needed?** Three more setups on the same task, on Gemini
only, two runs each: blyte alone on the raw task; blyte writing its own plan and then implementing
it; and blyte implementing Claude's plan with no Claude review.

- **A frontier plan with blyte implementing, unreviewed, was the best combination**: it passed both
  times and was the cheapest and fastest pass of any setup, about two dollars of Gemini and a
  quarter of an hour, on top of the one-off plan.
- **Claude's review added no quality here**: with and without it, blyte's work passed; with it, the
  task cost about twice as much and took three times as long. The gate did the checking.
- **A weak plan is worse than none**: from its own plan blyte failed both runs, a few target tests
  short, while from the raw task it passed both.
- **blyte alone beat Claude alone on this task**, passing both runs at about two thirds of the cost.
  On the FrontierHarness tasks blyte alone passed 5 of 9 of the same kind, so this varies by task.

**So:** spend the frontier model on the plan, let a passing gate do most of the checking, and keep
deep review for failed gates and risky changes. The pattern pays when frontier usage is the
constraint (a subscription's limits, or an API bill): it cut Claude's usage by two thirds to 86% on
both tasks. Run separate tasks in parallel; keep one feature in one dispatch. For one small task
that you want done quickly and cheaply in total, Claude alone is faster.

Write-ups are in the eval repository.

**Known costs on Gemini.** Gemini makes one tool call per turn, whatever the prompt asks, so blyte
on Gemini takes more turns than Claude does for the same work, and every turn re-sends the context.
Gemini's automatic prompt caching misses on roughly a quarter of blyte's turns. Both were
investigated; neither a prompt change, a multi-file read, streaming, explicit caching nor the Vertex
AI endpoint improved them, so none of those were kept.

**Limits.** One task and two runs per setup: a direction, not a rate. The plan was unusually
detailed, which favours delegation. Claude's cost is the notional list price that Claude Code reports under a subscription.
Results: blyte-eval `results/offload-v2-sonnet-20260928-1028/` and `results/offload-v3-blyte-20260928-1754/`.

