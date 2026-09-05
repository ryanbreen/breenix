#!/bin/bash
#
# commit-msg.sh — the BODY of a git commit-msg hook. Named with a `.sh`
# extension (review m4) so claim-lint's own diff-mode extension allowlist
# does not SKIP this file the way it skipped the original extensionless
# `scripts/hooks/commit-msg` -- git itself does not care about the source
# file's name, only the INSTALLED symlink's name, which still has to be
# exactly `commit-msg`. This file is NOT installed into anyone's
# `.git/hooks/` automatically by anything in this repo; a `.git` directory
# is per-checkout and per-person, and no script here writes into it
# uninvited. To opt in, symlink it from the repo root:
#
#   ln -sf ../../scripts/hooks/commit-msg.sh .git/hooks/commit-msg
#
# (the relative target is `../../scripts/hooks/commit-msg.sh` because
# `.git/hooks/` sits two directories below the repo root).
#
# Git invokes a commit-msg hook as `<hook> <path-to-msg-file>` and aborts the
# commit if it exits nonzero. This body delegates to
# scripts/lint-commit-msg.sh, which is also the standalone entry point for a
# `git commit -F <file>` workflow — one implementation, two call sites. See
# docs/planning/green-program/claim-linting.md's "`--commit-msg` mode (R21)"
# section for what it checks and the incident (commit `e6dd14a6`) it exists
# to catch on the next occurrence.

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
exec "${REPO_ROOT}/scripts/lint-commit-msg.sh" "$1"
