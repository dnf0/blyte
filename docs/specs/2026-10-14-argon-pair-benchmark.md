# Argon pair benchmark: blyte orchestrating blyte

Date: 2026-10-14. Status: agreed with the user.

## Question

How fast and how expensive is blyte orchestrating blyte on Gemini models, and is the orchestrator worth its cost?

## Arms

| Arm | Orchestrator | Implementer |
|---|---|---|
| A | `vertex/gemini-4-argon` | `vertex/gemini-4-argon` |
| B | `vertex/gemini-4-argon` | `gemini-3.8-flash-high` |
| C (control) | none | solo blyte on `vertex/gemini-4-argon` |

Vertex runs with `GOOGLE_CLOUD_LOCATION=global`, the default; Argon is not served in regions.

## Tasks and repetitions

- Pilot: 5 tasks × 1 run per arm, from `~/.cache/agent-rules/frontier-eval/tasks/` (30 tasks there).
  It measures real cost and time per run.
- Full run: up to 20 of those tasks × 3 runs per arm. The size and budget are set from the pilot.
- Budget: the pilot is capped at about $30 in total; the full budget is decided after the pilot.
  Every run, and every dispatch, also gets its own `--max-cost`.

## Metrics

- Speed:
  - wall clock per task;
  - orchestrator and implementer time separately;
  - model time and tool time per turn (`model_ms` and `tools_ms`, already in `turn_usage`).
- Cost:
  - USD per task, split into orchestrator and implementers;
  - cost per solved task;
  - cache hit ratio.
- Quality:
  - pass rate from the sealed grader;
  - dispatches per task;
  - gate attempts.

Costs come from blyte's own `cost_usd`, which prices fresh, cached, output and thinking tokens
(`crates/blyte-provider/src/lib.rs`). The eval does not price tokens itself.

## What has to change first

1. **blyte: headless `--orchestrate`.** Today `--orchestrate` is interactive only (`main.rs` refuses a prompt).
   - With a prompt, it runs the orchestrator once, writes `--report`, and exits.
   - With `--dispatch-cmd <cmd>`, the orchestrator gets a `dispatch` tool that runs `<cmd>` on the host,
     passing the request as JSON on stdin and returning its stdout.
   - This is needed because in the eval the orchestrator's shell runs in a container with no network and no blyte.
2. **blyte-eval: honest cost.** `blyte_native` prices tokens with its own table, which has no Argon entry
   and returns $0.00. Use the report's `cost_usd` instead.
3. **blyte-eval: a `blyte+blyte:<implementer-model>` harness.**
   - It runs the headless orchestrator, with `--dispatch-cmd` wired to the existing dispatch logic in
     `container_exec_mcp.py` (copy, run the implementer, merge back).
   - It sums the dispatch reports, as `claude+blyte` does: `frontier_cost_usd` plus `implementer_cost_usd`.

## Not doing

- Time to first gate pass: not recorded today and not needed to answer the question.
- Claude arms: this benchmark is Gemini only. `claude+blyte` results can be compared from earlier runs if wanted.
