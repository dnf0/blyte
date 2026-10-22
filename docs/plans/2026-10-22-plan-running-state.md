# Plan: `blyte plan` must not call a running or empty run "merged" (2026-10-22)

## Why

Reported by the orchestrator session (blyte 0.37.1). About 30 seconds after
`blyte dispatch CL6 ...`, `blyte plan` printed `CL6  merged`, while `blyte status CL6` said
`state: running`, turn 2.

Cause: `crates/blyte-cli/src/plan_cmd.rs`, rule 1 (about line 220). `is_merged` is true whenever
`git merge-base --is-ancestor wip/<id> HEAD` succeeds (`check_is_ancestor`, line 119). This rule is
checked before "running". A new `wip/<id>` branch has no commits yet: it points at its base commit
(the reflog shows `branch: Created from HEAD` at 5a01fd1), and that commit is an ancestor of HEAD.
The same happens with any finished run that made no commits (`no_changes`, or a failure before the first commit).

Verified 2026-10-22:
- `.wt/<id>.start.json` holds `pid`, `started_at` and `base: "HEAD"` (symbolic, no SHA).
- `.wt/<id>.json`, the summary written when the run ends, has `commits: [{"sha", "subject"}, ...]` and `outcome`.

### Task PS1: running before merged; the ancestor rule needs commits

Files:
- `crates/blyte-cli/src/plan_cmd.rs`: only `read_summary` (line 150) and the rules block (about lines 213-247);
- a new test in `crates/blyte-cli/tests/test_plans_running.rs` (copy its existing setup).

No fallbacks: a run whose state cannot be shown from its files is `dead`, as today, never `merged`.

1. `read_summary` also returns whether the summary's `commits` array is non-empty. A missing or
   non-array `commits` counts as empty. Adjust its callers.
2. Order of the rules:
   - **Running first:** if there is no summary and `run.pid` is alive, the state is `running`, whatever the branch says.
   - **Merged:** keep the second half as it is (`!branch_exists` with an `accepted`/`fixed` verdict).
     Count `is_ancestor` only when the summary exists **and** its `commits` is non-empty.
   - Keep the rest as it is: `redone` → no run, a summary → `review`, otherwise `dead`.
3. Tests, in `test_plans_running.rs`, using its helpers. Each case must fail before the change.
   - **Still running:** a plan task with a start record whose pid is alive (the test's own pid, as the existing running test does), and branch `wip/<id>` at HEAD with no commits beyond it.
     `blyte plan` shows `running`, not `merged`.
   - **No commits:** the same, but a dead pid and a summary with `outcome: "no_changes"` and `commits: []`. `blyte plan` shows `review`, not `merged`.
   - **Merged:** a summary with one commit, and `wip/<id>` merged into HEAD. `blyte plan` still shows `merged`.

Gate: the repository checks. Commit: `fix(plan): show a running or commit-less run as running or review, not merged`
