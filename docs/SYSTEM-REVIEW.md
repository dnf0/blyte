# Superstate: system review

**Date:** 2026-09-17
**Scope:** the whole system, against its stated design and against what the evaluation measures.
**Prompted by:** the evaluation producing +3pp and the question of whether it was measuring
superstate at all.

**Conclusion up front: it was not.** The design is a dependency graph of units of work, each
verified before the next begins. Three separate layers of loss stand between that design and
every number recorded so far, and only one of them is in the evaluation.

---

## 1. What superstate is designed to be

The README is unambiguous:

> It decides **which task the agent is given next**, and whether the work it produces is allowed
> to advance … one task at a time, **drawn from a dependency graph**, with no progress until a
> verification command passes.

ADR 0001 (*Progressive Staged Verification*) assumes the same: *"agents executing multi-step
implementation plans"*, *"a 4-milestone task"*, per-milestone verification with impact-scoped
test slicing, and the full suite only at the end.

The data model backs it:

```rust
pub struct TaskSpec {
    pub id: String,
    pub title: String,
    pub depends_on: Vec<String>,          // the graph edges
    pub files_modify: Vec<PathBuf>,       // blast radius, and the split key for decompose
    pub interfaces_produces: Vec<String>, // what later tasks may consume
    pub verification_cmd: Option<String>, // this unit's definition of done
}
```

That is a state-driven tracking system that decomposes a problem into units of work and fixes
each before moving on.

## 2. What is actually built

Substantially more than the evaluation uses. These are real, not stubs:

| module | lines | what it does |
|---|---|---|
| `state_machine.rs` | — | plan → state, `ready_tasks` (DAG), `advance_step` |
| `plan_parser.rs` | — | parses `### Task N:` and `**Depends on:**` |
| `impact.rs` | 346 | test slicing: which tests a diff actually affects |
| `daemon.rs` | — | autonomous loop, gate, retry, checkpoint commits |
| `checkpoint.rs` | 87 | git checkpoint and rollback |
| `superstate-review` | 103 | adversarial review over a git diff |
| `handoff.rs`, `ledger.rs` | 235 / 84 | cross-agent handoff, epistemic ledger |

The CLI exposes 21 `state` verbs plus `review`, `worktree`, `daemon`, `handoff`.

## 3. Three layers of loss

### Loss 1 — the graph does not drive execution

`ready_tasks()` computes which tasks are unblocked. Its own docstring says why it exists:

> This is what makes the plan a DAG rather than a list: without it a runner can only walk
> milestones in file order, which is not the same thing.

**It is called in exactly one place** — `state ready`, a command that *prints* the answer.

The daemon advances with `advance_step()`, which is:

```rust
state.active_task = Some(state.pending_tasks.remove(0));
```

File order. So the dependency graph is parsed, modelled, computed on request, and **never used
to choose what runs next**. The one consumer that does walk the DAG is the *evaluation's*
`superstate_mcp` adapter, which calls `state_ready` — the Rust daemon does not.

### Loss 2 — `interfaces_produces` is dead

The field is declared, deserialised, and written as an empty vector in every construction path.
**It is never read anywhere in the codebase.** The contract "task N produces X, which task N+1
consumes" — the thing that would let a later unit validate an earlier one — is modelled and
unimplemented.

### Loss 3 — progressive verification is not wired in

ADR 0001's whole purpose is running only impacted tests per milestone. `impact.rs` implements
the analysis in 346 lines. **Nothing in the execution path calls it.** Each milestone runs
whatever `verification_cmd` it was given, full stop.

Likewise `superstate-review` is reachable only as a standalone command and through one MCP tool
name; neither the state machine nor the daemon invokes it.

Checkpointing is the exception: the daemon does commit on gate pass. Rollback is never called.

## 4. What the evaluation measured

Against all of that, the evaluation exercised `init`, `prompt`, `next`, `ready`, `exec`,
`status`, `note`, `doctor`, `daemon`, `handoff` — and **never** `review`, `sync`, `checkpoint`,
`rollback`, `decompose`, `resume`, `metrics`, `validate`, `apply` or `list`.

Worse, it ran **253 of 309 runs on a single-milestone plan**. `zero_turn_planning=True` entered
on 2026-09-11 as a speed "fast-path", became the default the next day, and was then built into
Condition B — the configuration behind every headline number. With one node there is no graph,
no task selection, no staged verification and nothing for `ready_tasks` to order.

`files_modify` is written as `[]` on every milestone, so `state decompose` would be a no-op and
`impact.rs` would have nothing to slice, even if either were called.

**So every measurement describes `init` + `prompt` + one shell gate.** The +3pp effect, the 17%
versus 92% gate precision, the daemon-equals-mcp finding — all of it characterises that subset.
None of it tests the control plane.

## 5. What follows

The evaluation's negative result is not evidence against the design. It is evidence that a
prompt plus one gate does not help much when the gate is poor — which is unsurprising, and not
what the product claims.

In dependency order:

1. **Make the graph drive execution.** `advance_step` should select from `ready_tasks`, not
   `pending_tasks[0]`. Without this the DAG is decoration, and this is a small change.
2. **Populate `files_modify`** when a plan is produced. It unlocks `state decompose` and
   `impact.rs` at once; both are already written.
3. **Evaluate multi-milestone execution on datacurve**, where single-milestone superstate loses
   to vanilla (23-27% against 43%) and where pooled history suggests decomposition wins
   (anko 100% vs 33%, httpx 100% vs 44%, arktype 50% vs 11%).
4. **Wire `review` into the advance decision, or evaluate it beside the gate.** It is the only
   determination of "done" in the system that does not require the success criterion to be
   written before the work exists — which is precisely why gate precision collapses from 92% to
   17% when the instruction stops stating that criterion.
5. **Decide whether `interfaces_produces` is the design or dead weight.** If a later unit
   validating an earlier one is the intended mechanism, it needs implementing. If not, remove
   the field so it stops implying a capability that does not exist.

Nothing here requires new architecture. Items 1, 2 and 4 are wiring between components that are
already built and tested.
