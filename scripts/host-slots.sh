#!/bin/bash
# Source from a launcher, then re-exec under the lease-owning supervisor.
HOST_SLOTS_HELPER="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/host-slots.py"
host_slots_start() {
    if [ -z "${BREENIX_SLOT_SESSION:-}" ]; then
        exec python3 "$HOST_SLOTS_HELPER" supervise -- "$@"
    fi
}
host_slot_acquire() { python3 "$HOST_SLOTS_HELPER" acquire "$1"; }
host_slot_release() { python3 "$HOST_SLOTS_HELPER" release "$1"; }
host_slot_vm() { python3 "$HOST_SLOTS_HELPER" vm "$@"; }
host_slot_serial() { python3 "$HOST_SLOTS_HELPER" serial "$1"; }
host_slot_header() { python3 "$HOST_SLOTS_HELPER" header "$1"; }
