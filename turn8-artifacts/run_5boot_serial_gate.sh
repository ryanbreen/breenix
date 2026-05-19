#!/usr/bin/env bash
set +e
set -u

ROOT="/Users/wrb/fun/code/breenix.worktrees/ahci-interrupt-driven"
CONTROL="/Users/wrb/Downloads/Ralph/breenix-ahci-interrupt-driven-1779178791"
ART="$ROOT/turn8-artifacts"
HB="$CONTROL/heartbeat"
SERIAL_LOG="/tmp/breenix-parallels-serial.log"

RUN_PID=""
VM_NAME=""

mkdir -p "$ART"
: >"$ART/harness.log"
: >"$ART/vm-names.txt"
printf 'boot\tstatus\ttotal\tpost_reg\tpre_sched\tpost_sched\tcpu0_pct\tahci_spi\tuserspace\ttimeouts\tpanics\tcpu0_alarm\treason\n' >"$ART/metrics.tsv"

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
  local timeout="$1"
  shift
  "$@" &
  local pid=$!
  local start now
  start="$(date +%s)"
  while kill -0 "$pid" 2>/dev/null; do
    heartbeat
    now="$(date +%s)"
    if [ $((now - start)) -ge "$timeout" ]; then
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

stop_and_delete_vm() {
  local vm="$1"
  local run_dir="$2"
  [ -n "$vm" ] || return 0
  run_with_timeout 60 prlctl stop "$vm" --kill >>"$run_dir/prlctl-cleanup.log" 2>&1 || true
  run_with_timeout 60 prlctl delete "$vm" >>"$run_dir/prlctl-cleanup.log" 2>&1 || true
}

cleanup_run() {
  if [ -n "${RUN_PID:-}" ]; then
    pkill -P "$RUN_PID" >/dev/null 2>&1 || true
    kill "$RUN_PID" >/dev/null 2>&1 || true
    wait "$RUN_PID" >/dev/null 2>&1 || true
    RUN_PID=""
  fi
  pkill -f "/bin/bash ./run.sh --parallels" >/dev/null 2>&1 || true
  pkill -f "tail -f $SERIAL_LOG" >/dev/null 2>&1 || true
}

vm_name_from_run_out() {
  local run_dir="$1"
  awk '/^VM name:[[:space:]]+breenix-/ { print $3 } /^VM:[[:space:]]+breenix-/ { print $2 }' "$run_dir/run.out" 2>/dev/null | tail -1
}

