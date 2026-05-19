#!/usr/bin/env bash
set +e
set -u

ROOT="/Users/wrb/fun/code/breenix.worktrees/ahci-interrupt-driven"
CONTROL="/Users/wrb/Downloads/Ralph/breenix-ahci-interrupt-driven-1779178791"
ART="$ROOT/turn5-artifacts"
RUN_DIR="$ART/single-boot-run"
HB="$CONTROL/heartbeat"
SERIAL_LOG="/tmp/breenix-parallels-serial.log"
KERNEL_ELF="$ROOT/target/aarch64-breenix/release/kernel-aarch64"
GDB_PORT=9600

RUN_PID=""
DBG_PID=""
VM_NAME=""

rm -rf "$RUN_DIR"
mkdir -p "$RUN_DIR"
: >"$RUN_DIR/harness.log"

log() {
  printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$RUN_DIR/harness.log"
}

heartbeat() {
  touch "$HB"
}

qemu_cleanup() {
  pkill -9 qemu-system-x86 2>/dev/null || true
  killall -9 qemu-system-x86_64 2>/dev/null || true
  pgrep -l qemu >>"$RUN_DIR/qemu-cleanup.log" 2>&1 || echo "All QEMU processes killed" >>"$RUN_DIR/qemu-cleanup.log"
}

delete_breenix_vms() {
  local tmp vm
  tmp="$(mktemp)"
  prlctl list --all 2>/dev/null | awk 'NR > 1 && $NF ~ /^breenix-/ { print $NF }' >"$tmp" || true
  while IFS= read -r vm; do
    [ -n "$vm" ] || continue
    prlctl stop "$vm" --kill >>"$RUN_DIR/prlctl-cleanup.log" 2>&1 || true
    prlctl delete "$vm" >>"$RUN_DIR/prlctl-cleanup.log" 2>&1 || true
  done <"$tmp"
  rm -f "$tmp"
}

cleanup_run() {
  if [ -n "${RUN_PID:-}" ]; then
    pkill -P "$RUN_PID" >/dev/null 2>&1 || true
    kill "$RUN_PID" >/dev/null 2>&1 || true
    wait "$RUN_PID" >/dev/null 2>&1 || true
  fi
  pkill -f "/bin/bash ./run.sh --parallels" >/dev/null 2>&1 || true
  pkill -f "tail -f $SERIAL_LOG" >/dev/null 2>&1 || true
  if [ -n "${DBG_PID:-}" ]; then
    kill "$DBG_PID" >/dev/null 2>&1 || true
    wait "$DBG_PID" >/dev/null 2>&1 || true
  fi
}

copy_serial() {
  if [ -f "$SERIAL_LOG" ]; then
    cp "$SERIAL_LOG" "$ART/single-boot-serial.log"
    cp "$SERIAL_LOG" "$RUN_DIR/single-boot-serial.log"
  else
    : >"$ART/single-boot-serial.log"
    : >"$RUN_DIR/single-boot-serial.log"
  fi
}

vm_name_from_run_out() {
  awk '/^VM name:[[:space:]]+breenix-/ { print $3 } /^VM:[[:space:]]+breenix-/ { print $2 }' "$RUN_DIR/run.out" 2>/dev/null | tail -1
}

wait_for_port() {
  local i
  for i in $(seq 1 60); do
    heartbeat
    nc -z 127.0.0.1 "$GDB_PORT" >/dev/null 2>&1 && return 0
    sleep 1
  done
  return 1
}

sym_addr() {
  local pattern="$1"
  nm "$KERNEL_ELF" | awk -v p="$pattern" '$3 ~ p { print "0x" $1; exit }'
}

