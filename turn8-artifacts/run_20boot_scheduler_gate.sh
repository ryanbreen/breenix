#!/usr/bin/env bash
set +e
set -u

ROOT="/Users/wrb/fun/code/breenix"
CONTROL="/Users/wrb/Downloads/Ralph/breenix-polling-elimination-linux-gate-1779267844"
ART="$ROOT/turn8-artifacts/stress-20boot"
HB="$CONTROL/heartbeat.codex"
SERIAL_LOG="/tmp/breenix-parallels-serial.log"
SCREENSHOT_TMP="/tmp/breenix-screenshot.png"

BOOT_COUNT="${BOOT_COUNT:-20}"
WAIT_SECONDS="${WAIT_SECONDS:-90}"
RUN_TIMEOUT=$((WAIT_SECONDS + 480))
PRL_TIMEOUT=60

RUN_PID=""
VM_NAME=""

mkdir -p "$ART"
rm -rf "$ART"/boot-*
: >"$ART/harness.log"
: >"$ART/qemu-cleanup.log"
printf 'boot\trun_status\tstatus\treason\tactivity\tmax_uptime_ms\tcpu\tmsi\tirq\tirq_delta\tlock\tstale_not_ready\tstale_current\tstale_deferred\tfailures\tdata_abort\tpid1\n' >"$ART/metrics.tsv"

log() {
  printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$ART/harness.log"
}

heartbeat() {
  touch "$HB"
}

qemu_cleanup() {
  pkill -9 qemu-system-x86 2>/dev/null || true
  killall -9 qemu-system-x86_64 2>/dev/null || true
  pgrep -l qemu >>"$ART/qemu-cleanup.log" 2>&1 || echo "All QEMU processes killed" >>"$ART/qemu-cleanup.log"
}

run_with_timeout() {
  local timeout_s="$1"
  shift
  "$@" &
  local pid=$!
  local start now
  start="$(date +%s)"
  while kill -0 "$pid" 2>/dev/null; do
    heartbeat
    now="$(date +%s)"
    if [ $((now - start)) -ge "$timeout_s" ]; then
      kill "$pid" >/dev/null 2>&1 || true
      sleep 2
      kill -9 "$pid" >/dev/null 2>&1 || true
      wait "$pid" >/dev/null 2>&1 || true
      return 124
    fi
    sleep 1
  done
  wait "$pid"
}

cleanup_run_process() {
  if [ -n "${RUN_PID:-}" ]; then
    pkill -P "$RUN_PID" >/dev/null 2>&1 || true
    kill "$RUN_PID" >/dev/null 2>&1 || true
    wait "$RUN_PID" >/dev/null 2>&1 || true
    RUN_PID=""
  fi
  pkill -f "/bin/bash ./run.sh --parallels" >/dev/null 2>&1 || true
  pkill -f "tail -f $SERIAL_LOG" >/dev/null 2>&1 || true
}

stop_and_delete_vm() {
  local vm="$1"
  local run_dir="$2"
  [ -n "$vm" ] || return 0
  run_with_timeout "$PRL_TIMEOUT" prlctl stop "$vm" --kill >>"$run_dir/prlctl-cleanup.log" 2>&1 || true
  run_with_timeout "$PRL_TIMEOUT" prlctl delete "$vm" >>"$run_dir/prlctl-cleanup.log" 2>&1 || true
}

cleanup_old_breenix_vms() {
  local vm
  for vm in $(prlctl list --all 2>/dev/null | awk 'NR > 1 && $1 ~ /^breenix-/ { print $1 }'); do
    run_with_timeout "$PRL_TIMEOUT" prlctl stop "$vm" --kill >/dev/null 2>&1 || true
    run_with_timeout "$PRL_TIMEOUT" prlctl delete "$vm" >/dev/null 2>&1 || true
  done
}

