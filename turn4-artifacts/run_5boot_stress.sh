#!/usr/bin/env bash
set +e
set -u

ROOT="/Users/wrb/fun/code/breenix.worktrees/virtio-gpu-interrupt-driven"
CONTROL="/Users/wrb/Downloads/Ralph/breenix-virtio-gpu-interrupt-driven-1779178791"
AHCI_CONTROL="/Users/wrb/Downloads/Ralph/breenix-ahci-interrupt-driven-1779178791"
ART="$ROOT/turn4-artifacts"
HB="$CONTROL/heartbeat"
SERIAL_LOG="/tmp/breenix-parallels-serial.log"
SCREENSHOT_TMP="/tmp/breenix-screenshot.png"
KERNEL_ELF="$ROOT/target/aarch64-breenix/release/kernel-aarch64"

BOOT_COUNT=5
ACTIVE_SECONDS=220
BOOT_PROGRESS_TIMEOUT=180
PRL_TIMEOUT=30
COORDINATION_WAIT_SECONDS=1800

for arg in "$@"; do
  case "$arg" in
    boots=*) BOOT_COUNT="${arg#boots=}" ;;
    active=*) ACTIVE_SECONDS="${arg#active=}" ;;
    progress-timeout=*) BOOT_PROGRESS_TIMEOUT="${arg#progress-timeout=}" ;;
    *)
      echo "unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

mkdir -p "$ART"
HARNESS_LOG="$ART/harness.log"
METRICS="$ART/metrics.tsv"
AGG="$ART/aggregate-result.txt"
rm -rf "$ART"/boot-*
: >"$HARNESS_LOG"
printf 'boot\tstatus\treason\tmax_uptime_ms\tlast_completion_ms\tfinal_fps\tfinal_completes\trescue_tid13\tstuck_tid13\tsoftlock\tcpu0_regression\tfar_0xccd\tpanic_or_exception\tpoll_markers\tmax_busy_ms\tserial_bytes\tvm\n' >"$METRICS"

RUN_PID=""
VM_NAME=""

log() {
  printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$HARNESS_LOG"
}

heartbeat() {
  touch "$HB"
}

qemu_cleanup() {
  pkill -9 qemu-system-x86 2>/dev/null || true
  killall -9 qemu-system-x86_64 2>/dev/null || true
}

coordination_snapshot() {
  local out_file="$1"
  local err_file="$2"
  : >"$out_file"
  : >"$err_file"
  timeout -k 5 "$PRL_TIMEOUT" prlctl list -a -o name,status >"$out_file" 2>"$err_file"
}

wait_for_coordination_gate() {
  local boot_num="$1"
  local boot_dir="$ART/boot-$boot_num"
  local coord_log="$boot_dir/coordination.log"
  local prl_out="$boot_dir/prlctl-coordination.out"
  local prl_err="$boot_dir/prlctl-coordination.err"
  local start now elapsed ahci_state prl_rc breenix_lines active_lines
  mkdir -p "$boot_dir"
  : >"$coord_log"
  start="$(date +%s)"

  while true; do
    heartbeat
    ahci_state="$(cat "$AHCI_CONTROL/state.txt" 2>/dev/null || echo MISSING)"
    coordination_snapshot "$prl_out" "$prl_err"
    prl_rc="$?"
    breenix_lines="$(awk 'NR > 1 && $1 ~ /^breenix-/ { print }' "$prl_out" 2>/dev/null)"
    active_lines="$(printf '%s\n' "$breenix_lines" | awk '$2 == "running" || $2 == "stopping" { print }')"
    now="$(date +%s)"
    elapsed=$((now - start))
    {
      printf '[%s] elapsed=%ss ahci_state=%s prl_rc=%s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$elapsed" "$ahci_state" "$prl_rc"
      if [ -n "$breenix_lines" ]; then
        printf 'breenix_vms:\n%s\n' "$breenix_lines"
      else
        printf 'breenix_vms: none\n'
      fi
      if [ -s "$prl_err" ]; then
        printf 'prlctl_stderr:\n'
        cat "$prl_err"
      fi
    } >>"$coord_log"

    case "$ahci_state" in
      AWAITING_REVIEW|STOP|MISSING)
        if [ "$prl_rc" -eq 0 ] && [ -z "$breenix_lines" ] && [ -z "$active_lines" ]; then
          log "boot-$boot_num: coordination gate open ahci_state=$ahci_state"
          return 0
        fi
        ;;
    esac

    if [ "$elapsed" -ge "$COORDINATION_WAIT_SECONDS" ]; then
      log "boot-$boot_num: coordination gate timeout ahci_state=$ahci_state"
      return 1
    fi
    sleep 120
  done
}

