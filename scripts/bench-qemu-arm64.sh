#!/bin/bash
# The ARM64 QEMU performance rig (DEV-31): Breenix and stock Linux in one VM shape.
#
#   scripts/bench-qemu-arm64.sh run --run ID [--count N] [--cases a,b] [--headed]
#   scripts/bench-qemu-arm64.sh provision-linux
#
# Holds the Mac's boot slot and the host-wide ARM64 QEMU lock for the whole run, so
# no other lane's VM shares the host while samples are taken, then hands its
# arguments to userland/bbench/rig.py. See userland/bbench/RIG.md for the inputs.
BREENIX_HOST_ENTRY="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
BREENIX_HOST_ARGS=("$@")
set -e
BREENIX_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../docker/qemu/lib/qemu-host-lock.sh
source "$BREENIX_ROOT/docker/qemu/lib/qemu-host-lock.sh"

case "${1:-}" in
    fetch-linux) exec python3 "$BREENIX_ROOT/userland/bbench/rig.py" "$@" ;;
    run|provision-linux) ;;
    *) echo "usage: $0 run --run ID [...] | provision-linux | fetch-linux" >&2; exit 2 ;;
esac

host_slot_acquire mac-boot
qemu_host_lock_acquire
host_slot_quiet
# Backgrounded and tracked, as in run-arm64-qemu.sh: a signal to this script stops
# the rig (which stops the VMs it started) before the lock is released.
python3 "$BREENIX_ROOT/userland/bbench/rig.py" "$@" &
RIG_PID=$!
qemu_host_lock_track_pid "$RIG_PID"
wait "$RIG_PID"