vm_name_from_run_out() {
  local run_out="$1"
  awk '/^VM name:[[:space:]]+breenix-/ { print $3 } /^VM:[[:space:]]+breenix-/ { print $2 }' "$run_out" 2>/dev/null | tail -1
}

count_pattern() {
  local pattern="$1"
  local file="$2"
  grep -Eaci "$pattern" "$file" 2>/dev/null || true
}

max_key_value() {
  local key="$1"
  local file="$2"
  grep -Eao "${key}=[0-9]+" "$file" 2>/dev/null | cut -d= -f2 | sort -n | tail -1
}

last_counter_value() {
  local key="$1"
  local file="$2"
  grep -a "${key}=" "$file" 2>/dev/null | tail -1 | sed -nE "s/.*${key}=([0-9]+).*/\\1/p"
}

abs_diff() {
  local a="$1"
  local b="$2"
  if [ "$a" -ge "$b" ]; then
    echo $((a - b))
  else
    echo $((b - a))
  fi
}

append_reason() {
  local current="$1"
  local addition="$2"
  if [ -z "$current" ]; then
    printf '%s' "$addition"
  else
    printf '%s;%s' "$current" "$addition"
  fi
}

classify_boot() {
  local boot="$1"
  local run_dir="$2"
  local serial="$run_dir/parallels-boot.log"
  local run_status="$3"
  local max_uptime cpu msi irq irq_delta lock stale_not_ready stale_current stale_deferred
  local failures data_abort pid1 activity status reason

  max_uptime="$(max_key_value uptime_ms "$serial")"
  cpu="$(max_key_value timer_ticks_cpu0 "$serial")"
  msi="$(last_counter_value XHCI_MSI_EVENT_TOTAL "$serial")"
  irq="$(last_counter_value XHCI_IRQ_ENTRY_TOTAL "$serial")"
  lock="$(last_counter_value XHCI_LOCK_CONTENDED_TOTAL "$serial")"
  stale_not_ready="$(last_counter_value SCHED_STALE_QUEUE_NOT_READY "$serial")"
  stale_current="$(last_counter_value SCHED_STALE_QUEUE_CURRENT "$serial")"
  stale_deferred="$(last_counter_value SCHED_STALE_QUEUE_DEFERRED "$serial")"
  failures="$(count_pattern 'KERNEL PANIC|panicked at|Data Abort|DATA_ABORT|Synchronous exception' "$serial")"
  data_abort="$(count_pattern 'Data Abort|DATA_ABORT' "$serial")"

  max_uptime="${max_uptime:-0}"
  cpu="${cpu:-0}"
  msi="${msi:-0}"
  irq="${irq:-0}"
  lock="${lock:-0}"
  stale_not_ready="${stale_not_ready:-0}"
  stale_current="${stale_current:-0}"
  stale_deferred="${stale_deferred:-0}"
  failures="${failures:-0}"
  data_abort="${data_abort:-0}"
  irq_delta="$(abs_diff "$msi" "$irq")"

  if grep -aqE 'Generated PID 1|\[init\].*PID 1|PID 1 \[running\] init' "$serial" 2>/dev/null; then
    pid1="yes"
  else
    pid1="no"
  fi

  if [ "$max_uptime" -ge 35000 ]; then
    activity="yes"
  else
    activity="no"
  fi

  status="pass"
  reason=""
  if [ "$run_status" -ne 0 ]; then
    status="fail"; reason="$(append_reason "$reason" "run_status=$run_status")"
  fi
  if [ "$activity" != "yes" ]; then
    status="fail"; reason="$(append_reason "$reason" "activity=$activity")"
  fi
  if [ "$pid1" != "yes" ]; then
    status="fail"; reason="$(append_reason "$reason" "pid1=$pid1")"
  fi
  if [ "$data_abort" -ne 0 ]; then
    status="fail"; reason="$(append_reason "$reason" "data_abort=$data_abort")"
  fi
  if [ "$failures" -ne 0 ]; then
    status="fail"; reason="$(append_reason "$reason" "failures=$failures")"
  fi
  if [ "$msi" -le 0 ] || [ "$irq" -le 0 ] || [ "$irq_delta" -gt 2 ]; then
    status="fail"; reason="$(append_reason "$reason" "xhci_msi_irq=${msi}/${irq}")"
  fi

  [ -n "$reason" ] || reason="-"

  grep -aE 'xhci-counters|SCHED_STALE_QUEUE|KERNEL PANIC|panicked at|Data Abort|DATA_ABORT|Synchronous exception|Generated PID 1|\[init\].*PID 1|PID 1 \[running\] init|\[freeze-watch\]' \
    "$serial" >"$run_dir/signals.log" 2>/dev/null || true
  grep -a 'xhci-counters' "$serial" >"$run_dir/xhci-counters.txt" 2>/dev/null || true
  grep -aE 'Generated PID 1|\[init\].*PID 1|PID 1 \[running\] init' "$serial" >"$run_dir/pid1.txt" 2>/dev/null || true
  grep -aE 'Data Abort|DATA_ABORT' "$serial" >"$run_dir/data-abort.txt" 2>/dev/null || true
  grep -a 'timer_ticks_cpu0=' "$serial" | tail -1 >"$run_dir/cpu0-final.txt" 2>/dev/null || true

  {
    echo "boot=$boot"
    echo "run_status=$run_status"
    echo "status=$status"
    echo "reason=$reason"
    echo "activity=$activity"
    echo "max_uptime_ms=$max_uptime"
    echo "cpu=$cpu"
    echo "msi=$msi"
    echo "irq=$irq"
    echo "irq_delta=$irq_delta"
    echo "lock=$lock"
    echo "stale_not_ready=$stale_not_ready"
    echo "stale_current=$stale_current"
    echo "stale_deferred=$stale_deferred"
    echo "failures=$failures"
    echo "data_abort=$data_abort"
    echo "pid1=$pid1"
    echo "vm=$VM_NAME"
  } >"$run_dir/result.txt"

  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$boot" "$run_status" "$status" "$reason" "$activity" "$max_uptime" "$cpu" \
    "$msi" "$irq" "$irq_delta" "$lock" "$stale_not_ready" "$stale_current" \
    "$stale_deferred" "$failures" "$data_abort" "$pid1" >>"$ART/metrics.tsv"

  printf '%s run_status=%s status=%s reason=%s activity=%s cpu=%s msi=%s irq=%s lock=%s stale_not_ready=%s stale_current=%s stale_deferred=%s failures=%s data_abort=%s pid1=%s\n' \
    "$boot" "$run_status" "$status" "$reason" "$activity" "$cpu" "$msi" "$irq" "$lock" \
    "$stale_not_ready" "$stale_current" "$stale_deferred" "$failures" "$data_abort" "$pid1" \
    >>"$ART/stress-summary.tsv"
}