cleanup_run_process() {
  if [ -n "${RUN_PID:-}" ]; then
    pkill -P "$RUN_PID" >/dev/null 2>&1 || true
    kill "$RUN_PID" >/dev/null 2>&1 || true
    wait "$RUN_PID" >/dev/null 2>&1 || true
  fi
  pkill -f "tail -f $SERIAL_LOG" >/dev/null 2>&1 || true
  pkill -f "/bin/bash ./run.sh --parallels" >/dev/null 2>&1 || true
}

vm_name_from_run_out() {
  local run_out="$1"
  awk '/^VM name:[[:space:]]+breenix-/ { print $3 } /^VM:[[:space:]]+breenix-/ { print $2 }' "$run_out" 2>/dev/null | tail -1
}

count_pattern() {
  local pattern="$1"
  local file="$2"
  grep -Eai "$pattern" "$file" 2>/dev/null | wc -l | tr -d ' '
}

last_freeze_line() {
  local file="$1"
  grep -a '\[freeze-watch\]' "$file" 2>/dev/null | tail -1
}

max_key_value() {
  local key="$1"
  local file="$2"
  grep -Eao "${key}=[0-9]+" "$file" 2>/dev/null \
    | cut -d= -f2 \
    | sort -n \
    | tail -1
}

line_key_value() {
  local key="$1"
  local line="$2"
  printf '%s\n' "$line" | grep -Eo "${key}=[0-9]+" | tail -1 | cut -d= -f2
}

max_busy_window_ms() {
  local file="$1"
  awk '
    /\[freeze-watch\]/ {
      uptime = ""; lock = "";
      for (i = 1; i <= NF; i++) {
        if ($i ~ /^uptime_ms=/) {
          split($i, a, "="); uptime = a[2] + 0;
        }
        if ($i ~ /^gpu_pci_lock=/) {
          split($i, b, "="); lock = b[2];
        }
      }
      if (uptime == "" || lock == "") {
        next;
      }
      last = uptime;
      if (lock == "busy") {
        if (busy_start == "") {
          busy_start = uptime;
        }
      } else if (lock == "ok" && busy_start != "") {
        dur = uptime - busy_start;
        if (dur > max) {
          max = dur;
        }
        busy_start = "";
      }
    }
    END {
      if (busy_start != "" && last != "") {
        dur = last - busy_start;
        if (dur > max) {
          max = dur;
        }
      }
      print max + 0;
    }
  ' "$file" 2>/dev/null
}

capture_gdb_state() {
  local boot_dir="$1"
  local boot_num="$2"
  local vm="$3"
  local serial="$4"
  local gdb_state="$boot_dir/gdb-state.log"
  local port=$((9600 + boot_num))
  local final_freeze
  final_freeze="$(last_freeze_line "$serial")"

  {
    echo "vm=$vm"
    echo "port=$port"
    echo "kernel=$KERNEL_ELF"
    echo "final_freeze=$final_freeze"
    echo
    echo "nm symbols:"
    nm "$KERNEL_ELF" 2>/dev/null | grep -E 'VIRTGPU_(SUBMIT_TOTAL|COMPLETE_TOTAL|FAIL_TOTAL|WAIT_TIMEOUT_COUNT|LAST_COMPLETION_MS)|GPU_(IRQ|CONFIG_IRQ|COMPLETED_USED_IDX)' || true
    echo
    echo "guest-debugger:"
  } >"$gdb_state"

  timeout -k 5 "$PRL_TIMEOUT" prlctl guest-debugger "$vm" --port "$port" >>"$boot_dir/guest-debugger.log" 2>&1 &
  local dbg_pid=$!
  local opened=0
  local i
  for i in $(seq 1 20); do
    heartbeat
    if nc -z 127.0.0.1 "$port" >/dev/null 2>&1; then
      opened=1
      break
    fi
    sleep 1
  done

  if [ "$opened" -eq 1 ]; then
    cat >"$boot_dir/gdb-commands.txt" <<GDBEOF
set pagination off
target remote 127.0.0.1:$port
info registers pc sp
p/x VIRTGPU_SUBMIT_TOTAL
p/x VIRTGPU_COMPLETE_TOTAL
p/x VIRTGPU_FAIL_TOTAL
p/x VIRTGPU_WAIT_TIMEOUT_COUNT
p/x VIRTGPU_LAST_COMPLETION_MS
detach
quit
GDBEOF
    timeout -k 5 45 gdb -nx -batch -x "$boot_dir/gdb-commands.txt" "$KERNEL_ELF" >>"$gdb_state" 2>&1
    echo "gdb_exit=$?" >>"$gdb_state"
  else
    echo "guest_debugger_port=timeout" >>"$gdb_state"
  fi

  kill "$dbg_pid" >/dev/null 2>&1 || true
  wait "$dbg_pid" >/dev/null 2>&1 || true
}

