# Plan: headless orchestrator for the Argon pair benchmark

Spec: `docs/specs/2026-10-14-argon-pair-benchmark.md`. The blyte-eval side has its own plan,
`~/repos/blyte-eval/docs/plans/2026-10-14-argon-pair-benchmark.md`, and needs this task merged and released first.

### Task HO1

**Goal:** `blyte --orchestrate` can run unattended: given a prompt, it orchestrates once, writes `--report`, and exits.
With `--dispatch-cmd <cmd>`, it delegates work through a host-side `dispatch` tool, not through `blyte dispatch` in its shell.

**Where:**
- `crates/blyte-cli/src/main.rs`: today it refuses `--orchestrate` with a prompt (around line 174).
- `crates/blyte-cli/src/cli.rs`: `orchestrate` is at line 358; `apply_orchestrate_defaults` is at line 454.
- `crates/blyte-cli/src/task_runner.rs`: the one-shot path, `run_task`.
- `crates/blyte-cli/src/repl.rs`: how orchestrate mode builds its prompt today (around line 80, `blyte_agent::orchestrator::prompt_sections`) and turns the host sandbox off (around line 620).
- `crates/blyte-agent/src/tools.rs`: the tool registry.

**Behaviour:**
1. Remove the refusal. `blyte --orchestrate [flags] -- "<prompt>"` runs the orchestrator through the one-shot task path:
   - it uses the orchestrate prompt sections and `apply_orchestrate_defaults`;
   - it uses the same host-sandbox setting as the REPL's orchestrate mode;
   - it never takes a `--gate`, since the orchestrator's work is judged by the implementers' gates and the caller.

   The run ends when the model stops calling tools. Its outcome is reported as finished, not verified, because there is no gate. Pick the existing `Outcome` variant that fits, or add one, and document it.
2. It writes the same `RunReport` as any one-shot run when `--report <path>` is given:
   - `wall_clock_secs`, `usage`, `turn_usage` and `cost_usd`;
   - `cost_usd` covers the orchestrator's own tokens only.
3. A new flag, `--dispatch-cmd <shell command>`, is valid only with `--orchestrate`.
   - When set, the model gets a `dispatch` tool with input `{ "task": string, "gate": string, "max_seconds": integer (optional) }`.
   - blyte runs the command on the host with `sh -c`, outside any container and sandbox, writes the input JSON to its stdin, and waits.
   - The tool result is the command's stdout, capped like other tool output. A non-zero exit status is returned as an error result carrying the stderr tail.
   - Set `BLYTE_DISPATCH_PARENT` to the orchestrator's session id in the command's environment.
4. With `--dispatch-cmd` set, the orchestrate prompt tells the model to delegate through the `dispatch` tool, one self-contained task per call, and not to run `blyte dispatch` in its shell. Without it, the prompt is unchanged.
5. Interactive `--orchestrate` with no prompt keeps working as it does today.

**Tests:**
- `--orchestrate` with a prompt is accepted. `--dispatch-cmd` without `--orchestrate` is rejected by clap.
- A `dispatch` tool call runs a stub command such as `cat`, which echoes the JSON. The result contains the stdout, and a failing command gives an error result.
- The prompt contains the dispatch-tool instruction only when `--dispatch-cmd` is set.

**Docs:** add `--dispatch-cmd` and headless orchestrate to the CLI reference in `docs/` wherever `--orchestrate` is documented.
