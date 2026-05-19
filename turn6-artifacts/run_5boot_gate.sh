#!/usr/bin/env bash
set +e
set -u

ROOT="/Users/wrb/fun/code/breenix.worktrees/ahci-interrupt-driven"
CONTROL="/Users/wrb/Downloads/Ralph/breenix-ahci-interrupt-driven-1779178791"
ART="$ROOT/turn6-artifacts"
HB="$CONTROL/heartbeat"
SERIAL_LOG="/tmp/breenix-parallels-serial.log"
KERNEL_ELF="$ROOT/target/aarch64-breenix/release/kernel-aarch64"

RUN_PID=""
DBG_PID=""

mkdir -p "$ART"
: >"$ART/harness.log"
: >"$ART/metrics.tsv"
printf 'boot\tstatus\tahci_isr_count\tcpu0_ticks\tpeer_max\tcpu0_pct_of_max\tpost_poll_count\treason\n' >"$ART/metrics.tsv"

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

delete_breenix_vms() {
  local tmp vm
  tmp="$(mktemp)"
  prlctl list --all 2>/dev/null | awk 'NR > 1 && $NF ~ /^breenix-/ { print $NF }' >"$tmp" || true
  while IFS= read -r vm; do
    [ -n "$vm" ] || continue
    prlctl stop "$vm" --kill >>"$ART/prlctl-cleanup.log" 2>&1 || true
    prlctl delete "$vm" >>"$ART/prlctl-cleanup.log" 2>&1 || true
  done <"$tmp"
  rm -f "$tmp"
}

ensure_no_breenix_vms() {
  local attempt remaining
  for attempt in 1 2 3 4 5; do
    delete_breenix_vms
    sleep 1
    remaining="$(prlctl list --all 2>/dev/null | awk 'NR > 1 && $NF ~ /^breenix-/ { print $NF }' | xargs)"
    if [ -z "$remaining" ]; then
      return 0
    fi
    log "cleanup attempt $attempt still sees Breenix VMs: $remaining"
  done
  return 1
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
  if [ -n "${DBG_PID:-}" ]; then
    kill "$DBG_PID" >/dev/null 2>&1 || true
    wait "$DBG_PID" >/dev/null 2>&1 || true
    DBG_PID=""
  fi
}

vm_name_from_run_out() {
  local run_dir="$1"
  awk '/^VM name:[[:space:]]+breenix-/ { print $3 } /^VM:[[:space:]]+breenix-/ { print $2 }' "$run_dir/run.out" 2>/dev/null | tail -1
}

wait_for_port() {
  local port="$1"
  local i
  for i in $(seq 1 60); do
    heartbeat
    nc -z 127.0.0.1 "$port" >/dev/null 2>&1 && return 0
    sleep 1
  done
  return 1
}

sym_addr() {
  local pattern="$1"
  nm "$KERNEL_ELF" | awk -v p="$pattern" '$3 ~ p { print "0x" $1; exit }'
}

