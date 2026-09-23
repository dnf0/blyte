# Native Harness Eval Surface

**Date:** 2026-09-23
**Status:** Approved (direction), pending spec review
**Repos:** `superstate` (Part A), `superstate-eval` (Part B)

## Why

Superstate is moving from a layer attached to Claude Code / agy to being a harness in its own right (`superstate "<prompt>"`, #42). The evaluation has not followed: every adapter in `superstate-eval` (`superstate`, `superstate_daemon`, `superstate_mcp`, `superstate_agent_mcp`) drives Claude Code or agy. No run has ever measured the native harness, so every harness change since #42, including #44's working set and loop-breaker, is unmeasured, and none of the eval's earlier numbers transfer to it.

The eval's own conclusion (FINDINGS 13.25, 13.28, 13.29) is that cost scales with call count and that reducing calls is a worker-plane problem that the bolt-on could not reach. The native harness is the worker plane. This spec makes it measurable.

## Decisions

1. **The native harness is a driver, not an arm.** In eval terms it replaces the *worker runtime* (agy / claude). It is wrapped as `SuperstateNativeDriver` (`harness_name` `superstate[<config>]`) so it can run under the existing `vanilla` arm. The first comparison is `vanilla` + agy vs `vanilla` + superstate, same model, same prompt, both graded by the sealed grader. That isolates "is our harness better than agy's harness" from every arm-level mechanism.
2. **Ungated first.** All three canary tasks ship only the sealed grader (`bash /tests/test.sh`), which no agent may run. The daemon arm gets a gate through autonomous TDD; the native harness has none yet. The first native runs pass no `--gate`, exactly like `vanilla` + agy. Autonomous TDD in the native harness is a separate, parallel spec (Part C, not this document).
3. **Model parity is enforced, not assumed.** agy's model string `gemini-3.8-flash-high` carries an effort suffix. The native Gemini provider currently sends the whole string as the model id and sets no thinking level. The provider learns the suffix: `-low`/`-medium`/`-high` are stripped from the id and sent as `generationConfig.thinkingConfig.thinkingLevel`. `-xhigh`/`-max` are left in the id, so the API rejects them loudly rather than running at a silently different effort.
4. **The loop-breaker must be ablatable and must not starve the worker.** #44's guard re-intercepts an identical request forever, because the intercepted call is itself logged as a read of that path. Under a finite turn budget that can spend the whole run on refusals. Two changes:
   - **Insist wins:** if the immediately preceding action was an interception of the identical call (same tool, same target), the call is executed. This matches the daemon read cache's escape (FINDINGS 13.22).
   - **Switchable:** `Runner.loop_breaker: bool` (default `true`), exposed as `--no-loop-breaker`.
5. **The harness reports itself as JSON.** The headless solver currently prints `usage: Usage { .. }` via `Debug`. A new `--report <path>` writes one JSON object on every exit path, including provider errors. This is the whole contract between Part A and Part B.
6. **The harness enforces its own deadline.** A run killed from outside writes no report, and its tokens vanish from the totals. That is how the daemon once hid 3.6M input tokens and flipped a cost conclusion (FINDINGS 13.5). `Budget` gains `max_duration: Option<Duration>`, checked at the top of every turn. When it is spent, the run ends `budget_exhausted` and the report is written like any other outcome. It is exposed as `--max-seconds <n>`.
7. **Container workdir is explicit.** `ContainerShell` hardcodes `/workspace`; the eval mounts the task repository at `/app`. A new `--container-workdir <path>` (default `/workspace`, unchanged behaviour) sets it.

## Part A — `superstate` changes

| Change | Where |
|---|---|
| Insist escape, `loop_breaker` field, `Budget.max_duration` | `crates/superstate-agent/src/run.rs`, plus the 9 `Runner { .. }` and 8 `Budget { .. }` literals |
| Effort suffix → `thinkingLevel` | `crates/superstate-provider/src/gemini.rs` |
| `--no-loop-breaker`, `--container-workdir`, `--max-seconds`, `--report` | `crates/superstate-cli/src/cli.rs`, `task_runner.rs`, new `report.rs`, `main.rs` |

Exit codes are unchanged: `0` when verified, `1` otherwise. The report, not the exit code, is what the eval reads.

### Report schema (version 1)

Written to `--report <path>` as a single JSON object, pretty-printed, on every exit from headless mode, including a provider error before the first turn.

```json
{
  "schema_version": 1,
  "outcome": "verified",
  "error": null,
  "model_requested": "gemini-3.8-flash-high",
  "model_id": "gemini/gemini-3.8-flash",
  "context": "state-only",
  "loop_breaker": true,
  "gated": false,
  "container": "3f2a…",
  "container_workdir": "/app",
  "duration_seconds": 412.7,
  "turns": 18,
  "empty_turns": 0,
  "tool_calls": 31,
  "gate_attempts": 1,
  "rollbacks": 0,
  "usage": { "input": 812345, "cached": 402110, "output": 12001, "thinking": 30455 },
  "interceptions": { "read": 3, "bash": 1 },
  "insists": 1,
  "agent_note_keys": ["hypothesis", "next_step"]
}
```

| Field | Meaning |
|---|---|
| `outcome` | `verified` \| `budget_exhausted` \| `gate_failed` \| `error` |
| `error` | the error message for `error` outcomes, otherwise `null` |
| `model_requested` | `--model` exactly as passed, or `null` |
| `model_id` | what the provider actually called (`Provider::model_id()`), `null` if the provider never resolved |
| `gated` | `true` iff a non-empty `--gate` was supplied |
| `turns`, `empty_turns`, `tool_calls`, `gate_attempts` | from `RunReport` |
| `rollbacks` | `state.runtime.rollbacks` |
| `usage` | `RunReport.usage`, all four fields |
| `interceptions.read` / `.bash` | actions whose summary is `intercepted: already in working set` / `intercepted: redundant inspection` |
| `insists` | actions that were executed immediately after an interception of the same tool and target |
| `agent_note_keys` | sorted keys of `state.agent.fields`. This is the measurement FINDINGS 13.30 asks for |

Zero values are written, never omitted. On an `error` outcome before the run starts, the counters are `0`, `usage` is all zeros, and `model_id` is `null`.

## Part B — `superstate-eval` changes

A new driver, `lab/engine/drivers/superstate_native.py`, class `SuperstateNativeDriver(AgentDriver)`:

- `harness_name` encodes the configuration, for example `superstate[state-only]` or `superstate[conversational,no-loop-breaker]`, because results are pooled by it. `provides_isolated_execution = True`. All tool execution goes through `docker exec` into the task container, and the host shell is never offered to the model.
- `run(...)` invokes, with `cwd=workspace_path`:
  `superstate <prompt> --model <model> --context <context> --container <container_id> --container-workdir /app --max-turns 1000 --max-seconds <timeout_sec> --report <tmpfile>` plus `--no-loop-breaker` when disabled. No `--gate`.
- It reads the report and returns `DriverTelemetry(success = outcome == "verified", input_tokens = usage.input, output_tokens = usage.output, cost_usd = estimate_cost_usd(model, input_tokens, output_tokens), raw_response = "", conversation_id = None)`. This is exactly the agy driver's accounting. agy reports `input_tokens`, `output_tokens`, `thinking_tokens` and `cache_read_tokens` separately, and `AgyCliDriver` counts only the first two. The native Gemini provider splits usage the same way (`input` = `promptTokenCount − cachedContentTokenCount`; `output` = `candidatesTokenCount`; thinking separate). `cached` and `thinking` are not added into the totals, or the native arm would look more expensive for bookkeeping reasons alone. The full report, including `cached` and `thinking`, is kept on the driver as `last_report` for provenance.
- **The time budget binds, the turn budget does not.** agy has no turn cap: it runs until it stops or its `--print-timeout` fires. The native harness defaults to `--max-turns 25`, which would cap the native arm where agy is uncapped. The driver therefore passes `--max-turns 1000`, a loop-breaker rather than a budget, the same stance as `TurnBasedAdapter.max_turns`, and passes `--max-seconds <timeout_sec>` so the harness stops itself and reports. A turn already in flight can overrun (a model call may take up to 300s, a command up to 300s), so the driver only kills the subprocess at `timeout_sec + 900`. A run killed that way has no report and is `success=False` with `error_message="superstate killed after …s with no report"`. It should never happen, so if it does, it is a defect to investigate, not a result.
- `run` returns a completion signal the `vanilla` loop treats as done: an empty `raw_response`, so `resolve_next_turn_prompt` returns `None` and the arm makes exactly one driver call.
- A missing or unparseable report, or a non-zero exit with no report, is `success=False` with the stderr tail as `error_message`. It is never a pass.

Selection: `scripts/paired_run.py --driver superstate` with `--context {state-only,conversational}` (default `state-only`) and `--no-loop-breaker`. The model defaults to the same `--model` as every other driver, so parity is by construction.

Parity tests (`tests/test_superstate_native_driver.py`) use a fake `superstate` binary that writes a canned report and records its argv. They assert the model string is passed verbatim, `--container-workdir /app` is present, no `--gate` is passed, `--no-loop-breaker` appears only when requested, token totals are `usage.input` / `usage.output` only (agy parity), and a missing report is a failure. No test calls a model or Docker.

## Evaluation plan (after A and B merge)

The decision each run informs is named, per `superstate-eval/CLAUDE.md`.

1. **Wiring smoke** (Claude, minutes, cents): one canary task, native driver, confirm the report parses and the sealed grader runs. Decision: is the instrument trustworthy?
2. **Harness vs harness** (`paired_run.py --canary`, `vanilla` arm, drivers `agy` and `superstate`, `state-only`): tokens, turns and solve rate. Decision: is the native harness at least at parity with agy's before anything is built on it?
3. **Loop-breaker ablation** (native driver, loop-breaker on vs `--no-loop-breaker`): tokens, turns, `interceptions`, `insists`. Decision: does the loop-breaker stay on by default?

## Out of scope

- Autonomous TDD / gating in the native harness (Part C, its own spec).
- Cost pricing for the native provider's API billing. `estimate_cost_usd` uses the same table as agy, so costs are comparable, not invoices.
- Any change to existing arms or drivers.
