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

TMP_DIR="$(mktemp -d /tmp/superstate_fleet_XXXXXX)"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

echo "==> Setting up test repo at $TMP_DIR..."
cd "$TMP_DIR"
git init -b main
git config user.name "Fleet Tester"
git config user.email "fleet-tester@superstate.local"
echo "initial baseline" > README.md
git add README.md
git commit --no-gpg-sign -m "chore: initial baseline"

# Create driver scripts with slight stagger to ensure concurrent execution overlap
cat << 'DRIVER_A_EOF' > driver_alpha.sh
#!/usr/bin/env bash
set -eu
sleep 0.5
echo "alpha data" > alpha.txt
DRIVER_A_EOF
chmod +x driver_alpha.sh

cat << 'DRIVER_B_EOF' > driver_beta.sh
#!/usr/bin/env bash
set -eu
sleep 0.8
echo "beta data" > beta.txt
DRIVER_B_EOF
chmod +x driver_beta.sh

# ==============================================================================
# Step 1: Initialize branch states for feat/fleet-alpha and feat/fleet-beta
# ==============================================================================
echo "==> Initializing branch feat/fleet-alpha..."
git checkout -b feat/fleet-alpha
"$BIN" state init \
  --milestones '[{"title":"Fleet Alpha Milestone","verify_cmd":"test -f alpha.txt"}]' \
  --no-worktree

STATE_ALPHA="$TMP_DIR/.agent-state/branches/feat-fleet-alpha.json"
if [ ! -f "$STATE_ALPHA" ]; then
    echo "Error: expected alpha state file at $STATE_ALPHA" >&2
    exit 1
fi

echo "==> Initializing branch feat/fleet-beta..."
git checkout -b feat/fleet-beta
"$BIN" state init \
  --milestones '[{"title":"Fleet Beta Milestone","verify_cmd":"test -f beta.txt"}]' \
  --no-worktree

STATE_BETA="$TMP_DIR/.agent-state/branches/feat-fleet-beta.json"
if [ ! -f "$STATE_BETA" ]; then
    echo "Error: expected beta state file at $STATE_BETA" >&2
    exit 1
fi

# ==============================================================================
# Step 2: Spawn detached daemons concurrently on both branches
# ==============================================================================
echo "==> Spawning detached daemon for feat/fleet-alpha..."
"$BIN" state daemon \
  --state "$STATE_ALPHA" \
  --driver "$TMP_DIR/driver_alpha.sh" \
  --detach

echo "==> Spawning detached daemon for feat/fleet-beta..."
"$BIN" state daemon \
  --state "$STATE_BETA" \
  --driver "$TMP_DIR/driver_beta.sh" \
  --detach

# ==============================================================================
# Step 3: Run 'superstate state daemon list' and assert active fleet status
# ==============================================================================
echo "==> Inspecting multi-daemon fleet via 'superstate state daemon list'..."
DAEMON_LIST="$("$BIN" state daemon list)"
echo "$DAEMON_LIST"

if ! echo "$DAEMON_LIST" | grep -q "feat-fleet-alpha"; then
    echo "Error: feat-fleet-alpha not found in daemon list" >&2
    exit 1
fi

if ! echo "$DAEMON_LIST" | grep -q "feat-fleet-beta"; then
    echo "Error: feat-fleet-beta not found in daemon list" >&2
    exit 1
fi

ALPHA_PID_FILE="$TMP_DIR/.agent-state/daemons/feat-fleet-alpha.pid"
BETA_PID_FILE="$TMP_DIR/.agent-state/daemons/feat-fleet-beta.pid"

if [ ! -f "$ALPHA_PID_FILE" ] || [ ! -f "$BETA_PID_FILE" ]; then
    echo "Error: daemon PID files missing" >&2
    exit 1
fi

ALPHA_PID="$(cat "$ALPHA_PID_FILE" | tr -d '[:space:]')"
BETA_PID="$(cat "$BETA_PID_FILE" | tr -d '[:space:]')"

echo "==> Detected Alpha PID: $ALPHA_PID, Beta PID: $BETA_PID"

if [ "$ALPHA_PID" = "$BETA_PID" ]; then
    echo "Error: daemon PIDs must be distinct for concurrent fleet jobs" >&2
    exit 1
fi

if ! echo "$DAEMON_LIST" | grep -q "$ALPHA_PID"; then
    echo "Error: Alpha PID $ALPHA_PID not visible in daemon list" >&2
    exit 1
fi

if ! echo "$DAEMON_LIST" | grep -q "$BETA_PID"; then
    echo "Error: Beta PID $BETA_PID not visible in daemon list" >&2
    exit 1
fi

