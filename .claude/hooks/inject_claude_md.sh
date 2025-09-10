#!/usr/bin/env bash
set -euo pipefail

proj="${CLAUDE_PROJECT_DIR:-$PWD}"
file="$proj/CLAUDE.md"

[ -f "$file" ] || exit 0

printf "\n### INSTRUCTIONS (from %s)\n\n" "$file"
cat "$file"
printf "\n"
