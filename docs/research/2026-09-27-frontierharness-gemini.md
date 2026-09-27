# blyte on FrontierHarness with Gemini 3.8 Flash

A single run of blyte, on 26–27 September 2026, over the 30 tasks of [FrontierHarness Eval v1.0](https://frontierharness.org/)
(21 Terminal-Bench 2, 9 DeepSWE), graded by each task's own held-out tests through superstate-eval,
compared task by task with the published results of the 12 FrontierHarness configurations.

**Where blyte fits.** blyte is an implementer, not a planner. It is meant to sit under a frontier
harness: Claude Code, or another harness on a frontier model, writes the plan, then hands each task
to blyte running on a cheap model, and checks the result. This run tests blyte **on its own**: no
plan, a cheap model and the raw task instructions, against harnesses that plan and implement for
themselves. That makes it a hard test of the implementer, not a picture of how blyte is meant to
be used.

**Read the caveats before quoting the numbers.** The model differs (blyte ran on Gemini 3.8 Flash; the
leaderboard runs on Kimi K3), the infrastructure differs (local Docker, not Runta), each task was run
once, and 3 of the 30 tasks could not be matched and are excluded for everyone.

## Result

Like-for-like: 27 matched tasks, one attempt each, binary pass/fail (a task passes only if the
verifier passes it outright, as FrontierHarness scores it).

| Configuration | Model | Passed | Rate |
|---|---|---|---|
| **blyte** (build `9f7cdd0`, folded context, ungated) | Gemini 3.8 Flash | **19/27** | **70.4%** |
| Codex | Kimi K3 | 17/27 | 63.0% |
| DSH Creator | Kimi K3 | 16/27 | 59.3% |
| Claude Code | Kimi K3 | 16/27 | 59.3% |
| Pi, DSH PTC, DSH Standard | Kimi K3 | 15/27 | 55.6% |
| Oh My Pi, Kimi Code, DSH Minimal | Kimi K3 | 14/27 | 51.9% |
| Exo, Hermes | Kimi K3 | 13/27 | 48.1% |
| OpenCode | Kimi K3 | 12/27 | 44.4% |

- **DeepSWE (9 tasks): blyte 5/9**, tying the best configuration (Codex 5/9; the rest 0–4).
- **Terminal-Bench (18 matched tasks): blyte 14/18.**
- **Solved by blyte and by no configuration:** `scc-bounded-memory-spilling`, `largest-eigenval`
  (both offline, under FrontierHarness's conditions).
- **Failed by blyte, passed by most configurations:** `sanitize-git-repo` (10/12).

**What the margin means.** With one run over 27 tasks the standard error of a pass rate is about ±9
points, so a two-task lead over Codex is a tie. The defensible reading is that blyte with Gemini 3.8
Flash is competitive with the best Kimi K3 harnesses on these tasks, and ahead of most. It is not
evidence that blyte is a better *harness* than Codex: the model differs. A same-model comparison
needs blyte on Kimi K3 (Amazon Bedrock offers it; the account role used here lacked
`bedrock:InvokeModel`).

## Method

- **Tasks.** The 30 FrontierHarness v1.0 task ids. 22 came from the local copy of the frontier-eval
  task set, 8 (whose tests were missing locally) from the public
  [terminal-bench-2](https://github.com/laude-institute/terminal-bench-2) repository.
- **Harness.** blyte build `9f7cdd0`, `blyte[folded]`, model `gemini-3.8-flash-high`, no gate (like
  FrontierHarness, the agent decides when it is done and only the hidden tests judge).
- **Runner.** superstate-eval harness mode (`scripts/paired_run.py --harnesses "blyte[folded]"`), one
  task at a time, each task image pulled before its run and removed after it.
- **Network.** FrontierHarness runs every task offline. DeepSWE tasks ran offline throughout.
  Terminal-Bench tasks are graded inside the agent's container by a script that downloads `curl` and
  `uv`, so switching the network off also broke grading; superstate-eval was changed to run the agent
  on an internal Docker network and connect it to the bridge only for grading (eval #56). The
  Terminal-Bench tasks first ran with the network on; every pass was then checked for network use, and
  the 8 affected tasks (7 network-assisted passes, 1 ungraded) were re-run offline:
  `constraints-scheduling` pass, `largest-eigenval` pass, `polyglot-c-py` pass, `dna-insert` fail,
  `kv-store-grpc` fail.
- **Excluded for everyone (unmatched):** `build-cython-ext`, `merge-diff-arc-agi-task`, `regex-log`.
  FrontierHarness rewrote their instructions for offline use (for example a local git bundle instead
  of `git clone` from GitHub); the older versions cannot be run offline. blyte passed all three with
  the network on.
- **Field results.** `results/eval-data.json` from the FrontierHarness repository (first valid attempt
  per task per configuration).

## Cost and time

About $52 of Gemini tokens (main run $46.49 plus re-runs), 5.5 hours for the main pass. Costs are not
comparable with the leaderboard's: the model and its pricing differ.

## What it taught us about blyte

- **Speed within a wall-clock limit decides long tasks.** On `arktype` (solved by 2/12), 7 of the 10
  failing configurations hit the 60-minute limit after 288–380 turns; blyte took 352 turns in 39.5
  minutes. Part of that is the model (Gemini Flash answers quickly), part the harness (its own
  overhead is about 0.7 ms per command).
- **Near misses are specification ambiguities, not a harness gap.** Four DeepSWE failures missed by one
  or two target tests with every existing test passing (`anko` 7/9, `fastapi` 136/137, `httpx` 121/122,
  `katex` 92/94). A prompt defect was found and fixed (ungated, blyte had been told to leave the full
  test run to a check that did not exist; #89), and on the fixed build `anko` passed. The other three
  were unchanged: blyte had run the full suite on both builds, and the missed cases (header-folding
  whitespace, whole-column separator suppression, a date-precedence rule) are not stated in the task
  instructions.
- **An offline agent must not be able to change its host.** In an unrelated run the same week, blyte
  cleared a host cache after a gate failure; blyte now states the workspace boundary itself (#87).

## Where the data is

All three runs are in superstate-eval:

| Run | What it answers |
|---|---|
| [Main run](https://github.com/dnfcx/superstate-eval/tree/main/results/fh-gemini-blyte-20260926-1947) | The 30 tasks on build `9f7cdd0`: the numbers in the result table |
| [Offline re-runs](https://github.com/dnfcx/superstate-eval/tree/main/results/fh-gemini-blyte-offline-rerun-20260927-0122) | The 8 Terminal-Bench tasks that passed with network access, re-run offline |
| [Near-miss re-runs](https://github.com/dnfcx/superstate-eval/tree/main/results/fh-gemini-blyte-ungated-fix-20260927-0209) | The four DeepSWE near misses, on the prompt-fixed build `69fbc6c` |

Each task has `<task>.json` (the verdict, test counts, cost and tokens), `<task>.log` (the runner log) and
`<task>.native_reports/` (blyte's own report and event log).
