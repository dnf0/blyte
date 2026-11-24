# Run reports

`--report path.json` writes one JSON object when a headless run exits, on every outcome, including
an error before the first turn. Every field is always written: a zero is a measurement, and a
missing key would be a bug. The exceptions are the research fields `quote_problems`, `claim_check` and
`claim_check_skipped`, and a turn's `failed_attempts`, which are left out when there is nothing to record. `schema_version` changes only when a field changes meaning or is
removed; new fields are added without a bump.

```json
{
  "schema_version": 1,
  "harness_build": "933da30",
  "outcome": "verified",
  "error": null,
  "model_requested": "gemini-3.8-flash-high",
  "model_id": "gemini/gemini-3.8-flash",
  "effort": "high",
  "context": "folded",
  "gated": true,
  "gate_source": "user",
  "container": null,
  "container_workdir": "/workspace",
  "sandboxed": true,
  "sandbox": "seatbelt",
  "network": "on",
  "allow_read": [],
  "pass_env": [],
  "withheld_env": ["GITHUB_TOKEN", "SSH_AUTH_SOCK"],
  "duration_seconds": 186.4,
  "turns": 38,
  "empty_turns": 0,
  "tool_calls": 51,
  "gate_attempts": 1,
  "rollbacks": 0,
  "provider_retries": 0,
  "cost_usd": 0.4566,
  "max_cost_usd": null,
  "cost_capped": false,
  "fold_budget": null,
  "turn_usage": [ { "turn": 1, "input": 14210, "cached": 0, "output": 312, "thinking": 540, "...": "..." } ],
  "usage": { "input": 3120455, "cached": 2811020, "output": 18011, "thinking": 40210 },
  "gate_ms": [41230],
  "gate_turns": [38],
  "gate_failures": [],
  "tool_counts": { "bash": 22, "read": 17, "edit": 9, "grep": 3 },
  "model_seconds": 121.7,
  "tools_seconds": 20.1,
  "gate_seconds": 41.2,
  "final_message": "Changed parse() to return Err(ParseError::Empty) ... Deviations: none."
}
```

## Fields

**The run**

| field | meaning |
|---|---|
| `harness_build` | the commit blyte was built from |
| `outcome` | `verified`, `finished` (an ungated run that stopped by itself), `budget_exhausted`, `gate_failed`, `blocked` or `error` |
| `error` | the error message for an `error` or `blocked` outcome, else `null` |
| `model_requested` / `model_id` | the `--model` given, and what the provider actually called |
| `effort` | the effort level when the model supports it, else `null` |
| `context` | `folded` or `conversational` |
| `gated`, `gate_source` | whether a gate was given; `user` or `none` |
| `duration_seconds` | wall-clock time of the run |
| `final_message` | the model's last text: its own report of what it did (what `blyte dispatch` shows the conversation model) |

**Work**

| field | meaning |
|---|---|
| `turns`, `empty_turns` | model calls, and calls that returned neither text nor a tool call |
| `tool_calls`, `tool_counts` | tool calls in total and by tool |
| `gate_attempts`, `gate_ms`, `gate_failures` | gate runs, the time of each, and the (trimmed) output of each failure |
| `gate_turns` | the turn on which each gate attempt ran, in order (one entry per gate attempt) |
| `rollbacks` | times the tree was reset after a failed gate |
| `provider_retries` | transient provider errors waited out |
| `gate_wait_ms` | time spent waiting for a machine-wide gate slot on each attempt, in order (0 when a slot was free) |
| `gate_left_out` | dispatch only: the gate steps `blyte.toml` records as left out, each with its reason |
| `brief_missing` | research dispatch only (in `.wt/<id>.start.json`): the brief sections the question lacked (`[]` when all four are present) |
| `truncated_turns` | turns whose reply hit the output token limit and was cut off |
| `big_read_turns` | turns whose tool output was more than half of the fold budget |
| `read_loop`, `peak_read_repeats` | whether the run stopped because it kept repeating `read`, `grep` and `glob` calls without changing the workspace, and the most repeats seen between two changes |
| `fold_thrash` | whether the run stopped because consecutive turns kept folding the previous turn's results |
| `stalled` | whether a fix run stopped because it made no change and kept calling tools |

**Cost and time**

| field | meaning |
|---|---|
| `usage` | tokens over the run: fresh `input`, `cached` input, `output`, `thinking` |
| `turn_usage` | the same per call (`turn`, `input`, `cached`, `output`, `thinking`), with the prompt's size (`prompt_messages`), whether it was the previous prompt plus new messages (`appended`) and how much was new (`new_chars`), the fold budget in force, and timings: `model_ms` (the model call, retries included), `tools_ms` (running the tool calls), `first_data_ms` and `max_gap_ms` (time to the first streamed data and the longest silence; `null` and 0 when the provider does not stream) |
| `cost_usd` | list-price cost, or `null` when the model's price is unknown |
| `max_cost_usd`, `cost_capped` | the `--max-cost` cap, and whether the run stopped at it |
| `fold_budget` | the `--fold-budget` given, if any |
| `model_seconds`, `tools_seconds`, `gate_seconds` | where the time went; the rest is start-up and the harness itself |

**Security settings**

| field | meaning |
|---|---|
| `sandboxed` | whether host commands ran sandboxed |
| `sandbox` | `seatbelt`, `landlock`, `landlock-offline`, `container` or `off` |
| `network` | `on` or `off` |
| `allow_read`, `pass_env` | the `--allow-read` grants and `--pass-env` names |
| `withheld_env` | names, never values, of variables withheld from commands (empty in a container) |
| `container`, `container_workdir` | the container, if the run used one |

## Using reports

Reports are designed to be compared. The evaluation harness that measures blyte reads exactly this
shape; a conversation model can read `outcome`, `gate_failures` and `cost_usd` to decide whether to
merge, retry or escalate. `jq` is enough for most questions:

```bash
jq '{outcome, turns, cost_usd, model_s: .model_seconds, gate_s: .gate_seconds}' run.json
```