generate_gdb_script() {
  local run_dir="$1"
  local port="$2"
  local ahci_irq ahci_isr_count ahci_last_cpu polled polled_post timer_ticks timer_hw_ticks timer_total
  ahci_irq="$(sym_addr 'drivers4ahci8AHCI_IRQ17')"
  ahci_isr_count="$(sym_addr 'drivers4ahci14AHCI_ISR_COUNT17')"
  ahci_last_cpu="$(sym_addr 'drivers4ahci19AHCI_ISR_LAST_MPIDR17')"
  polled="$(sym_addr '^ahci_polled_completion_count$')"
  polled_post="$(sym_addr '^ahci_polled_post_registration_count$')"
  timer_ticks="$(sym_addr 'timer_interrupt16TIMER_TICK_COUNT17')"
  timer_hw_ticks="$(sym_addr 'timer_interrupt19TIMER_TICK_HW_COUNT17')"
  timer_total="$(sym_addr 'timer_interrupt21TIMER_INTERRUPT_COUNT17')"

  {
    echo "ahci_irq_addr=$ahci_irq"
    echo "ahci_isr_count_addr=$ahci_isr_count"
    echo "ahci_isr_last_mpidr_aff0_addr=$ahci_last_cpu"
    echo "ahci_polled_completion_count_addr=$polled"
    echo "ahci_polled_post_registration_count_addr=$polled_post"
    echo "timer_tick_count_addr=$timer_ticks"
    echo "timer_tick_hw_count_addr=$timer_hw_ticks"
    echo "timer_interrupt_count_addr=$timer_total"
  } >"$run_dir/symbol-addresses.txt"

  cat >"$run_dir/gdb-state.gdb" <<GDB
set pagination off
set confirm off
set architecture aarch64
set remotetimeout 10
set mem inaccessible-by-default off
set logging file $run_dir/gdb-state.log
set logging overwrite on
set logging enabled on

echo === TURN6 BOOT ENDPOINT STATE ===\\n
target remote 127.0.0.1:$port

set \$AHCI_IRQ = $ahci_irq
set \$AHCI_ISR_COUNT = $ahci_isr_count
set \$AHCI_ISR_LAST_MPIDR = $ahci_last_cpu
set \$AHCI_POLLED_COMPLETION_COUNT = $polled
set \$AHCI_POLLED_POST_REGISTRATION_COUNT = $polled_post
set \$TIMER_TICK_COUNT = $timer_ticks
set \$TIMER_TICK_HW_COUNT = $timer_hw_ticks
set \$TIMER_INTERRUPT_COUNT = $timer_total

printf "ahci_irq=%u\\n", *(unsigned int*)\$AHCI_IRQ
printf "ahci_isr_count=%u\\n", *(unsigned int*)\$AHCI_ISR_COUNT
printf "ahci_isr_last_mpidr_aff0=%lu\\n", *(unsigned long*)\$AHCI_ISR_LAST_MPIDR
printf "ahci_polled_completion_count=%lu\\n", *(unsigned long*)\$AHCI_POLLED_COMPLETION_COUNT
printf "ahci_polled_post_registration_count=%lu\\n", *(unsigned long*)\$AHCI_POLLED_POST_REGISTRATION_COUNT
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

start_guest_debugger() {
  local run_dir="$1"
  local vm_name="$2"
  local port="$3"

  generate_gdb_script "$run_dir" "$port"
  log "boot $(basename "$run_dir"): starting guest-debugger for $vm_name on port $port"
  prlctl guest-debugger "$vm_name" --port "$port" >"$run_dir/guest-debugger.out" 2>&1 &
  DBG_PID=$!

  if ! wait_for_port "$port"; then
    log "boot $(basename "$run_dir"): guest-debugger port did not open"
    echo "gdb_rc=guestdebugger-port-timeout" >"$run_dir/gdb-state.log"
    return 1
  fi
  return 0
}

capture_gdb() {
  local run_dir="$1"
  local vm_name="$2"
  local port="$3"

  if [ -z "${DBG_PID:-}" ] || ! kill -0 "$DBG_PID" 2>/dev/null || ! nc -z 127.0.0.1 "$port" >/dev/null 2>&1; then
    start_guest_debugger "$run_dir" "$vm_name" "$port" || return
  fi

  (
    cd "$run_dir" || exit 1
    gdb -nx -batch -x gdb-state.gdb "$KERNEL_ELF" >gdb-driver.out 2>&1
  ) &
  local gdb_pid=$!
  local start now
  start="$(date +%s)"
  while kill -0 "$gdb_pid" 2>/dev/null; do
    heartbeat
    now="$(date +%s)"
    if [ $((now - start)) -gt 120 ]; then
      log "boot $(basename "$run_dir"): GDB endpoint capture timed out"
      kill "$gdb_pid" >/dev/null 2>&1 || true
      sleep 2
      kill -9 "$gdb_pid" >/dev/null 2>&1 || true
      echo "gdb_rc=timeout" >>"$run_dir/gdb-state.log"
      return
    fi
    sleep 1
  done
  wait "$gdb_pid"
  echo "gdb_rc=$?" >>"$run_dir/gdb-state.log"

  if [ -n "${DBG_PID:-}" ]; then
    kill "$DBG_PID" >/dev/null 2>&1 || true
    wait "$DBG_PID" >/dev/null 2>&1 || true
    DBG_PID=""
  fi
}

gdb_value() {
  local file="$1"
  local key="$2"
  awk -F= -v key="$key" '$1 == key { print $2 }' "$file" 2>/dev/null | tail -1
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

evaluate_boot() {
  local run_dir="$1"
  local boot="$2"
  local gdb="$run_dir/gdb-state.log"
  local serial="$run_dir/serial.log"
  local ahci_irq ahci_isr cpu_aff0 total_poll post_poll cpu0 cpu1 cpu2 cpu3 peer_max pct gdb_rc timeout_count panic_count userspace reason status

  ahci_irq="$(gdb_value "$gdb" ahci_irq)"
  ahci_isr="$(gdb_value "$gdb" ahci_isr_count)"
  cpu_aff0="$(gdb_value "$gdb" ahci_isr_last_mpidr_aff0)"
  total_poll="$(gdb_value "$gdb" ahci_polled_completion_count)"
  post_poll="$(gdb_value "$gdb" ahci_polled_post_registration_count)"
  cpu0="$(gdb_value "$gdb" timer_tick_count_cpu0)"
  cpu1="$(gdb_value "$gdb" timer_tick_count_cpu1)"
  cpu2="$(gdb_value "$gdb" timer_tick_count_cpu2)"
  cpu3="$(gdb_value "$gdb" timer_tick_count_cpu3)"
  gdb_rc="$(gdb_value "$gdb" gdb_rc)"

  reason=""
  [ "$ahci_irq" = "34" ] || reason="$(append_reason "$reason" "ahci_irq=${ahci_irq:-missing}")"
  [ "${ahci_isr:-0}" -ge 100 ] 2>/dev/null || reason="$(append_reason "$reason" "ahci_isr_count=${ahci_isr:-missing}")"
  [ "$cpu_aff0" = "0" ] || reason="$(append_reason "$reason" "ahci_cpu=${cpu_aff0:-missing}")"
  [ "$post_poll" = "0" ] || reason="$(append_reason "$reason" "post_poll=${post_poll:-missing}")"
  [ "$gdb_rc" = "0" ] || reason="$(append_reason "$reason" "gdb_rc=${gdb_rc:-missing}")"

  peer_max="${cpu0:-0}"
  for value in "${cpu1:-0}" "${cpu2:-0}" "${cpu3:-0}"; do
    [ "$value" -gt "$peer_max" ] 2>/dev/null && peer_max="$value"
  done
  pct="$(awk -v c="${cpu0:-0}" -v m="$peer_max" 'BEGIN { if (m > 0) printf "%.2f", (100.0*c)/m; else printf "0.00" }')"
  awk -v c="${cpu0:-0}" -v m="$peer_max" 'BEGIN { exit !(m > 0 && c * 100 >= m * 90) }' || reason="$(append_reason "$reason" "cpu0_pct=$pct")"

  timeout_count="$(grep -aciE 'AHCI.*TIMEOUT|AHCI: command timeout|command timeout' "$serial" 2>/dev/null || true)"
  panic_count="$(grep -aciE 'KERNEL PANIC|panicked at|Data Abort|Synchronous exception|CPU0 REGRESSION ALARM' "$serial" 2>/dev/null || true)"
  userspace="$(grep -acF '[ OK ] syscall path verified' "$serial" 2>/dev/null || true)"
  [ "$timeout_count" = "0" ] || reason="$(append_reason "$reason" "ahci_timeouts=$timeout_count")"
  [ "$panic_count" = "0" ] || reason="$(append_reason "$reason" "panic_markers=$panic_count")"
  [ "$userspace" -gt 0 ] 2>/dev/null || reason="$(append_reason "$reason" "userspace_marker_missing")"

  if [ -z "$reason" ]; then
    status="pass"
    reason="-"
  else
    status="fail"
  fi

  {
    echo "boot=$boot"
    echo "status=$status"
    echo "reason=$reason"
    echo "ahci_irq=$ahci_irq"
    echo "ahci_isr_count=$ahci_isr"
    echo "ahci_isr_last_mpidr_aff0=$cpu_aff0"
    echo "ahci_polled_completion_count=$total_poll"
    echo "ahci_polled_post_registration_count=$post_poll"
    echo "timer_tick_count_cpu0=$cpu0"
    echo "timer_tick_count_cpu1=$cpu1"
    echo "timer_tick_count_cpu2=$cpu2"
    echo "timer_tick_count_cpu3=$cpu3"
    echo "peer_max=$peer_max"
    echo "cpu0_pct_of_max=$pct"
    echo "ahci_timeout_markers=$timeout_count"
    echo "panic_markers=$panic_count"
    echo "userspace_markers=$userspace"
  } >"$run_dir/result.txt"

  printf 'boot-%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$boot" "$status" "${ahci_isr:-0}" "${cpu0:-0}" "$peer_max" "$pct" "${post_poll:-missing}" "$reason" >>"$ART/metrics.tsv"
}

run_boot() {
  local boot="$1"
  local run_dir="$ART/boot-$boot"
  local port=$((9600 + boot))
  local vm_name wait_start now

  rm -rf "$run_dir"
  mkdir -p "$run_dir"
  : >"$run_dir/harness.log"
  log "boot-$boot: preparing clean environment"
  heartbeat
  cleanup_run
  qemu_cleanup
  if ! ensure_no_breenix_vms; then
    log "boot-$boot: failed to clean existing Breenix VMs"
    printf 'boot-%s\tfail\t0\t0\t0\t0.00\tmissing\tvm_cleanup_failed\n' "$boot" >>"$ART/metrics.tsv"
    echo "status=fail" >"$run_dir/result.txt"
    echo "reason=vm_cleanup_failed" >>"$run_dir/result.txt"
    return
  fi

  rm -f "$SERIAL_LOG"
  (
    cd "$ROOT" || exit 1
    ./run.sh --parallels
  ) >"$run_dir/run.out" 2>&1 &
  RUN_PID=$!

  wait_start="$(date +%s)"
  while true; do
    heartbeat
    vm_name="$(vm_name_from_run_out "$run_dir")"
  if [ -n "$vm_name" ]; then
      log "boot-$boot: detected VM $vm_name"
      break
    fi
    if ! kill -0 "$RUN_PID" 2>/dev/null; then
      wait "$RUN_PID"
      echo "status=fail" >"$run_dir/result.txt"
      echo "reason=run_script_exited_before_vm" >>"$run_dir/result.txt"
      printf 'boot-%s\tfail\t0\t0\t0\t0.00\tmissing\trun_script_exited_before_vm\n' "$boot" >>"$ART/metrics.tsv"
      cleanup_run
      ensure_no_breenix_vms
      return
    fi
    now="$(date +%s)"
    if [ $((now - wait_start)) -gt 420 ]; then
      echo "status=fail" >"$run_dir/result.txt"
      echo "reason=vm_create_timeout" >>"$run_dir/result.txt"
      printf 'boot-%s\tfail\t0\t0\t0\t0.00\tmissing\tvm_create_timeout\n' "$boot" >>"$ART/metrics.tsv"
      cleanup_run
      ensure_no_breenix_vms
      return
    fi
    sleep 1
  done

  start_guest_debugger "$run_dir" "$vm_name" "$port"

  log "boot-$boot: waiting 90s for boot and timer-regression window"
  for _ in $(seq 1 90); do
    heartbeat
    sleep 1
  done

  if [ -f "$SERIAL_LOG" ]; then
    cp "$SERIAL_LOG" "$run_dir/serial.log"
  else
    : >"$run_dir/serial.log"
  fi
  grep -aE 'AHCI|ahci|CPU0 timer|CPU0|timer regression|KERNEL PANIC|panicked at|TIMEOUT|SOFT LOCKUP|Data Abort|Synchronous exception|freeze-watch|bwm-fps|Frame #|syscall path verified' \
    "$run_dir/serial.log" >"$run_dir/signals.log" 2>/dev/null || true

  capture_gdb "$run_dir" "$vm_name" "$port"
  evaluate_boot "$run_dir" "$boot"

  cleanup_run
  ensure_no_breenix_vms
  prlctl list --all >"$run_dir/prlctl-after.log" 2>&1 || true
  heartbeat
}

write_aggregate() {
  awk -F '\t' '
    NR == 1 { next }
    {
      boot_count++
      if ($2 != "pass") {
        overall = "fail"
      }
      if (boot_count == 1 || $3 < min_isr) min_isr = $3
      if (boot_count == 1 || $3 > max_isr) max_isr = $3
      sum_isr += $3
      if (boot_count == 1 || $6 < min_pct) min_pct = $6
      if (boot_count == 1 || $6 > max_pct) max_pct = $6
      sum_pct += $6
      post = ($7 == "missing" ? -1 : $7)
      if (boot_count == 1 || post > max_post) max_post = post
      boot_line[boot_count] = $1 ": " $2 " reason=" $8
    }
    END {
      if (overall == "") overall = "pass"
      for (i = 1; i <= boot_count; i++) print boot_line[i]
      printf "overall: %s\n", overall
      if (boot_count > 0) {
        printf "ahci_isr_count: min=%d, max=%d, mean=%.2f\n", min_isr, max_isr, sum_isr / boot_count
        printf "cpu0_pct_of_max: min=%.2f, max=%.2f, mean=%.2f\n", min_pct, max_pct, sum_pct / boot_count
        printf "ahci_polled_post_registration_count: max across all boots = %d\n", max_post
      } else {
        print "ahci_isr_count: no boots"
        print "cpu0_pct_of_max: no boots"
        print "ahci_polled_post_registration_count: no boots"
      }
    }
  ' "$ART/metrics.tsv" >"$ART/aggregate-result.txt"
}

finish() {
  cleanup_run
  ensure_no_breenix_vms
  qemu_cleanup
  write_aggregate
  heartbeat
}

trap 'log "received signal"; finish; exit 143' INT TERM HUP

log "starting Turn 6 5-boot Parallels gate"
for boot in 1 2 3 4 5; do
  run_boot "$boot"
done
finish
log "Turn 6 5-boot Parallels gate complete"
