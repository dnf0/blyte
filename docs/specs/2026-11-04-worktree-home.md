# One home for worktrees: `<main checkout>/.wt/`

## Problem (audit of one machine, 2026-11-04)

There were 227 git worktrees across the user's repositories, about 168 GB by `du`:

| Where | Count | Size |
|---|---|---|
| blyte runs, `<repo>/.wt/<id>` | 57 | 74 GB |
| blyte runs inside another worktree's `.wt/` | 9 | 12 GB |
| next to the repository (`<repo>--<topic>`) | 82 | 50 GB |
| `.claude/worktrees/` (Claude Code's own) | 17 | 14 GB |
| temp and scratchpad folders | 45 | 12 GB |
| other folders | 16 | 4 GB |

There are three causes:

1. **Runs follow the folder they are dispatched from.** `run_dispatch`, `run_review`, `run_status`
   (`dispatch.rs`) and `wait::run` take the root from `git rev-parse --show-toplevel`. Inside a
   linked worktree, that is the worktree itself. So a run dispatched from `.wt/plan-AC` lands in
   `.wt/plan-AC/.wt/AC1`, and `blyte review AC1` from the main checkout can't find it.

   Only `clean::find_repo_root` maps back to the main checkout, and only for paths under `.wt/`.
   Removing the outer worktree deletes its git-ignored `.wt/`. That takes the runs' logs, reports
   and verdicts with it, and leaves the nested run worktrees as broken entries.
2. **Sessions make worktrees wherever they like.** They use `git worktree add` next to the
   repository or in a scratchpad. Nothing says where a worktree should go.
3. **Nothing removes a session's worktree after its PR merges.** The session has usually ended
   by then. `blyte clean` removes only runs (worktrees with a `<id>.start.json`). It reports
   merged worktrees outside `.wt/` with a `git worktree remove` command but never removes them.
   It ignores non-run worktrees under `.wt/` entirely.

Claude Code's own worktrees (`--worktree`, `EnterWorktree`, subagent isolation) go in
`.claude/worktrees/<name>`. Claude Code removes those on exit when they have no changes.

A `WorktreeCreate` hook could move them, but it would replace Claude Code's creation entirely.
Removal would then be blyte's job, including the subagent case, where Claude Code runs no removal
hook. No hook covers `git worktree add` run through Bash (Claude Code docs, worktrees and hooks
pages, checked 2026-11-04). So blyte does not install a hook.

## Decisions

### 1. Every command uses the main checkout as its root

`dispatch`, `review`, `status`, `wait`, `clean` and any other command that reads or writes `.wt/`
or the registry resolve the root the same way: the main checkout of the repository they are run
in. Inside a linked worktree, wherever it is, that is the parent of `git rev-parse
--path-format=absolute --git-common-dir`; this is the existing `config::main_checkout`. Runs
always live in `<main>/.wt/<id>`, and `registry::register` records the main checkout.

The run's base does not change. It is the `--base` value, or, without one, whatever dispatch uses
today from the folder it was started in. Only where the run is stored changes.

No fallbacks: if the main checkout can't be found (for example a bare repository), the command
fails with an error naming the folder. It never falls back to the worktree's own root.

### 2. `blyte clean` removes merged session worktrees inside the repository

`clean` (and so `blyte update` and `blyte review --verdict`, which run it) also looks at two kinds
of linked worktree:
- worktrees directly under `<main>/.wt/` that are not runs (no `<name>.start.json`), such as
  `.wt/plan-<topic>`;
- worktrees under `<main>/.claude/worktrees/`.

It removes one only when all of these hold:
- its branch is merged into the default branch by the same test runs use
  (`is_branch_merged`: ancestor, cherry, merge-tree or squash). For a detached HEAD, the same test
  is applied to its commit.
- it is clean (`is_worktree_clean`): no uncommitted or untracked files.
- it holds no runs: no `<worktree>/.wt/*.start.json`.
- it hasn't been touched for 24 hours: `index` and `HEAD` in its git admin folder
  (`.git/worktrees/<name>/`) are both more than 24 hours old. This keeps a live session's
  worktree.

Removal uses `git worktree remove` (never `--force`), then deletes the local branch the way
`clean` deletes a merged `wip/` branch. `--dry-run` lists what would go. Output follows the run
lines, for example `removed .wt/plan-AC (fix/statusline-compact-window merged, 2.3 GB)` or
`kept .wt/plan-RB (not merged yet)`. A worktree that holds runs is kept, and the output names the
folder to review them from.

Worktrees outside the repository (siblings, temp folders) are reported as today and never removed.

### 3. blyte's installed rule says where session worktrees go

`DEFAULT_BLOCK` (installed into `~/.claude/CLAUDE.md` by `blyte skill install`) gains a
paragraph: a session that needs its own worktree creates it at `<repo>/.wt/plan-<topic>` from the
default branch. It never creates one next to the repository or in a temp or scratchpad folder.
`blyte clean` removes it once its branch is merged and it has no changes. A session still running
at merge time removes it itself. Existing users get the new text on `blyte update`.

## Not covered

- Moving or removing today's worktrees outside `.wt/`. That is a one-off tidy-up, done by hand.
- Claude Code's `worktree.location` setting (Desktop SSH only) and a `WorktreeCreate` hook, as
  explained above.
- Moving existing nested runs. They stay where they are. Decision 2 keeps their outer worktree,
  and they can be reviewed from it.
