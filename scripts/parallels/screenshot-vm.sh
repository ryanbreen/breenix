#!/bin/bash
# Non-interactive screenshot of a Parallels VM window (works even when offscreen).
# Usage: ./screenshot-vm.sh [vm-name-substring] [output-path]
# Defaults: vm-name=breenix, output=/tmp/breenix-screenshot.png

set -euo pipefail

VM_SUBSTR="${1:-breenix}"
OUTPUT="${2:-/tmp/breenix-screenshot.png}"

# Get window ID via Quartz CGWindowList — include ALL windows (not just on-screen)
WINDOW_ID=$(python3 -c "
import Quartz
windows = Quartz.CGWindowListCopyWindowInfo(
    Quartz.kCGWindowListOptionAll,
    Quartz.kCGNullWindowID
)
for w in windows:
    owner = w.get('kCGWindowOwnerName', '')
    name = w.get('kCGWindowName', '')
    wid = w.get('kCGWindowNumber', 0)
    if 'Parallels' in owner and '${VM_SUBSTR}' in name.lower():
        print(wid)
        break
" 2>/dev/null)

if [ -z "$WINDOW_ID" ]; then
    echo "ERROR: No Parallels window found matching '$VM_SUBSTR'"
    exit 1
fi

screencapture -x -o -l"$WINDOW_ID" "$OUTPUT"
echo "$OUTPUT"