classify_boot() {
  local boot_dir="$1"
  local serial="$boot_dir/serial.log"
  local result="$boot_dir/result.txt"
  local final_line max_uptime last_completion final_fps final_completes
  local rescue stuck softlock cpu0 far panic poll max_busy serial_bytes
  local status="pass"
  local reason="pass"

  final_line="$(last_freeze_line "$serial")"
  max_uptime="$(max_key_value uptime_ms "$serial")"
  last_completion="$(max_key_value last_completion_ms "$serial")"
  final_fps="$(line_key_value fps_last_5s "$final_line")"
  final_completes="$(line_key_value completes "$final_line")"
  rescue="$(count_pattern 'rescue_tid=13' "$serial")"
  stuck="$(count_pattern '\bstuck_tid=13\b' "$serial")"
  softlock="$(count_pattern 'SOFT_LOCK|softlock|SOFTLOCK' "$serial")"
  cpu0="$(count_pattern 'CPU0 REGRESSION|CPU0 tick_count.*peer max' "$serial")"
  far="$(count_pattern 'FAR=0x0*ccd|FAR=0xccd' "$serial")"
  panic="$(count_pattern 'KERNEL PANIC|Synchronous exception|Data Abort' "$serial")"
  poll="$(count_pattern 'virtio.*poll|gpu.*poll|fence.*poll|used\.idx.*poll|polling loop' "$serial")"
  max_busy="$(max_busy_window_ms "$serial")"
  serial_bytes="$(wc -c <"$serial" 2>/dev/null || echo 0)"

  max_uptime="${max_uptime:-0}"
  last_completion="${last_completion:-0}"
  final_fps="${final_fps:-0}"
  final_completes="${final_completes:-0}"
  rescue="${rescue:-0}"
  stuck="${stuck:-0}"
  softlock="${softlock:-0}"
  cpu0="${cpu0:-0}"
  far="${far:-0}"
  panic="${panic:-0}"
  poll="${poll:-0}"
  max_busy="${max_busy:-0}"

  if [ "$max_uptime" -lt 220000 ]; then
    status="fail"; reason="max_uptime_lt_220000"
  elif [ "$last_completion" -lt 200000 ]; then
    status="fail"; reason="last_completion_lt_200000"
  elif [ "$stuck" -ne 0 ]; then
    status="fail"; reason="stuck_tid13"
  elif [ "$softlock" -ne 0 ]; then
    status="fail"; reason="softlock"
  elif [ "$cpu0" -ne 0 ]; then
    status="fail"; reason="cpu0_regression"
  elif [ "$far" -ne 0 ]; then
    status="fail"; reason="far_0xccd"
  elif [ "$panic" -ne 0 ]; then
    status="fail"; reason="panic_or_exception"
  elif [ "$max_busy" -gt 5500 ]; then
    status="fail"; reason="gpu_pci_lock_busy_gt_5s"
  elif [ "$poll" -ne 0 ]; then
    status="fail"; reason="poll_marker"
  elif [ "$final_fps" -lt 60 ]; then
    status="fail"; reason="final_fps_lt_60"
  elif [ "$final_completes" -lt 10000 ]; then
    status="fail"; reason="final_completes_lt_10000"
  elif [ "$rescue" -gt 10 ]; then
    status="fail"; reason="rescue_tid13_gt_10"
  fi

  {
    echo "status=$status"
    echo "reason=$reason"
    echo "max_uptime_ms=$max_uptime"
    echo "last_completion_ms=$last_completion"
    echo "final_fps=$final_fps"
    echo "final_completes=$final_completes"
    echo "rescue_tid13=$rescue"
    echo "stuck_tid13=$stuck"
    echo "softlock=$softlock"
    echo "cpu0_regression=$cpu0"
    echo "far_0xccd=$far"
    echo "panic_or_exception=$panic"
    echo "poll_markers=$poll"
    echo "max_busy_ms=$max_busy"
    echo "serial_bytes=$serial_bytes"
    echo "vm=$VM_NAME"
    echo "final_freeze=$final_line"
  } >"$result"

  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$(basename "$boot_dir")" "$status" "$reason" "$max_uptime" "$last_completion" \
    "$final_fps" "$final_completes" "$rescue" "$stuck" "$softlock" "$cpu0" "$far" \
    "$panic" "$poll" "$max_busy" "$serial_bytes" "$VM_NAME" >>"$METRICS"
}

