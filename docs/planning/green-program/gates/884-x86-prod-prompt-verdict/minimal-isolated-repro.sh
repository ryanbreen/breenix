#!/usr/bin/env bash
# #884 review F8: minimal isolated repro that a bare `test` failure under
# `set -euo pipefail; set -E; trap ... ERR` reaches the trap handler rather
# than dying silently. Mirrors the three-line shape described in
# X86-PROD-GATE-884-ROUND-2026-09-06.md's "What this round found" section:
# set -euo pipefail; set -E; trap ...; test "$X" -eq 1
set -euo pipefail
set -E
trap 'echo "TRAP_FIRED exit=$? line=$LINENO cmd=[$BASH_COMMAND]"; exit 7' ERR
X=0
test "$X" -eq 1
echo "UNREACHABLE: test should have failed and routed through the ERR trap above"
