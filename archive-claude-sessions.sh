#!/usr/bin/env bash
# Archive old Claude Code sessions for the breenix project.
# Keeps the 5 most recent sessions, zips the rest at max compression
# into ~/Downloads/claude_archive/, then removes the originals.

set -euo pipefail

SESSION_DIR="$HOME/.claude/projects/-Users-wrb-fun-code-breenix"
ARCHIVE_DIR="$HOME/Downloads/claude_archive"
KEEP_COUNT=5

if [ ! -d "$SESSION_DIR" ]; then
  echo "Session directory not found: $SESSION_DIR"
  exit 1
fi

mkdir -p "$ARCHIVE_DIR"

# Get all top-level JSONL files sorted by modification time (newest first).
# Extract just the UUID stem from each.
ALL_UUIDS=()
while IFS= read -r f; do
  ALL_UUIDS+=("$(basename "$f" .jsonl)")
done < <(ls -t "$SESSION_DIR"/*.jsonl 2>/dev/null)

TOTAL=${#ALL_UUIDS[@]}
if [ "$TOTAL" -le "$KEEP_COUNT" ]; then
  echo "Only $TOTAL session(s) found — nothing to archive."
  exit 0
fi

# The first KEEP_COUNT entries are the ones we keep; the rest get archived.
OLD_UUIDS=("${ALL_UUIDS[@]:$KEEP_COUNT}")
echo "Total sessions: $TOTAL"
echo "Keeping: $KEEP_COUNT most recent"
echo "Archiving: ${#OLD_UUIDS[@]} old sessions"

# Stage old files into a temp directory
STAGING=$(mktemp -d)
trap 'rm -rf "$STAGING"' EXIT

for uuid in "${OLD_UUIDS[@]}"; do
  # Copy the JSONL file
  cp "$SESSION_DIR/$uuid.jsonl" "$STAGING/" 2>/dev/null || true
  # Copy the subagent directory if present
  if [ -d "$SESSION_DIR/$uuid" ]; then
    cp -r "$SESSION_DIR/$uuid" "$STAGING/"
  fi
done

# Zip at maximum compression
ARCHIVE_NAME="breenix-claude-sessions-$(date +%Y%m%d-%H%M%S).zip"
ARCHIVE_PATH="$ARCHIVE_DIR/$ARCHIVE_NAME"
(cd "$STAGING" && zip -9 -r "$ARCHIVE_PATH" .)
echo ""
echo "Archive created: $ARCHIVE_PATH"
ls -lh "$ARCHIVE_PATH"

# Remove originals
for uuid in "${OLD_UUIDS[@]}"; do
  rm -f "$SESSION_DIR/$uuid.jsonl"
  rm -rf "$SESSION_DIR/$uuid"
done

echo ""
echo "Done. Removed ${#OLD_UUIDS[@]} old sessions from $SESSION_DIR"
