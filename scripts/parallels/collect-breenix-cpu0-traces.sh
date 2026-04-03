#!/usr/bin/env bash
set -euo pipefail

# The captured serial stream can contain non-UTF8 bytes. Force byte-oriented
# text processing so macOS grep/sed/tail do not abort on illegal sequences.
export LC_ALL=C
export LANG=C

# Collect a reproducible Parallels-side Breenix CPU0 boot artifact bundle.
#
# This is intentionally host-driven. It performs a clean VM boot, captures the
# serial log, preserves the VM configuration used for the run, and extracts the
# scheduler/timer signatures that currently distinguish the Breenix failure from
# the Linux reference behavior.

VM_NAME="${BREENIX_PARALLELS_VM:-breenix-1774799978}"
WAIT_SECS="${BREENIX_PARALLELS_WAIT_SECS:-20}"
RESET_NVRAM="${BREENIX_PARALLELS_RESET_NVRAM:-1}"
POST_LOCKUP_WAIT_SECS="${BREENIX_PARALLELS_POST_LOCKUP_WAIT_SECS:-8}"

STAMP="$(date +%Y%m%d-%H%M%S)"
OUT_DIR="${1:-logs/breenix-parallels-cpu0/${STAMP}}"

if ! command -v prlctl >/dev/null 2>&1; then
    echo "ERROR: prlctl is required on the host" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"

vm_info_before="$(prlctl list -i "$VM_NAME")"
serial_path="$(
    printf '%s\n' "$vm_info_before" | awk -F"'" '/serial0 .* output=/{print $2; exit}'
)"
if [ -z "$serial_path" ]; then
    serial_path="${BREENIX_PARALLELS_SERIAL:-}"
fi
if [ -z "$serial_path" ]; then
    echo "ERROR: could not determine VM serial path" >&2
    exit 1
fi

vm_home="$(
    printf '%s\n' "$vm_info_before" | sed -n 's/^Home: //p' | head -n 1
)"
nvram_path=""
if [ -n "$vm_home" ]; then
    nvram_path="${vm_home%/}/NVRAM.dat"
fi

cat >"$OUT_DIR/README.txt" <<EOF
Breenix Parallels CPU0 trace bundle
Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")
VM: ${VM_NAME}
Serial: ${serial_path}

Purpose:
- Capture a clean Breenix ARM64 Parallels boot with CPU0 scheduler/timer evidence
- Preserve the exact VM configuration and serial output used for Linux comparison

Commands:
- prlctl stop ${VM_NAME} --kill
- truncate serial log at ${serial_path}
- optional NVRAM reset at ${nvram_path:-<not available>}
- prlctl start ${VM_NAME}
- wait up to ${WAIT_SECS}s for boot evidence
- after soft lockup, continue for up to ${POST_LOCKUP_WAIT_SECS}s to preserve
  post-lockup timer evidence

Files:
- vm-info.before.txt: prlctl VM configuration before restart
- start.output.txt: prlctl start output
- vm-info.after.txt: prlctl VM configuration after boot
- serial.log: full captured serial log for this run
- serial.signals.txt: extracted CPU0 scheduler/timer/lockup markers
- serial.tail.txt: final 120 lines for quick review
- summary.txt: normalized interpretation of the current run
EOF

printf '%s\n' "$vm_info_before" >"$OUT_DIR/vm-info.before.txt"

prlctl stop "$VM_NAME" --kill >/dev/null 2>&1 || true
for _ in $(seq 1 15); do
    vm_state="$(prlctl status "$VM_NAME" 2>/dev/null | awk '{print $NF}')"
    if [ "$vm_state" = "stopped" ]; then
        break
    fi
    sleep 1
done

mkdir -p "$(dirname "$serial_path")"
: >"$serial_path"

if [ "$RESET_NVRAM" = "1" ] && [ -n "$nvram_path" ]; then
    rm -f "$nvram_path"
    rm -f "${vm_home%/}"/*.mem "${vm_home%/}"/*.mem.sh 2>/dev/null || true
fi

prlctl start "$VM_NAME" >"$OUT_DIR/start.output.txt" 2>&1

deadline=$((SECONDS + WAIT_SECS))
soft_lockup_seen=0
post_lockup_deadline=0
while [ "$SECONDS" -lt "$deadline" ]; do
    if grep -q "SOFT LOCKUP DETECTED" "$serial_path" 2>/dev/null; then
        if [ "$soft_lockup_seen" = "0" ]; then
            soft_lockup_seen=1
            post_lockup_deadline=$((SECONDS + POST_LOCKUP_WAIT_SECS))
        fi
    fi
    if [ "$soft_lockup_seen" = "1" ] && [ "$SECONDS" -ge "$post_lockup_deadline" ]; then
        break
    fi
    if [ "$soft_lockup_seen" = "0" ] && grep -q "\\[timer\\] cpu0 ticks=" "$serial_path" 2>/dev/null; then
        break
    fi
    sleep 1
done

prlctl list -i "$VM_NAME" >"$OUT_DIR/vm-info.after.txt"
cp "$serial_path" "$OUT_DIR/serial.log"

rg -n \
    "EL0_SYSCALL|\\[SCHED\\] queue_empty|SOFT LOCKUP DETECTED|Ready queue length:|Ready queue:|tid=[0-9]+ state=|SYSCALL_TOTAL:|IRQ_TOTAL:|CTX_SWITCH_TOTAL:|TIMER_TICK_TOTAL:|Timer IRQ count:|\\[timer\\] cpu0 ticks=" \
    "$OUT_DIR/serial.log" >"$OUT_DIR/serial.signals.txt" || true

tail -n 120 "$OUT_DIR/serial.log" >"$OUT_DIR/serial.tail.txt" || true

soft_lockup=0
if grep -q "SOFT LOCKUP DETECTED" "$OUT_DIR/serial.log"; then
    soft_lockup=1
fi

queue_empty_lines="$(grep -c "\\[SCHED\\] queue_empty" "$OUT_DIR/serial.log" 2>/dev/null || true)"
last_timer_tick="$(
    sed -n 's/.*\[timer\] cpu0 ticks=\([0-9][0-9]*\).*/\1/p' "$OUT_DIR/serial.log" | tail -n 1
)"
timer_irq_count="$(
    sed -n 's/.*Timer IRQ count:  *\([0-9][0-9]*\).*/\1/p' "$OUT_DIR/serial.log" | tail -n 1
)"
stuck_tid="$(
    sed -n 's/.*\[SCHED\] queue_empty stuck_tid=\([0-9][0-9]*\).*/\1/p' "$OUT_DIR/serial.log" | head -n 1
)"

cat >"$OUT_DIR/summary.txt" <<EOF
vm=${VM_NAME}
serial=${serial_path}
wait_secs=${WAIT_SECS}
post_lockup_wait_secs=${POST_LOCKUP_WAIT_SECS}
soft_lockup=${soft_lockup}
queue_empty_lines=${queue_empty_lines}
first_stuck_tid=${stuck_tid:-none}
timer_irq_count=${timer_irq_count:-none}
last_cpu0_tick=${last_timer_tick:-none}

Interpretation:
- If soft_lockup=1 and last_cpu0_tick is still increasing, CPU0 timer delivery
  remained live after scheduler progress stopped.
- If queue_empty_lines>0 with a concrete stuck tid, at least one Ready thread
  became unreachable from all run queues/current-thread/deferred-requeue slots.
EOF

echo "Breenix Parallels CPU0 traces collected:"
find "$OUT_DIR" -maxdepth 1 -type f | LC_ALL=C sort
