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
git checkout -b feat/e2e-autonomous

# Create synthetic headless driver script
cat << 'DRIVER_EOF' > driver.sh
#!/usr/bin/env bash
set -eu
if [ "${SUPERSTATE_ACTIVE_TASK:-}" = "task-1" ]; then
    echo "Creating a.txt"
    echo "milestone 1 content" > a.txt
elif [ "${SUPERSTATE_ACTIVE_TASK:-}" = "task-2" ]; then
    echo "Creating b.txt"
    echo "milestone 2 content" > b.txt
fi
DRIVER_EOF
chmod +x driver.sh

echo "==> Initializing multi-milestone state..."
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

COMPLETION_JSON="$TMP_DIR/.agent-state/branches/completion.json"
if [ ! -f "$COMPLETION_JSON" ]; then
    echo "Error: completion report not found at $COMPLETION_JSON" >&2
    exit 1
fi

echo "==> Completion JSON report:"
cat "$COMPLETION_JSON"

echo "==> Inspecting tail of daemon execution logs..."
"$BIN" state logs --state "$STATE_FILE" --tail 15

echo "==> E2E Detached Handoff & Autonomous Daemon verification passed successfully!"
