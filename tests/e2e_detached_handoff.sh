#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$REPO_ROOT/target/debug/superstate"

echo "==> Building latest superstate binary..."
cargo build -p superstate

if [ ! -x "$BIN" ]; then
    echo "Error: binary not found at $BIN" >&2
    exit 1
fi

TMP_DIR="$(mktemp -d /tmp/superstate_e2e_XXXXXX)"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

echo "==> Setting up test repo at $TMP_DIR..."
cd "$TMP_DIR"
git init -b main
git config user.name "E2E Tester"
git config user.email "tester@superstate.local"
git commit --allow-empty --no-gpg-sign -m "chore: initial commit"

# ==============================================================================
# Part 1: Autonomous In-Place Execution, Stdin Delivery & Auto-Checkpointing
# ==============================================================================
echo "==> Part 1: Autonomous In-Place Execution on feat/e2e-autonomous..."
git checkout -b feat/e2e-autonomous

# Synthetic driver that captures stdin delivery and creates milestone files
cat << 'DRIVER_EOF' > driver.sh
#!/usr/bin/env bash
set -eu

# Capture prompt delivered via standard input
STDIN_INPUT="$(cat)"
echo "$STDIN_INPUT" > "stdin_${SUPERSTATE_ACTIVE_TASK:-unknown}.txt"

if [ "${SUPERSTATE_ACTIVE_TASK:-}" = "task-1" ]; then
    echo "Creating a.txt"
    echo "milestone 1 content" > a.txt
elif [ "${SUPERSTATE_ACTIVE_TASK:-}" = "task-2" ]; then
    echo "Creating b.txt"
    echo "milestone 2 content" > b.txt
fi
DRIVER_EOF
chmod +x driver.sh

echo "==> Initializing multi-milestone in-place state..."
"$BIN" state init \
  --milestones '[{"title":"Create a.txt","verify_cmd":"test -f a.txt"},{"title":"Create b.txt","verify_cmd":"test -f b.txt"}]' \
  --no-worktree

STATE_FILE="$TMP_DIR/.agent-state/branches/feat-e2e-autonomous.json"
if [ ! -f "$STATE_FILE" ]; then
    echo "Error: expected state file at $STATE_FILE" >&2
    exit 1
fi

echo "==> Launching detached daemon runner..."
"$BIN" state daemon \
  --state "$STATE_FILE" \
  --driver "$TMP_DIR/driver.sh" \
  --detach

echo "==> Interrogating daemon status..."
STATUS_OUT="$("$BIN" state status --state "$STATE_FILE")"
echo "$STATUS_OUT"

echo "==> Polling for daemon completion (up to 10 seconds)..."
COMPLETED=0
for i in {1..20}; do
    JSON_STATUS="$("$BIN" state status --state "$STATE_FILE" --json || true)"
    if echo "$JSON_STATUS" | grep -q '"status": "completed"'; then
        COMPLETED=1
        break
    fi
    sleep 0.5
done

if [ "$COMPLETED" -ne 1 ]; then
    echo "Error: daemon failed to complete milestones within timeout" >&2
    echo "Daemon logs:"
    "$BIN" state logs --state "$STATE_FILE" || true
    exit 1
fi

echo "==> Verifying completion artifacts..."
if [ ! -f a.txt ]; then
    echo "Error: a.txt not created!" >&2
    exit 1
fi

if [ ! -f b.txt ]; then
    echo "Error: b.txt not created!" >&2
    exit 1
fi

echo "==> Verifying driver stdin reception..."
if [ ! -f stdin_task-1.txt ] || [ ! -f stdin_task-2.txt ]; then
    echo "Error: driver stdin capture files missing" >&2
    exit 1
fi
if ! grep -q "Implementer Subagent working on \*\*task-1" stdin_task-1.txt; then
    echo "Error: stdin for task-1 did not contain expected turnkey prompt header" >&2
    exit 1
fi
if ! grep -q "Implementer Subagent working on \*\*task-2" stdin_task-2.txt; then
    echo "Error: stdin for task-2 did not contain expected turnkey prompt header" >&2
    exit 1
fi

echo "==> Verifying prompt file existence in .agent-state/prompts/..."
PROMPT_FILE="$TMP_DIR/.agent-state/prompts/feat-e2e-autonomous.md"
if [ ! -f "$PROMPT_FILE" ]; then
    echo "Error: expected prompt file at $PROMPT_FILE" >&2
    exit 1
fi
if ! grep -q "Task Specification" "$PROMPT_FILE"; then
    echo "Error: prompt file missing task specification content" >&2
    exit 1
fi

echo "==> Verifying auto-checkpoint commits..."
GIT_LOG="$(git log --oneline)"
echo "$GIT_LOG"
if ! echo "$GIT_LOG" | grep -q "chore(superstate): checkpoint completed milestone task-1: Create a.txt"; then
    echo "Error: expected checkpoint commit for task-1 not found in git log" >&2
    exit 1
fi
if ! echo "$GIT_LOG" | grep -q "chore(superstate): checkpoint completed milestone task-2: Create b.txt"; then
    echo "Error: expected checkpoint commit for task-2 not found in git log" >&2
    exit 1
fi

COMPLETION_JSON="$TMP_DIR/.agent-state/branches/completion-feat-e2e-autonomous.json"
if [ ! -f "$COMPLETION_JSON" ]; then
    echo "Error: scoped completion report not found at $COMPLETION_JSON" >&2
    exit 1