generate_gdb_script() {
  local ahci_irq ahci_isr_count ahci_last_cpu polled timer_ticks timer_hw_ticks timer_total
  ahci_irq="$(sym_addr 'drivers4ahci8AHCI_IRQ17')"
  ahci_isr_count="$(sym_addr 'drivers4ahci14AHCI_ISR_COUNT17')"
  ahci_last_cpu="$(sym_addr 'drivers4ahci19AHCI_ISR_LAST_MPIDR17')"
  polled="$(sym_addr '^ahci_polled_completion_count$')"
  timer_ticks="$(sym_addr 'timer_interrupt16TIMER_TICK_COUNT17')"
  timer_hw_ticks="$(sym_addr 'timer_interrupt19TIMER_TICK_HW_COUNT17')"
  timer_total="$(sym_addr 'timer_interrupt21TIMER_INTERRUPT_COUNT17')"

  {
    echo "ahci_irq_addr=$ahci_irq"
    echo "ahci_isr_count_addr=$ahci_isr_count"
    echo "ahci_isr_last_mpidr_aff0_addr=$ahci_last_cpu"
    echo "ahci_polled_completion_count_addr=$polled"
    echo "timer_tick_count_addr=$timer_ticks"
    echo "timer_tick_hw_count_addr=$timer_hw_ticks"
    echo "timer_interrupt_count_addr=$timer_total"
  } >"$RUN_DIR/symbol-addresses.txt"

  cat >"$RUN_DIR/gdb-endpoint-state.gdb" <<GDB
set pagination off
set confirm off
set architecture aarch64
set remotetimeout 10
set mem inaccessible-by-default off
set logging file $ART/gdb-endpoint-state.log
set logging overwrite on
set logging enabled on

echo === TURN5 AHCI level-SPI ENDPOINT STATE ===\\n
target remote 127.0.0.1:$GDB_PORT

set \$AHCI_IRQ = $ahci_irq
set \$AHCI_ISR_COUNT = $ahci_isr_count
set \$AHCI_ISR_LAST_MPIDR = $ahci_last_cpu
set \$AHCI_POLLED_COMPLETION_COUNT = $polled
set \$TIMER_TICK_COUNT = $timer_ticks
set \$TIMER_TICK_HW_COUNT = $timer_hw_ticks
set \$TIMER_INTERRUPT_COUNT = $timer_total

printf "ahci_irq=%u\\n", *(unsigned int*)\$AHCI_IRQ
printf "ahci_isr_count=%u\\n", *(unsigned int*)\$AHCI_ISR_COUNT
printf "ahci_isr_last_mpidr_aff0=%lu\\n", *(unsigned long*)\$AHCI_ISR_LAST_MPIDR
printf "ahci_polled_completion_count=%u\\n", *(unsigned int*)\$AHCI_POLLED_COMPLETION_COUNT
printf "timer_tick_count_cpu0=%lu\\n", *(unsigned long*)(\$TIMER_TICK_COUNT + 0)
printf "timer_tick_count_cpu1=%lu\\n", *(unsigned long*)(\$TIMER_TICK_COUNT + 8)
printf "timer_tick_count_cpu2=%lu\\n", *(unsigned long*)(\$TIMER_TICK_COUNT + 16)
printf "timer_tick_count_cpu3=%lu\\n", *(unsigned long*)(\$TIMER_TICK_COUNT + 24)
printf "timer_tick_hw_count_cpu0=%lu\\n", *(unsigned long*)(\$TIMER_TICK_HW_COUNT + 0)
printf "timer_tick_hw_count_cpu1=%lu\\n", *(unsigned long*)(\$TIMER_TICK_HW_COUNT + 8)
printf "timer_tick_hw_count_cpu2=%lu\\n", *(unsigned long*)(\$TIMER_TICK_HW_COUNT + 16)
printf "timer_tick_hw_count_cpu3=%lu\\n", *(unsigned long*)(\$TIMER_TICK_HW_COUNT + 24)
printf "timer_interrupt_count=%lu\\n", *(unsigned long*)\$TIMER_INTERRUPT_COUNT
detach
quit
GDB
}

capture_gdb() {
  generate_gdb_script
  VM_NAME="${VM_NAME:-$(vm_name_from_run_out)}"
  if [ -z "$VM_NAME" ]; then
    echo "gdb_rc=vm-unknown" >"$ART/gdb-endpoint-state.log"
    return
  fi

  log "starting guest-debugger for $VM_NAME on port $GDB_PORT"
  prlctl guest-debugger "$VM_NAME" --port "$GDB_PORT" >"$RUN_DIR/guest-debugger.out" 2>&1 &
  DBG_PID=$!
  echo "$DBG_PID" >"$RUN_DIR/guest-debugger.pid"

  if ! wait_for_port; then
    log "guest-debugger port did not open"
    echo "gdb_rc=guestdebugger-port-timeout" >"$ART/gdb-endpoint-state.log"
    return
  fi

  log "running GDB endpoint capture"
  (
    cd "$RUN_DIR" || exit 1
    gdb -nx -batch -x gdb-endpoint-state.gdb "$KERNEL_ELF" >gdb-driver.out 2>&1
  ) &
  local gdb_pid=$!
  local start now
  start="$(date +%s)"
  while kill -0 "$gdb_pid" 2>/dev/null; do
    heartbeat
    now="$(date +%s)"
    if [ $((now - start)) -gt 120 ]; then
      log "GDB endpoint capture timed out"
      kill "$gdb_pid" >/dev/null 2>&1 || true
      sleep 2
      kill -9 "$gdb_pid" >/dev/null 2>&1 || true
      echo "gdb_rc=timeout" >>"$ART/gdb-endpoint-state.log"
      return
    fi
    sleep 1
  done
  wait "$gdb_pid"
  echo "gdb_rc=$?" >>"$ART/gdb-endpoint-state.log"
}