run_boot() {
  local boot="$1"
  local run_dir="$ART/boot-$boot"
  local start now elapsed last_log run_status

  rm -rf "$run_dir"
  mkdir -p "$run_dir"
  : >"$run_dir/run.out"
  VM_NAME=""
  RUN_PID=""

  log "boot-$boot: cleanup before launch"
  qemu_cleanup
  cleanup_old_breenix_vms
  rm -f "$SERIAL_LOG" "$SCREENSHOT_TMP"
  heartbeat

  log "boot-$boot: starting ./run.sh --parallels --no-build --test $WAIT_SECONDS"
  (
    cd "$ROOT" || exit 1
    ./run.sh --parallels --no-build --test "$WAIT_SECONDS"
  ) >"$run_dir/run.out" 2>&1 &
  RUN_PID=$!
  echo "$RUN_PID" >"$run_dir/run.pid"

  start="$(date +%s)"
  last_log="$start"
  run_status=124
  while true; do
    heartbeat
    VM_NAME="$(vm_name_from_run_out "$run_dir/run.out")"
    if [ -n "$VM_NAME" ]; then
      echo "$VM_NAME" >"$run_dir/vm-name.txt"
    fi
    if ! kill -0 "$RUN_PID" 2>/dev/null; then
      wait "$RUN_PID"
      run_status="$?"
      break
    fi
    now="$(date +%s)"
    elapsed=$((now - start))
    if [ "$elapsed" -ge "$RUN_TIMEOUT" ]; then
      log "boot-$boot: run timeout after ${elapsed}s"
      cleanup_run_process
      run_status=124
      break
    fi
    if [ $((now - last_log)) -ge 30 ]; then
      log "boot-$boot: monitor elapsed=${elapsed}s vm=${VM_NAME:-pending}"
      last_log="$now"
    fi
    sleep 1
  done

  if [ -f "$SERIAL_LOG" ]; then
    cp "$SERIAL_LOG" "$run_dir/parallels-boot.log"
  else
    : >"$run_dir/parallels-boot.log"
  fi
  cp "$run_dir/run.out" "$run_dir/parallels-run.out" 2>/dev/null || true
  if [ -f "$SCREENSHOT_TMP" ]; then
    cp "$SCREENSHOT_TMP" "$run_dir/parallels-screenshot.png" 2>/dev/null || true
  fi

  classify_boot "$boot" "$run_dir" "$run_status"
  log "boot-$boot: $(paste -sd ' ' "$run_dir/result.txt")"

  cleanup_run_process
  stop_and_delete_vm "$VM_NAME" "$run_dir"
  prlctl list --all >"$run_dir/prlctl-after.log" 2>&1 || true
  qemu_cleanup
  heartbeat
}

