#!/bin/bash
# VMware Fusion e1000-fallback boot capture (green arc 5, bus+NIC blended,
# leg 4b). See docs/planning/green-program/nic-bus/ for the evidence artifact
# this script produces.
#
# What this proves: on VMware Fusion, run.sh's own VMX generation never
# attaches a virtio-net PCI device (`ethernet0.virtualDev = "e1000e"`,
# unconditional), so `virtio::net_pci::init()` in drivers::init()
# (kernel/src/drivers/mod.rs) deterministically fails to find one and the
# Intel e1000 fallback is the only NIC path that can ever succeed on this
# platform. That fallback has been live and unchanged since e144f93a
# (2026-03-10) but had no recorded boot anywhere in this repo proving it
# still works -- this script's sole purpose is to capture one.
#
# This is a ONE-TIME, operator-run capture, not a repeatable CI gate: it
# never ran on a shared host before (unlike the Parallels
# truncate/boot/read/stop cycle CLAUDE.md documents, which this is modeled
# on line-for-line), it opens a real VMware Fusion VM, and it is not wired
# into any merge gate. Run it by hand when the e1000-fallback evidence needs
# refreshing.
#
# Usage: docker/qemu/run-vmware-gate.sh [boot-wait-seconds]
#   boot-wait-seconds : how long to let the VM run before reading serial and
#                        stopping it (default 90; override via
#                        BREENIX_VMWARE_BOOT_WAIT or the first argument)

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
VMRUN="/Applications/VMware Fusion.app/Contents/Public/vmrun"
SERIAL_LOG="/tmp/breenix-vmware-serial.log"
BOOT_WAIT_SECS="${1:-${BREENIX_VMWARE_BOOT_WAIT:-90}}"
BUILD_WAIT_SECS=900
RUN_LOG="/tmp/breenix-vmware-gate-run.log"

VMX_FILE=""
RUN_SH_PID=""

# Every exit path -- success, an assertion failure, or a build/start timeout
# -- must leave no VM running. This is the ONE cleanup path; every `exit`
# below routes through it via the EXIT trap rather than each caller
# remembering to stop the VM itself.
cleanup_vm() {
    if [ -n "$RUN_SH_PID" ] && kill -0 "$RUN_SH_PID" 2>/dev/null; then
        kill "$RUN_SH_PID" 2>/dev/null || true
        wait "$RUN_SH_PID" 2>/dev/null || true
    fi
    if [ -n "$VMX_FILE" ] && [ -f "$VMX_FILE" ]; then
        echo "[vmware-gate] Stopping VM: $VMX_FILE"
        "$VMRUN" stop "$VMX_FILE" hard >/dev/null 2>&1 || true
    fi
}
trap cleanup_vm EXIT

fail() {
    echo "VMware e1000-fallback gate: FAIL - $1"
    if [ -f "$RUN_LOG" ]; then
        echo "--- run.sh tail ($RUN_LOG) ---"
        tail -n 60 "$RUN_LOG"
    fi
    exit 1
}

if [ ! -x "$VMRUN" ]; then
    fail "vmrun not found at $VMRUN; is VMware Fusion installed?"
fi

echo "[vmware-gate] === Building + starting VM via run.sh --vmware ==="
rm -f "$SERIAL_LOG"
: > "$RUN_LOG"
cd "$BREENIX_ROOT" || fail "repo dir missing: $BREENIX_ROOT"
"$BREENIX_ROOT/run.sh" --vmware >"$RUN_LOG" 2>&1 &
RUN_SH_PID=$!

# run.sh --vmware execs into `tail -f` on the serial log once the VM is up,
# printing the VMX path just before that happens (build time varies with
# cache state, so poll for the line rather than sleeping a fixed window).
found_vmx=false
for _ in $(seq 1 "$BUILD_WAIT_SECS"); do
    line="$(grep -E '^VMX:[[:space:]]+.+\.vmx$' "$RUN_LOG" 2>/dev/null | tail -1)"
    if [ -n "$line" ]; then
        VMX_FILE="${line#VMX:}"
        # Trim leading whitespace left by the fixed-width "VMX:    " prefix.
        VMX_FILE="${VMX_FILE#"${VMX_FILE%%[![:space:]]*}"}"
        found_vmx=true
        break
    fi
    if ! kill -0 "$RUN_SH_PID" 2>/dev/null; then
        break
    fi
    sleep 1
done
if [ "$found_vmx" != true ] || [ -z "$VMX_FILE" ] || [ ! -f "$VMX_FILE" ]; then
    fail "VM never started within ${BUILD_WAIT_SECS}s build+start window"
fi
echo "[vmware-gate] VM started: $VMX_FILE"

echo "[vmware-gate] === Waiting ${BOOT_WAIT_SECS}s for boot ==="
sleep "$BOOT_WAIT_SECS"

echo "[vmware-gate] === Stopping VM and reading evidence ==="
"$VMRUN" stop "$VMX_FILE" hard >/dev/null 2>&1 || true
if kill -0 "$RUN_SH_PID" 2>/dev/null; then
    kill "$RUN_SH_PID" 2>/dev/null || true
    wait "$RUN_SH_PID" 2>/dev/null || true
fi
# The VM is already stopped; clear both so the EXIT trap's cleanup is a no-op
# rather than a redundant (harmless, but noisy) second stop attempt.
VMX_FILE_STOPPED="$VMX_FILE"
VMX_FILE=""
RUN_SH_PID=""

[ -s "$SERIAL_LOG" ] || fail "no serial output captured at $SERIAL_LOG"

# The two lines that prove the fallback actually ran, from
# kernel/src/drivers/mod.rs's Parallels/VMware PCI branch:
#   match virtio::net_pci::init() {
#       Ok(())  => "[drivers] VirtIO network (PCI) initialized",
#       Err(e)  => "[drivers] VirtIO network (PCI) init failed: {e}", then
#                  falls through to e1000::init().
NET_PCI_ATTEMPT=$(grep -h -c -F '[drivers] VirtIO network (PCI) init failed' "$SERIAL_LOG" 2>/dev/null || true)
E1000_SUCCESS=$(grep -h -c -F '[drivers] Intel e1000 network driver initialized' "$SERIAL_LOG" 2>/dev/null || true)
PCI_CENSUS_LINE=$(grep -h -E 'PCI: Enumeration complete\. Found [0-9]+ devices \([0-9]+ VirtIO block, [0-9]+ network\)' \
    "$SERIAL_LOG" 2>/dev/null | tail -1)

if [ "${NET_PCI_ATTEMPT:-0}" -lt 1 ]; then
    fail "virtio-net PCI attempt/failure line absent -- either the VM never reached driver init, or VMware unexpectedly presented a virtio-net device (see kernel/src/drivers/mod.rs)"
fi
if [ "${E1000_SUCCESS:-0}" -lt 1 ]; then
    fail "e1000 fallback success line absent -- the fallback in kernel/src/drivers/mod.rs did not report success"
fi
if [ -z "$PCI_CENSUS_LINE" ]; then
    fail "device-enumeration census line absent -- see kernel/src/drivers/pci.rs"
fi

echo ""
echo "VMware e1000-fallback gate: PASS"
echo "  virtio-net PCI attempted and failed, e1000 fallback initialized"
echo "  $PCI_CENSUS_LINE"
echo "  Serial: $SERIAL_LOG"
echo "  VM stopped: $VMX_FILE_STOPPED"
