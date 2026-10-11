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
# Just before every boot starts its VM, with the manifests of the suites it runs (none for
# other modes): records the boot's start, after waiting for a quiet host when a manifest is
# timingSensitive. BREENIX_QUIET* in the environment override the policy (host-slots.py).
host_slot_quiet() { python3 "$HOST_SLOTS_HELPER" quiet "$@"; }
host_slot_vm() { python3 "$HOST_SLOTS_HELPER" vm "$@"; }
host_slot_serial() { python3 "$HOST_SLOTS_HELPER" serial "$1"; }
host_slot_header() { python3 "$HOST_SLOTS_HELPER" header "$1"; }