write_summaries() {
  copy_serial
  grep -aE 'AHCI|ahci|CPU0 timer|CPU0|timer regression|KERNEL PANIC|panicked at|TIMEOUT|SOFT LOCKUP|Data Abort|Synchronous exception|freeze-watch|bwm-fps|Frame #' \
    "$ART/single-boot-serial.log" >"$RUN_DIR/signals.log" 2>/dev/null || true
  grep -aE 'AHCI|ahci|TIMEOUT|CPU0 timer|KERNEL PANIC|panicked at|SOFT LOCKUP|Data Abort|Synchronous exception' \
    "$ART/single-boot-serial.log" >"$ART/serial-signals.log" 2>/dev/null || true
  awk -F= '/ahci_polled_completion_count=/ { print $2 }' "$ART/gdb-endpoint-state.log" 2>/dev/null | tail -1 >"$ART/polling-counter.txt"
  if [ ! -s "$ART/polling-counter.txt" ]; then
    echo "unavailable" >"$ART/polling-counter.txt"
  fi
}

finish() {
  copy_serial
  write_summaries
  cleanup_run
  delete_breenix_vms
  qemu_cleanup
  prlctl list --all >"$RUN_DIR/prlctl-after.log" 2>&1 || true
  heartbeat
}

trap 'log "received signal"; finish; exit 143' INT TERM HUP

log "starting Turn 5 AHCI level-SPI single Parallels boot"
heartbeat
qemu_cleanup
delete_breenix_vms
rm -f "$ART/gdb-endpoint-state.log" "$ART/polling-counter.txt" "$ART/single-boot-serial.log" "$ART/serial-signals.log"
rm -f "$SERIAL_LOG"

(
  cd "$ROOT" || exit 1
  ./run.sh --parallels
) >"$RUN_DIR/run.out" 2>&1 &
RUN_PID=$!
echo "$RUN_PID" >"$RUN_DIR/run.pid"

log "waiting for fresh VM name"
wait_start="$(date +%s)"
while true; do
  heartbeat
  VM_NAME="$(vm_name_from_run_out)"
  if [ -n "$VM_NAME" ]; then
    log "detected VM $VM_NAME"
    break
  fi
  if ! kill -0 "$RUN_PID" 2>/dev/null; then
    wait "$RUN_PID"
    echo "run_script_rc=$?" >"$RUN_DIR/result.txt"
    log "run.sh exited before VM creation"
    finish
    exit 0
  fi
  now="$(date +%s)"
  if [ $((now - wait_start)) -gt 420 ]; then
    echo "run_script_rc=vm-create-timeout" >"$RUN_DIR/result.txt"
    log "timed out waiting for VM creation"
    finish
    exit 0
  fi
  sleep 1
done

log "waiting 90s after VM start for boot and CPU0 timer alarm window"
for _ in $(seq 1 90); do
  heartbeat
  sleep 1
done

copy_serial
capture_gdb
write_summaries

{
  echo "vm_name=$VM_NAME"
  echo "serial_bytes=$(wc -c <"$ART/single-boot-serial.log" 2>/dev/null || echo 0)"
  echo "gdb_endpoint_log=$ART/gdb-endpoint-state.log"
  echo "polling_counter=$(cat "$ART/polling-counter.txt" 2>/dev/null || echo unavailable)"
  echo "ahci_timeout_markers=$(grep -aciE 'AHCI.*TIMEOUT|AHCI: command timeout|command timeout' "$ART/single-boot-serial.log" 2>/dev/null || true)"
  echo "cpu0_timer_alarm_markers=$(grep -aciE 'CPU0.*timer|timer regression|CPU0-tick alarm' "$ART/single-boot-serial.log" 2>/dev/null || true)"
  echo "panic_markers=$(grep -aciE 'KERNEL PANIC|panicked at|Data Abort|Synchronous exception' "$ART/single-boot-serial.log" 2>/dev/null || true)"
} >"$RUN_DIR/result.txt"

finish
log "Turn 5 single boot harness complete"
