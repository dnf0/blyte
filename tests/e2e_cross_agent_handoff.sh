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

TMP_DIR="$(mktemp -d /tmp/blyte_cross_agent_XXXXXX)"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

echo "==> Setting up test repo at $TMP_DIR..."
cd "$TMP_DIR"
git init -b main
git config user.name "Cross Agent Tester"
git config user.email "cross-agent@blyte.local"
git commit --allow-empty --no-gpg-sign -m "chore: initial commit"

# ==============================================================================
# Phase 1: Claude Persona initiates cross-agent handoff to Agy
# ==============================================================================
echo "==> Phase 1: Claude persona creates handoff for Agy on feat/cross-agent-pipeline..."
git checkout -b feat/cross-agent-pipeline

CLAUDE_PROJECT_ROOT="$TMP_DIR" "$BIN" state handoff \
  --from claude \
  --to agy \
  --goal "Build cross-agent pipeline" \
  --verify-cmd "test -f beta.txt" \
  --no-worktree

HANDOFF_FILE="$TMP_DIR/.agent-state/handoff.md"
if [ ! -f "$HANDOFF_FILE" ]; then
    echo "Error: expected handoff file at $HANDOFF_FILE" >&2
    exit 1
fi

echo "==> Phase 2: Verifying handoff.md metadata and scoped package..."
if ! grep -q -i "Source Agent.*claude" "$HANDOFF_FILE"; then
    echo "Error: Source Agent: claude not found in $HANDOFF_FILE" >&2
    exit 1
fi

if ! grep -q -i "Target Agent.*agy" "$HANDOFF_FILE"; then
    echo "Error: Target Agent: agy not found in $HANDOFF_FILE" >&2
    exit 1
fi

if ! grep -q "agy" "$HANDOFF_FILE"; then
    echo "Error: agy resumption command not found in $HANDOFF_FILE" >&2
    exit 1
fi

SLUG="feat-cross-agent-pipeline"
SCOPED_FILE="$TMP_DIR/.agent-state/handoffs/${SLUG}.md"
if [ ! -f "$SCOPED_FILE" ]; then
    echo "Error: expected scoped handoff file at $SCOPED_FILE" >&2
    exit 1
fi

if ! grep -q -i "Source Agent.*claude" "$SCOPED_FILE"; then
    echo "Error: Source Agent: claude not found in scoped file $SCOPED_FILE" >&2
    exit 1
fi

if ! grep -q -i "Target Agent.*agy" "$SCOPED_FILE"; then
    echo "Error: Target Agent: agy not found in scoped file $SCOPED_FILE" >&2
    exit 1
fi

# ==============================================================================
# Phase 3: Agy Persona resumes execution via state resume
# ==============================================================================
echo "==> Phase 3: Agy persona resumes execution via state resume..."
RESUME_OUTPUT="$(AGY_VERSION="2.0.0" "$BIN" state resume)"
echo "$RESUME_OUTPUT"

if ! echo "$RESUME_OUTPUT" | grep -q "Build cross-agent pipeline"; then
    echo "Error: resume briefing did not mention goal" >&2
    exit 1
fi

if ! echo "$RESUME_OUTPUT" | grep -q -i "handoff"; then
    echo "Error: resume briefing did not capture handoff note" >&2
    exit 1
fi

# ==============================================================================
# Phase 4: Agy Persona implements deliverable and advances state
# ==============================================================================
echo "==> Phase 4: Agy implements beta.txt and advances state..."
echo "done" > beta.txt
git add beta.txt
git commit --no-gpg-sign -m "feat: implement beta capability"

AGY_VERSION="2.0.0" "$BIN" state next --verify-cmd "test -f beta.txt"

# ==============================================================================
# Phase 5: Verification of completion, commit integrity, and verification gates
# ==============================================================================
echo "==> Phase 5: Verifying completed status and commit integrity..."
STATUS_JSON="$("$BIN" state status --json)"
echo "$STATUS_JSON"

if ! echo "$STATUS_JSON" | grep -q '"status": "completed"'; then
    echo "Error: state is not marked completed" >&2
    exit 1
fi

if ! git log -1 --oneline | grep -q "feat: implement beta capability"; then
    echo "Error: git commit not present in log" >&2
    exit 1
fi

if [ ! -f beta.txt ]; then
    echo "Error: beta.txt is missing!" >&2
    exit 1
fi

# Run verification command to confirm gate passes
test -f beta.txt

echo "==> Cross-agent handoff E2E test passed successfully!"
