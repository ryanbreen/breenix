#!/bin/bash
# Extract saved files from Breenix ext2 session disk
#
# Usage:
#   ./scripts/extract-saves.sh              # Extract to ~/breenix-saves/
#   ./scripts/extract-saves.sh /path/to/dir # Extract to specific directory
#
# Requires: e2fsprogs (auto-installed via Homebrew if missing)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
EXT2_IMG="$BREENIX_ROOT/target/ext2-session.img"
OUTPUT_DIR="${1:-$HOME/breenix-saves}"

if [ ! -f "$EXT2_IMG" ]; then
    echo "Error: No session disk found at $EXT2_IMG"
    echo "Run Breenix first with ./run.sh, then try again."
    exit 1
fi

# Find debugfs (e2fsprogs), auto-installing via Homebrew if needed
find_debugfs() {
    if command -v debugfs &>/dev/null; then
        echo "debugfs"
    elif [ -x /opt/homebrew/opt/e2fsprogs/sbin/debugfs ]; then
        echo "/opt/homebrew/opt/e2fsprogs/sbin/debugfs"
    elif [ -x /usr/local/opt/e2fsprogs/sbin/debugfs ]; then
        echo "/usr/local/opt/e2fsprogs/sbin/debugfs"
    fi
}

DEBUGFS=$(find_debugfs)
if [ -z "$DEBUGFS" ]; then
    if command -v brew &>/dev/null; then
        echo "Installing e2fsprogs via Homebrew..."
        brew install e2fsprogs
        DEBUGFS=$(find_debugfs)
    fi
    if [ -z "$DEBUGFS" ]; then
        echo "Error: debugfs not found and could not be installed."
        echo "  Install Homebrew (https://brew.sh) then re-run, or:"
        echo "  brew install e2fsprogs"
        exit 1
    fi
fi

mkdir -p "$OUTPUT_DIR"

echo "Extracting saves from: $EXT2_IMG"
echo "Output directory: $OUTPUT_DIR"
echo ""

# List files in /home/ on the ext2 image
FILES=$($DEBUGFS -R "ls -l /home" "$EXT2_IMG" 2>/dev/null | grep -o 'guskit_[0-9]*\.bmp' || true)

if [ -z "$FILES" ]; then
    echo "No saved drawings found in /home/"
    exit 0
fi

COUNT=0
for f in $FILES; do
    echo "  Extracting $f..."
    $DEBUGFS -R "dump /home/$f $OUTPUT_DIR/$f" "$EXT2_IMG" 2>/dev/null
    COUNT=$((COUNT + 1))
done

echo ""
echo "Extracted $COUNT file(s) to $OUTPUT_DIR/"
if [ "$(uname)" = "Darwin" ]; then
    echo "Opening in Finder..."
    open "$OUTPUT_DIR"
fi
