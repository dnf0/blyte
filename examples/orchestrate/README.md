# Orchestrating blyte

A frontier model plans and reviews; blyte implements on a cheaper model; the repository's check
decides when the work is done. See [docs/orchestration.md](../../docs/orchestration.md) for the
pattern.

| file | what it is |
|---|---|
| `blyte-dispatch.sh` | hand one task file to blyte in its own worktree (`.wt/<id>`, branch `wip/<id>`), with a gate; prints a JSON summary |
| `blyte-review.sh` | show a run's commits, changed files, tree state, log and diff, for review before merging |
| `claude-skill/blyte-implementer/SKILL.md` | a Claude Code skill that teaches Claude the workflow: write the task, dispatch, verify, merge |

## Install the Claude Code skill

```bash
mkdir -p ~/.claude/skills/blyte-implementer
cp claude-skill/blyte-implementer/SKILL.md blyte-dispatch.sh blyte-review.sh \
   ~/.claude/skills/blyte-implementer/
```

Then ask Claude Code to "use blyte to implement" a task, or mention the skill by name. The skill
works in any git repository that has a check command; add `.wt/` to the repository's `.gitignore`.

## Without Claude Code

The scripts are plain bash and need only `git`, `python3` and `blyte` on `PATH`. Any orchestrator
that can write a file, run a command and read JSON can drive them.
