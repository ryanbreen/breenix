#!/usr/bin/env bash
set +e
set -u

ROOT="/Users/wrb/fun/code/breenix"
CONTROL="/Users/wrb/Downloads/Ralph/breenix-compositor-wait-softlock-1779136659"
ART="$ROOT/turn2-artifacts"
HB="$CONTROL/heartbeat"
SERIAL_LOG="/tmp/breenix-parallels-serial.log"
SCREENSHOT_TMP="/tmp/breenix-screenshot.png"
GDB_SCRIPT="$ART/gdb_softlock_capture.gdb"
KERNEL_ELF="$ROOT/target/aarch64-breenix/release/kernel-aarch64"
GDB_PORT=9600

MODE="no-build"
MAX_SECONDS=220
for arg in "$@"; do
  case "$arg" in
    mode=build) MODE="build" ;;
    mode=no-build) MODE="no-build" ;;
    max=*) MAX_SECONDS="${arg#max=}" ;;
    *)
      echo "unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

RUN_DIR="$ART/reproduce-run1"
if [ -e "$RUN_DIR" ]; then
  echo "run directory already exists: $RUN_DIR" >&2
  exit 2
fi
mkdir -p "$RUN_DIR"
HARNESS_LOG="$RUN_DIR/harness.log"
: >"$HARNESS_LOG"

RUN_PID=""
DBG_PID=""
VM_NAME=""
CLASSIFICATION="unknown"
RUN_RC="not-finished"
GDB_RC="not-run"

log() {
  printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$HARNESS_LOG"
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
    cp "$SERIAL_LOG" "$RUN_DIR/run.serial.log"
  else
    : >"$RUN_DIR/run.serial.log"
  fi
}

snapshot_window_serial() {
  if [ -f "$SERIAL_LOG" ]; then
    cp "$SERIAL_LOG" "$RUN_DIR/window.serial.log"
  else
    : >"$RUN_DIR/window.serial.log"
  fi
}

vm_name_from_run_out() {
  awk '/^VM name:[[:space:]]+breenix-/ { print $3 } /^VM:[[:space:]]+breenix-/ { print $2 }' "$RUN_DIR/run.out" 2>/dev/null | tail -1
}

max_number() {
  local pattern="$1"
  local file="$2"
  grep -Eao "$pattern" "$file" 2>/dev/null | grep -Eo '[0-9]+' | sort -n | tail -1
}

count_pattern() {
  local pattern="$1"
  local file="$2"
  grep -Eai "$pattern" "$file" 2>/dev/null | wc -l | tr -d ' '
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

capture_endpoint() {
  copy_serial
  VM_NAME="${VM_NAME:-$(vm_name_from_run_out)}"
  log "capturing screenshot for ${VM_NAME:-unknown}"
  if [ -n "$VM_NAME" ]; then
    prlctl capture "$VM_NAME" --file "$RUN_DIR/screenshot.png" >>"$RUN_DIR/prlctl-capture.log" 2>&1 || true
  fi
  if [ ! -f "$RUN_DIR/screenshot.png" ] && [ -f "$SCREENSHOT_TMP" ]; then
    cp "$SCREENSHOT_TMP" "$RUN_DIR/screenshot.png"
  fi

  if [ -z "$VM_NAME" ]; then
    GDB_RC="vm-unknown"
    return
  fi
  if [ ! -f "$GDB_SCRIPT" ]; then
    GDB_RC="script-missing"
    return
  fi

  log "starting guestdebugger port $GDB_PORT"
  prlctl guest-debugger "$VM_NAME" --port "$GDB_PORT" >"$RUN_DIR/guest-debugger.out" 2>&1 &
  DBG_PID=$!
  echo "$DBG_PID" >"$RUN_DIR/guest-debugger.pid"

  if ! wait_for_port; then
    log "guestdebugger port did not open"
    GDB_RC="guestdebugger-port-timeout"
    return
  fi

  log "running GDB softlock capture script"
  (
    cd "$RUN_DIR" || exit 1
    gdb -nx -batch -x "$GDB_SCRIPT" "$KERNEL_ELF" >gdb-driver.out 2>&1
  ) &
  local gdb_pid=$!
  local start now
  start="$(date +%s)"
  while kill -0 "$gdb_pid" 2>/dev/null; do
    heartbeat
    now="$(date +%s)"
    if [ $((now - start)) -gt 180 ]; then
      log "GDB timeout"
      kill "$gdb_pid" >/dev/null 2>&1 || true
      sleep 2
      kill -9 "$gdb_pid" >/dev/null 2>&1 || true
      GDB_RC="timeout"
      break
    fi
    sleep 1
  done
  if wait "$gdb_pid" >/dev/null 2>&1; then
    GDB_RC="0"
  elif [ "$GDB_RC" = "not-run" ]; then
    GDB_RC="$?"
  fi
}

extract_gdb_value() {
  local key="$1"
  awk -F'[= ]+' -v k="$key" '{
    for (i = 1; i <= NF; i++) {
      if ($i == k) {
        print $(i + 1)
      }
    }
  }' "$RUN_DIR/gdb_softlock_state.out" 2>/dev/null | tail -1
}