write_aggregate() {
  awk -F'\t' '
    NR == 1 { next }
    {
      count++;
      status = $3;
      msi = $8 + 0;
      irq = $9 + 0;
      delta = $10 + 0;
      stale_not_ready = $12 + 0;
      stale_current = $13 + 0;
      stale_deferred = $14 + 0;
      data_abort = $16 + 0;
      pid1 = $17;
      printf "boot-%s: %s run_status=%s activity=%s msi=%d irq=%d delta=%d stale_not_ready=%d stale_current=%d stale_deferred=%d data_abort=%d pid1=%s reason=%s\n",
        $1, status, $2, $5, msi, irq, delta, stale_not_ready, stale_current, stale_deferred, data_abort, pid1, $4;
      if (status != "pass") failures++;
      if (data_abort != 0) data_abort_boots++;
      if (delta > max_delta) max_delta = delta;
      stale_nr_sum += stale_not_ready;
      stale_cur_sum += stale_current;
      stale_def_sum += stale_deferred;
    }
    END {
      printf "overall: %s\n", failures ? "fail" : "pass";
      printf "boots: %d failures=%d data_abort_boots=%d max_msi_irq_delta=%d\n", count, failures + 0, data_abort_boots + 0, max_delta + 0;
      printf "scheduler_stale_totals: not_ready=%d current=%d deferred=%d\n", stale_nr_sum + 0, stale_cur_sum + 0, stale_def_sum + 0;
    }
  ' "$ART/metrics.tsv" >"$ART/aggregate-result.txt"
}

trap 'log "received signal"; cleanup_run_process; stop_and_delete_vm "${VM_NAME:-}" "$ART"; qemu_cleanup; exit 143' INT TERM HUP

log "starting Turn 8 20-boot scheduler gate boots=$BOOT_COUNT wait=${WAIT_SECONDS}s"
qemu_cleanup
: >"$ART/stress-summary.tsv"
for boot in $(seq 1 "$BOOT_COUNT"); do
  run_boot "$boot"
done
write_aggregate
cat "$ART/aggregate-result.txt"
qemu_cleanup
heartbeat
log "Turn 8 20-boot scheduler gate complete"
