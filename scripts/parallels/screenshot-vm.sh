#!/bin/bash
# Thin compatibility wrapper. Superseded by capture-display.sh (#917: this
# script's own CGWindowList lookup matched on kCGWindowName, which is
# documented empty in 12/12 Parallels-owned windows inventoried in
# docs/planning/green-program/gui/evidence/windowlist-no-vm.txt for a VM
# started via `prlctl start`, and this script had no `prlctl capture`
# fallback of its own, matching the 6/6 identical failures (of the runs
# that reached this step) in
# docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/evidence/).
# No longer called from run.sh; kept only so a direct invocation still
# produces a real screenshot instead of that window-title match.
# claim-lint:ok: #917, counts above
#
# capture-display.sh takes VM_NAME as an exact `prlctl capture <name>` /
# `--vm-name <name>` match, not a substring (correction: review C-12,
# 2026-09-07) -- the default below is a convenience for the common case
# where a caller passes the real VM name explicitly; it will not itself
# match a live epoch-suffixed name such as `breenix-<timestamp>`.
#
# Usage: ./screenshot-vm.sh [vm-name] [output-path]
# Defaults: vm-name=breenix (rarely a real running VM's exact name),
# output=/tmp/breenix-screenshot.png

set -euo pipefail

VM_SUBSTR="${1:-breenix}"
OUTPUT="${2:-/tmp/breenix-screenshot.png}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

exec "$SCRIPT_DIR/capture-display.sh" "$VM_SUBSTR" "$OUTPUT"