run_boot() {
  local boot_num="$1"
  local boot_dir="$ART/boot-$boot_num"
  local start now elapsed active_start active_elapsed progress_seen last_log serial_size
  mkdir -p "$boot_dir"
  : >"$boot_dir/harness.log"
  : >"$SERIAL_LOG"
  rm -f "$SCREENSHOT_TMP"
  VM_NAME=""
  RUN_PID=""

  log "boot-$boot_num: waiting for coordination gate"
  if ! wait_for_coordination_gate "$boot_num"; then
    {
      echo "status=blocked"
      echo "reason=coordination_gate_timeout"
      echo "vm="
    } >"$boot_dir/result.txt"
    printf 'boot-%s\tblocked\tcoordination_gate_timeout\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t\n' "$boot_num" >>"$METRICS"
    return
  fi

  log "boot-$boot_num: cleanup before launch"
  qemu_cleanup
  heartbeat

  log "boot-$boot_num: starting ./run.sh --parallels --no-build"
  (
    cd "$ROOT" || exit 1
    ./run.sh --parallels --no-build
  ) >"$boot_dir/run.out" 2>&1 &
  RUN_PID=$!
  echo "$RUN_PID" >"$boot_dir/run.pid"

  start="$(date +%s)"
  while true; do
    heartbeat
    VM_NAME="$(vm_name_from_run_out "$boot_dir/run.out")"
    if [ -n "$VM_NAME" ]; then
      log "boot-$boot_num: detected VM $VM_NAME"
      echo "$VM_NAME" >"$boot_dir/vm-name.txt"
      break
    fi
    if ! kill -0 "$RUN_PID" 2>/dev/null; then
      log "boot-$boot_num: run.sh exited before VM name"
      break
    fi
    now="$(date +%s)"
    if [ $((now - start)) -gt 300 ]; then
      log "boot-$boot_num: VM name timeout"
      break
    fi
    sleep 1
  done

  log "boot-$boot_num: waiting for VM start/serial stream"
  start="$(date +%s)"
  while true; do
    heartbeat
    if grep -qaE '^Serial:|^Tailing serial output|Breenix running on Parallels' "$boot_dir/run.out" 2>/dev/null; then
      break
    fi
    if [ -s "$SERIAL_LOG" ] || grep -qaE '\[virgl-composite\] Frame #|\[bwm-fps\]|\[freeze-watch\]' "$boot_dir/run.out" 2>/dev/null; then
      break
    fi
    if ! kill -0 "$RUN_PID" 2>/dev/null && [ ! -s "$SERIAL_LOG" ] && ! grep -qaE '\[virgl-composite\] Frame #|\[bwm-fps\]|\[freeze-watch\]' "$boot_dir/run.out" 2>/dev/null; then
      log "boot-$boot_num: run.sh exited before serial stream"
      break
    fi
    now="$(date +%s)"
    if [ $((now - start)) -gt 300 ]; then
      log "boot-$boot_num: serial stream timeout"
      break
    fi
    sleep 1
  done

  start="$(date +%s)"
  active_start=""
  progress_seen=0
  last_log="$start"
  while true; do
    heartbeat
    now="$(date +%s)"
    elapsed=$((now - start))
    active_elapsed=0
    if [ -n "$active_start" ]; then
      active_elapsed=$((now - active_start))
    fi

    if grep -qaE '\[virgl-composite\] Frame #|\[bwm-fps\]|\[freeze-watch\]' "$boot_dir/run.out" 2>/dev/null; then
      progress_seen=1
      if [ -z "$active_start" ]; then
        active_start="$now"
        log "boot-$boot_num: active rendering detected"
      fi
    fi

    if grep -qaE 'FAR=0x0*ccd|FAR=0xccd|KERNEL PANIC|Data Abort|Synchronous exception|SOFT_LOCK|softlock|SOFTLOCK|CPU0 REGRESSION|CPU0 tick_count.*peer max|\bstuck_tid=13\b' "$boot_dir/run.out" 2>/dev/null; then
      log "boot-$boot_num: early failure marker detected"
      break
    fi

    if [ -n "$active_start" ] && [ "$active_elapsed" -ge "$ACTIVE_SECONDS" ]; then
      log "boot-$boot_num: active window complete (${ACTIVE_SECONDS}s)"
      break
    fi

    if [ -z "$active_start" ] && [ "$elapsed" -ge "$BOOT_PROGRESS_TIMEOUT" ]; then
      log "boot-$boot_num: no active rendering within ${BOOT_PROGRESS_TIMEOUT}s"
      break
    fi

    if [ $((now - last_log)) -ge 30 ]; then
      serial_size=0
      serial_size="$(stat -f%z "$boot_dir/run.out" 2>/dev/null || echo 0)"
      log "boot-$boot_num: monitor elapsed=${elapsed}s active=${active_elapsed}s size=${serial_size} progress=${progress_seen}"
      last_log="$now"
    fi
    sleep 1
  done

  cp "$boot_dir/run.out" "$boot_dir/serial.log"
  if [ -s "$SERIAL_LOG" ]; then
    cp "$SERIAL_LOG" "$boot_dir/raw-serial-file.log"
  fi
  tail -160 "$boot_dir/serial.log" >"$boot_dir/tail160.log" 2>/dev/null || true
  grep -aE 'FAR=0x|SOFT_LOCK|softlock|SOFTLOCK|CPU0 REGRESSION|KERNEL PANIC|Data Abort|Synchronous exception|\bstuck_tid=13\b|rescue_tid=13|\[freeze-watch\]|\[bwm-fps\]' \
    "$boot_dir/serial.log" >"$boot_dir/signals.log" 2>/dev/null || true

  if [ -n "$VM_NAME" ]; then
    timeout -k 5 "$PRL_TIMEOUT" prlctl capture "$VM_NAME" --file "$boot_dir/screenshot.png" >>"$boot_dir/prlctl-capture.log" 2>&1 || true
    capture_gdb_state "$boot_dir" "$boot_num" "$VM_NAME" "$boot_dir/serial.log"
  else
    echo "vm=unknown" >"$boot_dir/gdb-state.log"
  fi

  classify_boot "$boot_dir"
  log "boot-$boot_num: $(cat "$boot_dir/result.txt" | tr '\n' ' ')"

  cleanup_run_process
  if [ -n "$VM_NAME" ]; then
    timeout -k 5 "$PRL_TIMEOUT" prlctl stop "$VM_NAME" --kill >>"$boot_dir/prlctl-stop.log" 2>&1 || true
    timeout -k 5 "$PRL_TIMEOUT" prlctl delete "$VM_NAME" >>"$boot_dir/prlctl-delete.log" 2>&1 || true
  fi
  qemu_cleanup
}

