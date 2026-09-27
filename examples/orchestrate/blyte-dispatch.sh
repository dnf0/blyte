#!/usr/bin/env bash
# Hand one task to blyte in its own git worktree, and print a summary an orchestrator can act on.
#
#   blyte-dispatch.sh <id> <task-file> <gate>
#
#   <id>         a short name for the run, e.g. T1; the worktree is .wt/<id> on branch wip/<id>
#   <task-file>  the task, in plain text or Markdown: what to change, the files involved, how
#                done is checked, and the exact commit message
#   <gate>       the check command; several lines are several steps, all of which must pass
#
# Run it from the repository's main checkout. The worktree starts from $BASE (default HEAD).
# blyte runs with rollback off (a failed gate keeps partial work for review), within
# $MAX_SECONDS (default 3600) and $MAX_TURNS (default 300). Its log and report go to
# .wt/<id>.log and .wt/<id>.report.json. Exit 0 means the gate passed and the run committed
# something; 4 means it passed without committing anything, which is not done.
#
# The model is $BLYTE_MODEL (default: blyte's default). Its API key must be in the environment;
# blyte never passes it to the commands it runs.
set -uo pipefail

[ $# -eq 3 ] || { sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
ID=$1; TASK_FILE=$2; GATE=$3
command -v blyte >/dev/null || { echo "blyte not on PATH" >&2; exit 2; }
[ -f "$TASK_FILE" ] || { echo "task file not found: $TASK_FILE" >&2; exit 2; }

ROOT=$(git rev-parse --show-toplevel) || exit 2
WT=$ROOT/.wt/$ID
mkdir -p "$ROOT/.wt"
grep -qxF ".wt/" "$ROOT/.gitignore" 2>/dev/null || echo "note: add .wt/ to .gitignore" >&2
[ -d "$WT" ] || git -C "$ROOT" worktree add -q -b "wip/$ID" "$WT" "${BASE:-HEAD}" || exit 1
BASE_SHA=$(git -C "$WT" rev-parse HEAD)

PROMPT=$(cat <<P
You are implementing exactly one task. Your working directory is a git worktree on its own branch; work only there.

$(cat "$TASK_FILE")

Rules:
- Stay within the task. If it cannot pass without a small change elsewhere, make the smallest one and say so.
- Never delete, clean or modify anything outside the worktree, and never change the machine to make a check pass. If a check fails because of the environment (disk, network, a missing service), stop and say so.
- Do not push, open pull requests, or read secrets.
- Commit locally before you stop, with the exact commit message the task gives (or a conventional one if it gives none).
- When you believe the task is done and committed, stop calling tools: the harness then runs the check. Before that, run the check's quick steps (formatting, lint, the tests for your change) yourself; the full run happens at the gate.
- Finish with a short report: what you changed, what you ran, and anything that deviated from the task.
P
)

LOG=$ROOT/.wt/$ID.log
REPORT=$ROOT/.wt/$ID.report.json
( cd "$WT" && blyte --gate "$GATE" --rollback=false \
    --max-seconds "${MAX_SECONDS:-3600}" --max-turns "${MAX_TURNS:-300}" \
    ${BLYTE_MODEL:+--model "$BLYTE_MODEL"} --report "$REPORT" -- "$PROMPT" ) > "$LOG" 2>&1
CODE=$?
COMMITS=$(git -C "$WT" rev-list --count "$BASE_SHA"..HEAD)
# A run that commits nothing is not done, even if the gate passed on the untouched tree.
[ "$CODE" -eq 0 ] && [ "$COMMITS" -eq 0 ] && CODE=4

python3 - "$REPORT" "$ID" "$WT" "$COMMITS" "$CODE" <<'PY'
import json, sys
path, rid, wt, commits, code = sys.argv[1:]
try:
    r = json.load(open(path))
except Exception as e:
    r = {"outcome": "error", "error": f"no report ({e.__class__.__name__})"}
print(json.dumps({
    "id": rid, "worktree": wt, "exit": int(code), "commits": int(commits),
    "outcome": r.get("outcome"), "error": r.get("error"),
    "turns": r.get("turns"), "gate_attempts": r.get("gate_attempts"),
    "gate_failures": [f[:300] for f in r.get("gate_failures", [])],
    "cost_usd": r.get("cost_usd"), "seconds": round(r.get("duration_seconds") or 0),
    "sandbox": r.get("sandbox"),
}, indent=2))
PY
exit "$CODE"
