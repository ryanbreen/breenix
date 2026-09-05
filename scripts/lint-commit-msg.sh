#!/bin/bash
#
# lint-commit-msg.sh — thin wrapper around `claim-lint.py --commit-msg` for
# the two calling conventions a commit message actually needs it in:
#
#   1. `git commit -F <file>` workflows: lint the file before you hand it to
#      `-F`, exactly like the round checklist's `--files <pr-body.md>` step
#      lints a PR body before it ships.
#   2. A `.git/hooks/commit-msg` hook: git invokes a commit-msg hook as
#      `<hook> <path-to-msg-file>` and aborts the commit if the hook exits
#      nonzero — this script's exit code IS the hook's exit code. See
#      scripts/hooks/commit-msg for the hook BODY to symlink; this script
#      does not install anything into anyone's .git on its own.
#
# See docs/planning/green-program/claim-linting.md's "`--commit-msg` mode
# (R21)" section for what this catches, why auto-close-keyword is checked
# even inside a fenced/quoted example here, and the incident (commit
# `e6dd14a6`) it exists to catch on the next occurrence.
#
# Usage: scripts/lint-commit-msg.sh <commit-message-file>

set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: $(basename "$0") <commit-message-file>" >&2
    exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "${SCRIPT_DIR}/claim-lint.py" --commit-msg "$1"
