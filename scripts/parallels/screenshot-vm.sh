#!/bin/bash
# Thin compatibility wrapper. Superseded by capture-display.sh (#917: this
# script's own CGWindowList lookup matched on kCGWindowName, which is
# documented empty in 12/12 Parallels-owned windows inventoried in
# docs/planning/green-program/gui/evidence/windowlist-no-vm.txt for a VM
# started via `prlctl start`, and this script had no `prlctl capture`
# fallback of its own, matching the 7/7 identical failures in
# docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/evidence/).
# No longer called from run.sh; kept only so a direct invocation still
# produces a real screenshot instead of that window-title match.
# claim-lint:ok: #917, counts above
#
# Usage: ./screenshot-vm.sh [vm-name-substring] [output-path]
# Defaults: vm-name=breenix, output=/tmp/breenix-screenshot.png

set -euo pipefail

VM_SUBSTR="${1:-breenix}"
OUTPUT="${2:-/tmp/breenix-screenshot.png}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

exec "$SCRIPT_DIR/capture-display.sh" "$VM_SUBSTR" "$OUTPUT"