write_result() {
  copy_serial
  local max_a max_b max_frame max_uptime far_count softlock_count panic_count data_abort_count
  local stuck13_count scheduler_lock process_lock gpu_lock ahci_irq serial_bytes window_serial_bytes

  max_a="$(max_number 'Frame #[0-9]+' "$RUN_DIR/run.serial.log")"
  max_b="$(max_number 'frame=[0-9]+' "$RUN_DIR/run.serial.log")"
  max_frame="${max_a:-0}"
  if [ "${max_b:-0}" -gt "$max_frame" ]; then
    max_frame="$max_b"
  fi
  max_uptime="$(max_number 'uptime_ms=[0-9]+' "$RUN_DIR/run.serial.log")"
  far_count="$(count_pattern 'FAR=0x0*ccd|FAR=0xccd' "$RUN_DIR/run.serial.log")"
  softlock_count="$(count_pattern 'SOFT LOCKUP' "$RUN_DIR/run.serial.log")"
  stuck13_count="$(count_pattern '\[SCHED\] queue_empty stuck_tid=13 count=' "$RUN_DIR/run.serial.log")"
  panic_count="$(count_pattern 'KERNEL PANIC|panicked at' "$RUN_DIR/run.serial.log")"
  data_abort_count="$(count_pattern 'Data Abort|Synchronous exception' "$RUN_DIR/run.serial.log")"
  scheduler_lock="$(extract_gdb_value scheduler_lock_byte)"
  process_lock="$(extract_gdb_value process_manager_lock_byte)"
  gpu_lock="$(extract_gdb_value gpu_pci_lock_byte)"
  ahci_irq="$(extract_gdb_value ahci_irq)"
  serial_bytes="$(wc -c <"$RUN_DIR/run.serial.log" 2>/dev/null || echo 0)"
  window_serial_bytes="$(wc -c <"$RUN_DIR/window.serial.log" 2>/dev/null || echo 0)"

  {
    echo "classification=$CLASSIFICATION"
    echo "run_rc=$RUN_RC"
    echo "gdb_rc=$GDB_RC"
    echo "vm=$VM_NAME"
    echo "max_frame=$max_frame"
    echo "max_uptime_ms=${max_uptime:-0}"
    echo "stuck_tid13_count=$stuck13_count"
    echo "softlock_count=$softlock_count"
    echo "far_0xccd_count=$far_count"
    echo "panic_count=$panic_count"
    echo "data_abort_count=$data_abort_count"
    echo "scheduler_lock_byte=${scheduler_lock:-unknown}"
    echo "process_manager_lock_byte=${process_lock:-unknown}"
    echo "gpu_pci_lock_byte=${gpu_lock:-unknown}"
    echo "ahci_irq=${ahci_irq:-unknown}"
    echo "window_serial_bytes=$window_serial_bytes"
    echo "full_serial_bytes=$serial_bytes"
  } >"$RUN_DIR/result.txt"

  grep -aE 'FAR=0x|SOFT LOCKUP|KERNEL PANIC|panicked at|Data Abort|Synchronous exception|\[SCHED\] queue_empty stuck_tid=13|\[freeze-watch\]|\[virgl-composite\] Frame #|\[bwm-fps\]' \
    "$RUN_DIR/run.serial.log" >"$RUN_DIR/signals.log" 2>/dev/null || true
  tail -160 "$RUN_DIR/run.serial.log" >"$RUN_DIR/tail160.log" 2>/dev/null || true
}

