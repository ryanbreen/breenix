#!/bin/bash
# Paste macOS clipboard into Breenix QEMU guest.
#
# Usage:
#   ./scripts/clipboard-paste.sh         # Paste clipboard contents
#   ./scripts/clipboard-paste.sh -n      # Paste + press Enter
#   ./scripts/clipboard-paste.sh -p 4444 # Specify monitor port
#
# Bind to a global hotkey via Hammerspoon, Karabiner, or macOS Shortcuts.

pbpaste | "$(dirname "$0")/paste.sh" "$@"