extract_kv() {
  local key="$1"
  local line="$2"
  printf '%s\n' "$line" | sed -nE "s/.*${key}=([0-9]+).*/\\1/p"
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

cpu0_pct_from_serial() {
  local serial="$1"
  local line cpu0 cpu1 cpu2 cpu3 cpu4 cpu5 cpu6 cpu7 max_peer
  line="$(grep -a '\[freeze-watch\].*timer_ticks_cpu0=' "$serial" 2>/dev/null | tail -1)"
  [ -n "$line" ] || return 0
  cpu0="$(extract_kv timer_ticks_cpu0 "$line")"
  cpu1="$(extract_kv timer_ticks_cpu1 "$line")"
  cpu2="$(extract_kv timer_ticks_cpu2 "$line")"
  cpu3="$(extract_kv timer_ticks_cpu3 "$line")"
  cpu4="$(extract_kv timer_ticks_cpu4 "$line")"
  cpu5="$(extract_kv timer_ticks_cpu5 "$line")"
  cpu6="$(extract_kv timer_ticks_cpu6 "$line")"
  cpu7="$(extract_kv timer_ticks_cpu7 "$line")"
  [ -n "$cpu0" ] || return 0
  max_peer="$cpu1"
  for value in "$cpu2" "$cpu3" "$cpu4" "$cpu5" "$cpu6" "$cpu7"; do
    [ -n "$value" ] || continue
    if [ "$value" -gt "$max_peer" ]; then
      max_peer="$value"
    fi
  done
  [ -n "$max_peer" ] && [ "$max_peer" -gt 0 ] || return 0
  awk -v c="$cpu0" -v m="$max_peer" 'BEGIN { printf "%.2f", (c * 100.0) / m }'
}

evaluate_boot() {
  local boot="$1"
  local run_dir="$2"
  local serial="$run_dir/serial.log"
  local line total post_reg pre_sched post_sched cpu0_pct
  local ahci_spi userspace timeouts panics cpu0_alarm status reason

  line="$(grep -a '\[ahci-poll-attrib\]' "$serial" 2>/dev/null | tail -1)"
  total="$(extract_kv total "$line")"
  post_reg="$(extract_kv post_reg "$line")"
  pre_sched="$(extract_kv pre_sched "$line")"
  post_sched="$(extract_kv post_sched "$line")"
  cpu0_pct="$(cpu0_pct_from_serial "$serial")"

  grep -aq '\[ahci\] Platform IRQ enabled: SPI 34 (wired, level-triggered, CPU0)' "$serial" 2>/dev/null
  ahci_spi=$?
  grep -aq '\[ OK \] syscall path verified' "$serial" 2>/dev/null
  userspace=$?
  timeouts="$(grep -aciE 'AHCI.*TIMEOUT|AHCI: command timeout|command timeout' "$serial" 2>/dev/null || true)"
  panics="$(grep -aciE 'KERNEL PANIC|panicked at|Data Abort|Synchronous exception' "$serial" 2>/dev/null || true)"
  cpu0_alarm="$(grep -aciE 'CPU0 REGRESSION ALARM|CPU0 timer regression' "$serial" 2>/dev/null || true)"

  status="pass"
  reason=""
  if [ -z "$line" ]; then
    status="fail"
    reason="$(append_reason "$reason" "missing-attrib-line")"
  fi
  if [ "${post_sched:-missing}" != "0" ]; then
    status="fail"
    reason="$(append_reason "$reason" "post_sched=${post_sched:-missing}")"
  fi
  if [ -z "${pre_sched:-}" ] || [ "$pre_sched" -lt 65 ] || [ "$pre_sched" -gt 75 ]; then
    status="fail"
    reason="$(append_reason "$reason" "pre_sched=${pre_sched:-missing}")"
  fi
  if [ "$ahci_spi" -ne 0 ]; then
    status="fail"
    reason="$(append_reason "$reason" "missing-ahci-spi34")"
  fi
  if [ "$userspace" -ne 0 ]; then
    status="fail"
    reason="$(append_reason "$reason" "missing-userspace-marker")"
  fi
  if [ "${timeouts:-0}" -ne 0 ]; then
    status="fail"
    reason="$(append_reason "$reason" "timeouts=$timeouts")"
  fi
  if [ "${panics:-0}" -ne 0 ]; then
    status="fail"
    reason="$(append_reason "$reason" "panics=$panics")"
  fi
  if [ "${cpu0_alarm:-0}" -ne 0 ]; then
    status="fail"
    reason="$(append_reason "$reason" "cpu0_alarm=$cpu0_alarm")"
  fi

  [ -n "$reason" ] || reason="-"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$boot" "$status" "${total:-}" "${post_reg:-}" "${pre_sched:-}" "${post_sched:-}" \
    "${cpu0_pct:-}" "$ahci_spi" "$userspace" "${timeouts:-0}" "${panics:-0}" "${cpu0_alarm:-0}" "$reason" \
    >>"$ART/metrics.tsv"

  {
    echo "boot=$boot"
    echo "status=$status"
    echo "vm_name=$VM_NAME"
    echo "attrib_line=$line"
    echo "total=${total:-}"
    echo "post_reg=${post_reg:-}"
    echo "pre_sched=${pre_sched:-}"
    echo "post_sched=${post_sched:-}"
    echo "cpu0_pct=${cpu0_pct:-}"
    echo "ahci_spi_rc=$ahci_spi"
    echo "userspace_rc=$userspace"
    echo "timeouts=${timeouts:-0}"
    echo "panics=${panics:-0}"
    echo "cpu0_alarm=${cpu0_alarm:-0}"
    echo "reason=$reason"
  } >"$run_dir/result.txt"
}

run_boot() {
  local boot="$1"
  local run_dir="$ART/boot-$boot"
  rm -rf "$run_dir"
  mkdir -p "$run_dir"
  VM_NAME=""
  RUN_PID=""

  log "boot $boot: starting fresh Parallels VM"
  qemu_cleanup
  rm -f "$SERIAL_LOG"

  (
    cd "$ROOT" || exit 1
    ./run.sh --parallels
  ) >"$run_dir/run.out" 2>&1 &
  RUN_PID=$!
  echo "$RUN_PID" >"$run_dir/run.pid"

  local start now
  start="$(date +%s)"
  while true; do
    heartbeat
    VM_NAME="$(vm_name_from_run_out "$run_dir")"
    if [ -n "$VM_NAME" ]; then
      echo "$VM_NAME" >>"$ART/vm-names.txt"
      log "boot $boot: detected VM $VM_NAME"
      break
    fi
    if ! kill -0 "$RUN_PID" 2>/dev/null; then
      wait "$RUN_PID"
      echo "run_script_rc=$?" >"$run_dir/result.txt"
      log "boot $boot: run.sh exited before VM creation"
      cleanup_run
      evaluate_boot "$boot" "$run_dir"
      return
    fi
    now="$(date +%s)"
    if [ $((now - start)) -gt 420 ]; then
      echo "run_script_rc=vm-create-timeout" >"$run_dir/result.txt"
      log "boot $boot: timed out waiting for VM creation"
      cleanup_run
      evaluate_boot "$boot" "$run_dir"
      return
    fi
    sleep 1
  done

  log "boot $boot: waiting 90s for scheduler-ready attribution and health window"
  for _ in $(seq 1 90); do
    heartbeat
    sleep 1
  done

  if [ -f "$SERIAL_LOG" ]; then
    cp "$SERIAL_LOG" "$run_dir/serial.log"
  else
    : >"$run_dir/serial.log"
  fi
  grep -aE 'ahci-poll-attrib|Platform IRQ enabled|syscall path verified|AHCI.*TIMEOUT|AHCI: command timeout|KERNEL PANIC|panicked at|Data Abort|Synchronous exception|CPU0 REGRESSION ALARM|CPU0 timer regression|freeze-watch' \
    "$run_dir/serial.log" >"$run_dir/signals.log" 2>/dev/null || true

  evaluate_boot "$boot" "$run_dir"
  cleanup_run
  stop_and_delete_vm "$VM_NAME" "$run_dir"
  prlctl list --all >"$run_dir/prlctl-after.log" 2>&1 || true
  qemu_cleanup
  heartbeat
}

write_aggregate() {
  awk -F'\t' '
    NR == 1 { next }
    {
      boot=$1; status=$2; total=$3; post_reg=$4; pre_sched=$5; post_sched=$6; cpu0_pct=$7; reason=$13
      printf "boot-%s: %s total=%s post_reg=%s pre_sched=%s post_sched=%s cpu0_pct=%s reason=%s\n", boot, status, total, post_reg, pre_sched, post_sched, cpu0_pct, reason
      if (status != "pass") failures++
      if (post_sched != "" && post_sched + 0 > max_post_sched) max_post_sched = post_sched + 0
      if (pre_sched != "") {
        if (pre_values == "") pre_values = pre_sched; else pre_values = pre_values "," pre_sched
      }
    }
    END {
      printf "overall: %s\n", failures ? "fail" : "pass"
      printf "post_sched: max across all boots = %d\n", max_post_sched
      printf "pre_sched: distribution = %s\n", pre_values
    }
  ' "$ART/metrics.tsv" >"$ART/aggregate-result.txt"
}

trap 'log "received signal"; cleanup_run; stop_and_delete_vm "${VM_NAME:-}" "$ART"; qemu_cleanup; exit 143' INT TERM HUP

log "starting Turn 8 5-boot serial gate"
for boot in 1 2 3 4 5; do
  run_boot "$boot"
done
write_aggregate
cat "$ART/aggregate-result.txt"
log "Turn 8 5-boot serial gate complete"