write_aggregate() {
  awk -F'\t' '
    NR == 1 { next }
    {
      boot[NR - 1] = $1;
      status[NR - 1] = $2;
      reason[NR - 1] = $3;
      fps[NR - 1] = $6 + 0;
      completes[NR - 1] = $7 + 0;
      rescue[NR - 1] = $8 + 0;
      if ($2 != "pass") {
        overall = "fail";
      }
      count++;
    }
    END {
      if (overall == "") {
        overall = "pass";
      }
      for (i = 1; i <= count; i++) {
        printf "%s: %s", boot[i], status[i];
        if (status[i] != "pass") {
          printf " + %s", reason[i];
        }
        printf "\n";
      }
      printf "overall: %s\n", overall;
      if (count > 0) {
        min_fps = max_fps = fps[1];
        min_comp = max_comp = completes[1];
        min_rescue = max_rescue = rescue[1];
        sum_fps = sum_comp = sum_rescue = 0;
        for (i = 1; i <= count; i++) {
          if (fps[i] < min_fps) min_fps = fps[i];
          if (fps[i] > max_fps) max_fps = fps[i];
          if (completes[i] < min_comp) min_comp = completes[i];
          if (completes[i] > max_comp) max_comp = completes[i];
          if (rescue[i] < min_rescue) min_rescue = rescue[i];
          if (rescue[i] > max_rescue) max_rescue = rescue[i];
          sum_fps += fps[i];
          sum_comp += completes[i];
          sum_rescue += rescue[i];
        }
        printf "fps_at_end: min=%d, max=%d, mean=%.1f\n", min_fps, max_fps, sum_fps / count;
        printf "completes_at_end: min=%d, max=%d, mean=%.1f\n", min_comp, max_comp, sum_comp / count;
        printf "rescue_tid13_events: min=%d, max=%d, mean=%.1f\n", min_rescue, max_rescue, sum_rescue / count;
      }
    }
  ' "$METRICS" >"$AGG"
}

log "starting 5-boot virtio-gpu stress gate boots=$BOOT_COUNT active=${ACTIVE_SECONDS}s"
for boot in $(seq 1 "$BOOT_COUNT"); do
  run_boot "$boot"
done
write_aggregate
cat "$AGG"
heartbeat
