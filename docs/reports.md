# Run reports

`--report path.json` writes one JSON object when a headless run exits, on every outcome, including
an error before the first turn. Every field is always written: a zero is a measurement, and a
missing key would be a bug. `schema_version` changes only when a field changes meaning or is
removed; new fields are added without a bump.

```json
{
  "schema_version": 1,
  "harness_build": "933da30",
  "outcome": "verified",
  "error": null,
  "model_requested": "gemini-3.8-flash-high",
  "model_id": "gemini/gemini-3.8-flash",
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
| `outcome` | `verified`, `budget_exhausted`, `gate_failed` or `error` |
| `error` | the error message for an `error` outcome, else `null` |
| `model_requested` / `model_id` | the `--model` given, and what the provider actually called |
| `context` | `folded` or `conversational` |
| `gated`, `gate_source` | whether a gate was given; `user` or `none` |
| `duration_seconds` | wall-clock time of the run |
| `final_message` | the model's last text: its own report of what it did (what `blyte dispatch` shows the orchestrator) |

**Work**

| field | meaning |
|---|---|
| `turns`, `empty_turns` | model calls, and calls that returned neither text nor a tool call |
| `tool_calls`, `tool_counts` | tool calls in total and by tool |
| `gate_attempts`, `gate_ms`, `gate_failures` | gate runs, the time of each, and the (trimmed) output of each failure |
| `rollbacks` | times the tree was reset after a failed gate |
| `provider_retries` | transient provider errors waited out |

**Cost and time**

| field | meaning |
|---|---|
| `usage` | tokens over the run: fresh `input`, `cached` input, `output`, `thinking` |
| `turn_usage` | the same per call, with the prompt's size and how much of it was new |
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
shape; an orchestrator can read `outcome`, `gate_failures` and `cost_usd` to decide whether to
merge, retry or escalate. `jq` is enough for most questions:

```bash
jq '{outcome, turns, cost_usd, model_s: .model_seconds, gate_s: .gate_seconds}' run.json
```