fi
LEGACY_COMPLETION_JSON="$TMP_DIR/.agent-state/branches/completion.json"
if [ -f "$LEGACY_COMPLETION_JSON" ]; then
    echo "Error: completion.json must NOT be created for branch-scoped daemons" >&2
    exit 1
fi

echo "==> Completion JSON report:"
cat "$COMPLETION_JSON"

echo "==> Verifying PID file cleanup for in-place daemon..."
PID_FILE="$TMP_DIR/.agent-state/daemons/feat-e2e-autonomous.pid"
if [ -f "$PID_FILE" ]; then
    echo "Error: PID file $PID_FILE should have been cleaned up after completion" >&2
    exit 1
fi

# ==============================================================================
# Part 2: Autonomous Worktree Lifecycle Execution under DaemonRunner
# ==============================================================================
echo "==> Part 2: Worktree Lifecycle under DaemonRunner on feat/e2e-worktree..."
git checkout -b feat/e2e-worktree main

cat << 'WT_DRIVER_EOF' > wt_driver.sh
#!/usr/bin/env bash
set -eu
if [ "${SUPERSTATE_ACTIVE_TASK:-}" = "task-1" ]; then
    echo "wt 1 content" > wt_a.txt
elif [ "${SUPERSTATE_ACTIVE_TASK:-}" = "task-2" ]; then
    echo "wt 2 content" > wt_b.txt
fi
WT_DRIVER_EOF
chmod +x wt_driver.sh

echo "==> Initializing state with worktree isolation..."
"$BIN" state init \
  --milestones '[{"title":"Create wt_a.txt","verify_cmd":"test -f wt_a.txt"},{"title":"Create wt_b.txt","verify_cmd":"test -f wt_b.txt"}]' \
  --worktree

WT_STATE_FILE="$TMP_DIR/.agent-state/branches/feat-e2e-worktree.json"
if [ ! -f "$WT_STATE_FILE" ]; then
    echo "Error: expected state file at $WT_STATE_FILE" >&2
    exit 1
fi

echo "==> Verifying initial worktree directory creation..."
INITIAL_WT="$TMP_DIR/.wt/feat-e2e-worktree-task-1"
if [ ! -d "$INITIAL_WT" ]; then
    echo "Error: expected provisioned worktree at $INITIAL_WT" >&2
    exit 1
fi

echo "==> Launching detached daemon runner in worktree mode..."
"$BIN" state daemon \
  --state "$WT_STATE_FILE" \
  --driver "$TMP_DIR/wt_driver.sh" \
  --detach

echo "==> Polling worktree daemon completion..."
WT_COMPLETED=0
for i in {1..20}; do
    JSON_STATUS="$("$BIN" state status --state "$WT_STATE_FILE" --json || true)"
    if echo "$JSON_STATUS" | grep -q '"status": "completed"'; then
        WT_COMPLETED=1
        break
    fi
    sleep 0.5
done

if [ "$WT_COMPLETED" -ne 1 ]; then
    echo "Error: worktree daemon failed to complete milestones within timeout" >&2
    echo "Daemon logs:"
    "$BIN" state logs --state "$WT_STATE_FILE" || true
    exit 1
fi

echo "==> Verifying worktree completion artifacts merged to base branch..."
if [ ! -f wt_a.txt ]; then
    echo "Error: wt_a.txt not merged to base branch!" >&2
    exit 1
fi
if [ ! -f wt_b.txt ]; then
    echo "Error: wt_b.txt not merged to base branch!" >&2
    exit 1
fi

echo "==> Verifying worktree commits in git log..."
WT_GIT_LOG="$(git log -n 5 --oneline)"
echo "$WT_GIT_LOG"
if ! echo "$WT_GIT_LOG" | grep -q "feat(task-1): Create wt_a.txt"; then
    echo "Error: expected worktree merge commit for task-1" >&2
    exit 1
fi
if ! echo "$WT_GIT_LOG" | grep -q "feat(task-2): Create wt_b.txt"; then
    echo "Error: expected worktree merge commit for task-2" >&2
    exit 1
fi

echo "==> Verifying worktrees were cleaned up..."
if [ -d "$INITIAL_WT" ]; then
    echo "Error: worktree directory $INITIAL_WT was not cleaned up!" >&2
    exit 1
fi
NEXT_WT="$TMP_DIR/.wt/feat-e2e-worktree-task-2"
if [ -d "$NEXT_WT" ]; then
    echo "Error: worktree directory $NEXT_WT was not cleaned up!" >&2
    exit 1
fi

echo "==> Verifying PID file cleanup for worktree daemon..."
WT_PID_FILE="$TMP_DIR/.agent-state/daemons/feat-e2e-worktree.pid"
if [ -f "$WT_PID_FILE" ]; then
    echo "Error: PID file $WT_PID_FILE should have been cleaned up after completion" >&2
    exit 1
fi

echo "==> Verifying worktree completion report..."
WT_COMPLETION_JSON="$TMP_DIR/.agent-state/branches/completion-feat-e2e-worktree.json"
if [ ! -f "$WT_COMPLETION_JSON" ]; then
    echo "Error: worktree completion report not found at $WT_COMPLETION_JSON" >&2
    exit 1
fi

echo "==> E2E Detached Handoff, Autonomous Daemon & Worktree Lifecycle verification passed successfully!"
