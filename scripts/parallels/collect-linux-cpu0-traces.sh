#!/usr/bin/env bash
set -euo pipefail

# Collect canonical CPU 0 interrupt/scheduler traces from the linux-probe VM.
#
# This script is intentionally host-driven so future investigations can replay
# the same Linux reference scenarios without relying on ad hoc terminal work.

VM_HOST="${LINUX_PROBE_HOST:-10.211.55.3}"
VM_USER="${LINUX_PROBE_USER:-wrb}"
VM_PASSWORD="${LINUX_PROBE_PASSWORD:-root}"
VM_SUDO_PASSWORD="${LINUX_PROBE_SUDO_PASSWORD:-$VM_PASSWORD}"

STAMP="$(date +%Y%m%d-%H%M%S)"
OUT_DIR="${1:-logs/linux-probe-cpu0/${STAMP}}"
REMOTE_DIR="/tmp/breenix-linux-cpu0"

if ! command -v sshpass >/dev/null 2>&1; then
    echo "ERROR: sshpass is required on the host" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"

SSH_BASE=(
    sshpass -p "$VM_PASSWORD"
    ssh
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile=/dev/null
)
SCP_BASE=(
    sshpass -p "$VM_PASSWORD"
    scp
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile=/dev/null
)

remote() {
    "${SSH_BASE[@]}" "${VM_USER}@${VM_HOST}" "$@"
}

echo "Collecting linux-probe CPU0 traces into $OUT_DIR"

cat >"$OUT_DIR/README.txt" <<EOF
linux-probe CPU0 trace bundle
Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")
Host: ${VM_USER}@${VM_HOST}

Purpose:
- Capture canonical Linux CPU0 timer/IPI/idle/scheduler traces on Parallels/HVF
- Provide reproducible reference artifacts for Breenix CPU0 SMP comparison

Files:
- env.txt: guest kernel and trace capability inventory
- interrupts.before.txt: /proc/interrupts before tracing
- cpu0-events.report.txt: CPU0 tracepoint report for timer/IPI/scheduler events
- cpu0-events.stderr.txt: trace-cmd record stderr for the event trace
- cpu0-fg.report.txt: CPU0 function-graph report for idle/IRQ/schedule flow
- cpu0-fg.stderr.txt: trace-cmd record stderr for the function-graph trace
- interrupts.after.txt: /proc/interrupts after tracing
EOF

cat <<'REMOTE_SCRIPT' | remote "cat > ${REMOTE_DIR}.sh && chmod +x ${REMOTE_DIR}.sh && env VM_SUDO_PASSWORD='${VM_SUDO_PASSWORD}' ${REMOTE_DIR}.sh"
#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="/tmp/breenix-linux-cpu0"
mkdir -p "$OUT_DIR"

sudo_run() {
    printf '%s\n' "$VM_SUDO_PASSWORD" | sudo -S -p '' "$@"
}

sudo_run true >/dev/null

uname -a >"$OUT_DIR/env.txt"
{
    echo
    echo "=== os-release ==="
    sed -n '1,8p' /etc/os-release
    echo
    echo "=== trace events ==="
    sudo_run trace-cmd list -e | egrep '^(sched:|irq:|ipi:|timer:|power:cpu_idle)' || true
    echo
    echo "=== function graph targets ==="
    sudo_run trace-cmd list -f | egrep '^(gic_handle_irq|arch_timer_handler_virt|tick_handle_periodic|update_process_times|scheduler_tick|ipi_handler|schedule|do_idle)$' || true
} >>"$OUT_DIR/env.txt"

cat /proc/interrupts >"$OUT_DIR/interrupts.before.txt"

rm -f /tmp/cpu0_fifo
sudo_run rm -f /tmp/cpu0-events.dat /tmp/cpu0-fg.dat
rm -f "$OUT_DIR/cpu0-events.report.txt" \
      "$OUT_DIR/cpu0-events.stderr.txt" \
      "$OUT_DIR/cpu0-fg.report.txt" \
      "$OUT_DIR/cpu0-fg.stderr.txt"

mkfifo /tmp/cpu0_fifo
(taskset -c 0 bash -c 'while read -r _; do :; done < /tmp/cpu0_fifo' >/dev/null 2>&1) &
reader_pid=$!
sleep 0.2
(taskset -c 1 bash -c 'while :; do printf x\\n > /tmp/cpu0_fifo; done' >/dev/null 2>&1) &
writer_pid=$!

sudo_run trace-cmd record -q -o /tmp/cpu0-events.dat \
    -e irq:irq_handler_entry \
    -e irq:irq_handler_exit \
    -e irq:softirq_entry \
    -e irq:softirq_exit \
    -e ipi:ipi_entry \
    -e ipi:ipi_exit \
    -e ipi:ipi_raise \
    -e ipi:ipi_send_cpu \
    -e ipi:ipi_send_cpumask \
    -e sched:sched_switch \
    -e sched:sched_wakeup \
    -e sched:sched_waking \
    -e sched:sched_wake_idle_without_ipi \
    -e timer:hrtimer_expire_entry \
    -e timer:hrtimer_expire_exit \
    -e power:cpu_idle \
    sleep 1 >/dev/null 2>"$OUT_DIR/cpu0-events.stderr.txt"

kill "$writer_pid" 2>/dev/null || true
wait "$writer_pid" 2>/dev/null || true
kill "$reader_pid" 2>/dev/null || true
wait "$reader_pid" 2>/dev/null || true
rm -f /tmp/cpu0_fifo

sudo_run trace-cmd report --cpu 0 /tmp/cpu0-events.dat >"$OUT_DIR/cpu0-events.report.txt"

sudo_run trace-cmd record -q -o /tmp/cpu0-fg.dat -M 1 -p function_graph \
    -g gic_handle_irq \
    -g arch_timer_handler_virt \
    -g tick_handle_periodic \
    -g update_process_times \
    -g scheduler_tick \
    -g ipi_handler \
    -g schedule \
    -g do_idle \
    --max-graph-depth 6 \
    sleep 0.3 >/dev/null 2>"$OUT_DIR/cpu0-fg.stderr.txt"

sudo_run trace-cmd report --cpu 0 /tmp/cpu0-fg.dat >"$OUT_DIR/cpu0-fg.report.txt"

cat /proc/interrupts >"$OUT_DIR/interrupts.after.txt"
REMOTE_SCRIPT

"${SCP_BASE[@]}" -r "${VM_USER}@${VM_HOST}:${REMOTE_DIR}/." "$OUT_DIR/"

echo "Linux CPU0 traces collected:"
find "$OUT_DIR" -maxdepth 1 -type f | LC_ALL=C sort
