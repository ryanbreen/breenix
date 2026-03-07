#!/bin/bash
# Start a Parallels VM, wait for boot, and screenshot the VM window.
# Usage: ./start-and-screenshot.sh <vm-name> [wait-seconds] [output-path]
#
# Defaults: wait=30s, output=/tmp/breenix-screenshot.png

set -euo pipefail

VM_NAME="${1:?Usage: $0 <vm-name> [wait-seconds] [output-path]}"
WAIT_SECS="${2:-30}"
OUTPUT="${3:-/tmp/breenix-screenshot.png}"

echo "Starting VM: $VM_NAME"

# Start VM via AppleScript (avoids prlctl start hanging on dialogs)
osascript -e "
tell application \"Parallels Desktop\"
    set vmList to every virtual machine
    repeat with vm in vmList
        if name of vm is \"$VM_NAME\" then
            start vm
            return \"started\"
        end if
    end repeat
    return \"not found\"
end tell
" 2>&1

echo "Waiting ${WAIT_SECS}s for boot..."
sleep "$WAIT_SECS"

# Screenshot the VM window
echo "Taking screenshot..."
osascript -e "
tell application \"Parallels Desktop\"
    activate
end tell
delay 1
tell application \"System Events\"
    tell process \"Parallels Desktop\"
        set frontmost to true
        -- Find the VM window
        set vmWindow to missing value
        repeat with w in windows
            if name of w contains \"$VM_NAME\" then
                set vmWindow to w
                exit repeat
            end if
        end repeat
        if vmWindow is not missing value then
            set {x, y} to position of vmWindow
            set {w, h} to size of vmWindow
            do shell script \"screencapture -R\" & x & \",\" & y & \",\" & w & \",\" & h & \" $OUTPUT\"
        else
            -- Fallback: screenshot the frontmost window
            do shell script \"screencapture -l\$(osascript -e 'tell app \\\"Parallels Desktop\\\" to id of front window') $OUTPUT 2>/dev/null || screencapture -w $OUTPUT\"
        end if
    end tell
end tell
" 2>&1 || {
    # Fallback: just use screencapture on the frontmost window
    echo "AppleScript window capture failed, using screencapture -l fallback..."
    # Get window ID of Parallels
    WINDOW_ID=$(osascript -e 'tell app "Parallels Desktop" to activate' -e 'delay 0.5' -e 'tell app "System Events" to tell process "Parallels Desktop" to set frontmost to true' 2>/dev/null || true)
    screencapture -x -o "$OUTPUT" 2>/dev/null || true
}

if [ -f "$OUTPUT" ]; then
    echo "Screenshot saved: $OUTPUT"
else
    echo "WARNING: Screenshot may have failed"
fi