# Interrogate machine-readable JSON daemon fleet listing
DAEMON_JSON="$("$BIN" state daemon list --json)"
echo "==> Machine-readable fleet JSON:"
echo "$DAEMON_JSON"

if ! echo "$DAEMON_JSON" | grep -q '"slug": "feat-fleet-alpha"'; then
    echo "Error: JSON daemon list missing feat-fleet-alpha entry" >&2
    exit 1
fi
if ! echo "$DAEMON_JSON" | grep -q '"slug": "feat-fleet-beta"'; then
    echo "Error: JSON daemon list missing feat-fleet-beta entry" >&2
    exit 1
fi

# ==============================================================================
# Step 4: Poll both daemons to completion
# ==============================================================================
echo "==> Polling both concurrent daemons to completion (up to 15 seconds)..."
ALPHA_COMPLETED=0
BETA_COMPLETED=0

for i in {1..30}; do
    if [ "$ALPHA_COMPLETED" -eq 0 ]; then
        STATUS_ALPHA="$("$BIN" state status --state "$STATE_ALPHA" --json 2>/dev/null || true)"
        if echo "$STATUS_ALPHA" | grep -q '"status": "completed"'; then
            ALPHA_COMPLETED=1
            echo "  ✓ feat/fleet-alpha daemon reached completion"
        fi
    fi

    if [ "$BETA_COMPLETED" -eq 0 ]; then
        STATUS_BETA="$("$BIN" state status --state "$STATE_BETA" --json 2>/dev/null || true)"
        if echo "$STATUS_BETA" | grep -q '"status": "completed"'; then
            BETA_COMPLETED=1
            echo "  ✓ feat/fleet-beta daemon reached completion"
        fi
    fi

    if [ "$ALPHA_COMPLETED" -eq 1 ] && [ "$BETA_COMPLETED" -eq 1 ]; then
        break
    fi
    sleep 0.5
done

if [ "$ALPHA_COMPLETED" -ne 1 ]; then
    echo "Error: feat/fleet-alpha daemon failed to complete within timeout" >&2
    echo "Alpha daemon logs:"
    "$BIN" state logs --state "$STATE_ALPHA" || true
    exit 1
fi

if [ "$BETA_COMPLETED" -ne 1 ]; then
    echo "Error: feat/fleet-beta daemon failed to complete within timeout" >&2
    echo "Beta daemon logs:"
    "$BIN" state logs --state "$STATE_BETA" || true
    exit 1
fi

# ==============================================================================
# Step 5: Verify branch milestones, commits, completion reports & cleanup
# ==============================================================================
echo "==> Verifying milestone output artifacts..."
if [ ! -f alpha.txt ]; then
    echo "Error: alpha.txt not created by alpha driver" >&2
    exit 1
fi
if [ ! -f beta.txt ]; then
    echo "Error: beta.txt not created by beta driver" >&2
    exit 1
fi

echo "==> Verifying auto-checkpoint commits on git branches..."
GIT_LOG="$(git log -n 5 --oneline)"
echo "$GIT_LOG"

if ! echo "$GIT_LOG" | grep -q "chore(superstate): checkpoint completed milestone task-1: Fleet Alpha Milestone"; then
    echo "Error: expected checkpoint commit for Fleet Alpha Milestone" >&2
    exit 1
fi

if ! echo "$GIT_LOG" | grep -q "chore(superstate): checkpoint completed milestone task-1: Fleet Beta Milestone"; then
    echo "Error: expected checkpoint commit for Fleet Beta Milestone" >&2
    exit 1
fi

echo "==> Verifying completion report generation..."
COMPLETION_JSON="$TMP_DIR/.agent-state/branches/completion.json"
if [ ! -f "$COMPLETION_JSON" ]; then
    echo "Error: completion report not found at $COMPLETION_JSON" >&2
    exit 1
fi

echo "==> Final completion report content:"
cat "$COMPLETION_JSON"

echo "==> Verifying PID file cleanup for both fleet daemons..."
if [ -f "$ALPHA_PID_FILE" ]; then
    echo "Error: Alpha PID file $ALPHA_PID_FILE was not cleaned up after completion" >&2
    exit 1
fi

if [ -f "$BETA_PID_FILE" ]; then
    echo "Error: Beta PID file $BETA_PID_FILE was not cleaned up after completion" >&2
    exit 1
fi

echo "==> Verifying daemon list is clear of terminated fleet jobs..."
CLEARED_LIST="$("$BIN" state daemon list)"
echo "$CLEARED_LIST"

if echo "$CLEARED_LIST" | grep -q "yes"; then
    echo "Error: daemon list still reports alive daemons" >&2
    exit 1
fi

echo "==> Multi-daemon fleet execution & concurrency verification passed successfully!"