finish() {
  cleanup_run
  delete_breenix_vms
  qemu_cleanup
  prlctl list --all >"$RUN_DIR/prlctl-after.log" 2>&1 || true
  heartbeat
}

trap 'log "received signal"; CLASSIFICATION="interrupted"; snapshot_window_serial; capture_endpoint; write_result; finish; exit 143' INT TERM HUP

log "starting turn2 Parallels BWM softlock capture max=${MAX_SECONDS}s mode=$MODE"
heartbeat
qemu_cleanup
delete_breenix_vms
: >"$SERIAL_LOG"
rm -f "$SCREENSHOT_TMP"

(
  cd "$ROOT" || exit 1
  if [ "$MODE" = "build" ]; then
    ./run.sh --parallels
  else
    ./run.sh --parallels --no-build
  fi
) >"$RUN_DIR/run.out" 2>&1 &
RUN_PID=$!
echo "$RUN_PID" >"$RUN_DIR/run.pid"

log "waiting for current run.sh VM name"
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
    RUN_RC="$?"
    CLASSIFICATION="run-script-exited-before-vm"
    snapshot_window_serial
    capture_endpoint
    write_result
    finish
    exit 0
  fi
  now="$(date +%s)"
  if [ $((now - wait_start)) -gt 300 ]; then
    CLASSIFICATION="vm-create-timeout"
    snapshot_window_serial
    capture_endpoint
    write_result
    finish
    exit 0
  fi
  sleep 1
done

start="$(date +%s)"
active_start=""
last_log="$start"
progress_seen=0
BOOT_PROGRESS_TIMEOUT=180

while true; do
  heartbeat
  now="$(date +%s)"
  elapsed=$((now - start))
  active_elapsed=0
  if [ -n "$active_start" ]; then
    active_elapsed=$((now - active_start))
  fi

  if [ -f "$SERIAL_LOG" ] && grep -qaE '\[virgl-composite\] Frame #|\[bwm-fps\]|\[freeze-watch\]' "$SERIAL_LOG" 2>/dev/null; then
    progress_seen=1
    if [ -z "$active_start" ]; then
      active_start="$now"
      log "BWM progress detected; starting active ${MAX_SECONDS}s window"
    fi
  fi

  if [ -f "$SERIAL_LOG" ] && grep -qaE '\[SCHED\] queue_empty stuck_tid=13 count=' "$SERIAL_LOG" 2>/dev/null; then
    CLASSIFICATION="softlock-leading-edge"
    log "detected first stuck_tid=13 queue_empty marker"
    break
  fi

  if [ -f "$SERIAL_LOG" ] && grep -qaE 'FAR=0x0*ccd|FAR=0xccd|KERNEL PANIC|panicked at|Data Abort|Synchronous exception' "$SERIAL_LOG" 2>/dev/null; then
    CLASSIFICATION="non-softlock-failure-marker"
    log "detected non-softlock failure marker"
    break
  fi

  if [ -n "$active_start" ] && [ "$active_elapsed" -ge "$MAX_SECONDS" ]; then
    CLASSIFICATION="no-softlock-within-window"
    log "max active window reached classification=$CLASSIFICATION"
    break
  fi

  if [ -z "$active_start" ] && [ "$elapsed" -ge "$BOOT_PROGRESS_TIMEOUT" ]; then
    CLASSIFICATION="no-bwm-progress-within-window"
    log "BWM progress timeout elapsed=${elapsed}s"
    break
  fi

  if ! kill -0 "$RUN_PID" 2>/dev/null; then
    wait "$RUN_PID"
    RUN_RC="$?"
    CLASSIFICATION="run-script-exited"
    log "run.sh exited rc=$RUN_RC"
    break
  fi

  if [ $((now - last_log)) -ge 30 ]; then
    local_size=0
    [ -f "$SERIAL_LOG" ] && local_size="$(stat -f%z "$SERIAL_LOG" 2>/dev/null || echo 0)"
    log "monitor elapsed=${elapsed}s active_elapsed=${active_elapsed}s size=${local_size} progress_seen=${progress_seen}"
    last_log="$now"
  fi
  sleep 1
done

if kill -0 "$RUN_PID" 2>/dev/null; then
  RUN_RC="still-running-at-capture"
else
  wait "$RUN_PID"
  RUN_RC="$?"
fi

snapshot_window_serial
capture_endpoint
write_result
log "result $CLASSIFICATION"
cat "$RUN_DIR/result.txt"
finish
