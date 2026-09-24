#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$REPO_ROOT/target/debug/blyte"

echo "==> Building latest blyte binary..."
cargo build -p blyte

if [ ! -x "$BIN" ]; then
    echo "Error: binary not found at $BIN" >&2
    exit 1
fi

TMP_DIR="$(mktemp -d /tmp/blyte_fleet_XXXXXX)"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

echo "==> Setting up test repo at $TMP_DIR..."
cd "$TMP_DIR"
git init -b main
git config user.name "Fleet Tester"
git config user.email "fleet-tester@blyte.local"
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
sleep 1.0
echo "beta data" > beta.txt
DRIVER_B_EOF
chmod +x driver_beta.sh

# ==============================================================================
# Step 1: Initialize branch states for feat/fleet-alpha and feat/fleet-beta
# ==============================================================================
echo "==> Initializing branch feat/fleet-alpha..."
git checkout -b feat/fleet-alpha main
"$BIN" state init \
  --milestones '[{"title":"Create alpha.txt","verify_cmd":"test -f alpha.txt"}]' \
  --worktree

STATE_ALPHA="$TMP_DIR/.agent-state/branches/feat-fleet-alpha.json"
if [ ! -f "$STATE_ALPHA" ]; then
    echo "Error: expected alpha state file at $STATE_ALPHA" >&2
    exit 1
fi

echo "==> Initializing branch feat/fleet-beta..."
git checkout -b feat/fleet-beta main
"$BIN" state init \
  --milestones '[{"title":"Create beta.txt","verify_cmd":"test -f beta.txt"}]' \
  --worktree

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
# Step 3: Run 'blyte state daemon list' and assert active fleet status
# ==============================================================================
echo "==> Inspecting multi-daemon fleet via 'blyte state daemon list'..."
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
echo "==> Verifying true branch isolation and dual commits on isolated branches..."
ALPHA_LOG="$(git log feat/fleet-alpha --oneline)"
echo "Alpha branch git log:"
echo "$ALPHA_LOG"

if ! echo "$ALPHA_LOG" | grep -q "Create alpha.txt"; then
    echo "Error: expected Alpha commit ('Create alpha.txt') on feat/fleet-alpha" >&2
    exit 1
fi

if echo "$ALPHA_LOG" | grep -q "Create beta.txt"; then
    echo "Error: Beta commit leaked onto feat/fleet-alpha!" >&2
    exit 1
fi

BETA_LOG="$(git log feat/fleet-beta --oneline)"
echo "Beta branch git log:"
echo "$BETA_LOG"

if ! echo "$BETA_LOG" | grep -q "Create beta.txt"; then
    echo "Error: expected Beta commit ('Create beta.txt') on feat/fleet-beta" >&2
    exit 1
fi

if echo "$BETA_LOG" | grep -q "Create alpha.txt"; then
    echo "Error: Alpha commit leaked onto feat/fleet-beta!" >&2
    exit 1
fi

echo "==> Verifying tracked files on isolated branches..."
if ! git show feat/fleet-alpha:alpha.txt >/dev/null 2>&1; then
    echo "Error: alpha.txt not found on feat/fleet-alpha" >&2
    exit 1
fi
if git show feat/fleet-alpha:beta.txt >/dev/null 2>&1; then
    echo "Error: beta.txt found on feat/fleet-alpha (branch pollution)!" >&2
    exit 1
fi

if ! git show feat/fleet-beta:beta.txt >/dev/null 2>&1; then
    echo "Error: beta.txt not found on feat/fleet-beta" >&2
    exit 1
fi
if git show feat/fleet-beta:alpha.txt >/dev/null 2>&1; then
    echo "Error: alpha.txt found on feat/fleet-beta (branch pollution)!" >&2
    exit 1
fi

echo "==> Verifying discrete completion reports..."
ALPHA_COMPLETION="$TMP_DIR/.agent-state/branches/completion-feat-fleet-alpha.json"
if [ ! -f "$ALPHA_COMPLETION" ]; then
    echo "Error: Alpha completion report not found at $ALPHA_COMPLETION" >&2
    exit 1
fi

BETA_COMPLETION="$TMP_DIR/.agent-state/branches/completion-feat-fleet-beta.json"
if [ ! -f "$BETA_COMPLETION" ]; then
    echo "Error: Beta completion report not found at $BETA_COMPLETION" >&2
    exit 1
fi

if [ -f "$TMP_DIR/.agent-state/branches/completion.json" ]; then
    echo "Error: Unscoped completion.json must NOT exist when running branch fleet daemons" >&2
    exit 1
fi

echo "==> Alpha completion report:"
cat "$ALPHA_COMPLETION"
if ! grep -q '"base_branch": "feat/fleet-alpha"' "$ALPHA_COMPLETION"; then
    echo "Error: Alpha completion report missing base branch feat/fleet-alpha" >&2
    exit 1
fi

echo "==> Beta completion report:"
cat "$BETA_COMPLETION"
if ! grep -q '"base_branch": "feat/fleet-beta"' "$BETA_COMPLETION"; then
    echo "Error: Beta completion report missing base branch feat/fleet-beta" >&2
    exit 1
fi

echo "==> Verifying worktree directories were cleaned up..."
if [ -d "$TMP_DIR/.wt/feat-fleet-alpha-task-1" ]; then
    echo "Error: Alpha worktree .wt/feat-fleet-alpha-task-1 was not cleaned up" >&2
    exit 1
fi

if [ -d "$TMP_DIR/.wt/feat-fleet-beta-task-1" ]; then
    echo "Error: Beta worktree .wt/feat-fleet-beta-task-1 was not cleaned up" >&2
    exit 1
fi

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
