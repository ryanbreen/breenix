#!/usr/bin/env bash
# Usage: scripts/ext2-acceptance.sh x86|arm accept|readonly OUTPUT
# Requires built testing/boot_tests kernels, test_binaries.img, ext2 images and x86 OVMF.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export CARGO_BUILD_JOBS=4 BREENIX_QUIET=0
if [ "${1:-}" = x86 ]; then source "$ROOT/scripts/x86-qemu-policy.sh"; fi
source "$ROOT/scripts/host-slots.sh"
host_slots_start "$0" "$@"
host_slot_acquire mac-boot
host_slot_serial "$3/serial_kernel.txt"
if [ "$(uname)" = Darwin ]; then taskpolicy -B -p "$$"; fi
exec python3 "$ROOT/scripts/ext2-acceptance.py" "$@"
