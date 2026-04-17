#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <runs> <out-dir>" >&2
    exit 2
fi

runs="$1"
out_root="$2"
wait_secs="${F20E_WAIT_SECS:-45}"
serial_log="/tmp/breenix-parallels-serial.log"

mkdir -p "$out_root"

sample_host_cpu() {
    ps -A -o pcpu= -o command= | awk '
        /prl_vm_app|Parallels VM|Parallels Desktop/ { sum += $1 }
        END { printf "%.1f\n", sum + 0.0 }
    '
}

extract_first_array_value() {
    local pattern="$1"
    local file="$2"
    sed -n "s/.*${pattern}=\\[\\([0-9][0-9]*\\).*/\\1/p" "$file" | tail -n 1
}

cleanup_qemu() {
    pkill -9 qemu-system-x86 2>/dev/null || true
    killall -9 qemu-system-x86_64 2>/dev/null || true
}

for run in $(seq 1 "$runs"); do
    run_dir="$out_root/run${run}"
    mkdir -p "$run_dir"
    : >"$run_dir/cpu-samples.tsv"

    cleanup_qemu

    set +e
    ./run.sh --parallels --test "$wait_secs" >"$run_dir/run.output.txt" 2>&1 &
    runner_pid="$!"

    while kill -0 "$runner_pid" 2>/dev/null; do
        printf "%s\t%s\n" "$(date +%s)" "$(sample_host_cpu)" >>"$run_dir/cpu-samples.tsv"
        sleep 2
    done
    wait "$runner_pid"
    exit_code="$?"
    set -e

    if [ -f "$serial_log" ]; then
        cp "$serial_log" "$run_dir/serial.log"
    else
        : >"$run_dir/serial.log"
    fi

    vm="$(
        sed -n 's/^VM:[[:space:]]*//p' "$run_dir/run.output.txt" | tail -n 1
    )"
    if [ -n "$vm" ]; then
        prlctl stop "$vm" --kill >/dev/null 2>&1 || true
        prlctl delete "$vm" >/dev/null 2>&1 || true
    fi

    host_cpu_avg="$(
        awk '{ sum += $2; count += 1 } END { if (count) printf "%.1f\n", sum / count; else print "0.0" }' \
            "$run_dir/cpu-samples.tsv"
    )"
    boot_script_completed=0
    if grep -q "\\[init\\] Boot script completed" "$run_dir/serial.log"; then
        boot_script_completed=1
    fi
    boot_script_exit127=""
    if grep -q "\\[init\\] Boot script exited with code 127" "$run_dir/serial.log"; then
        boot_script_exit127=127
    fi
    timer_tick_count="$(extract_first_array_value "tick_count" "$run_dir/serial.log")"
    post_wfi_count="$(extract_first_array_value "post_wfi_count" "$run_dir/serial.log")"

    {
        echo "run=${run}"
        echo "exit_code=${exit_code}"
        echo "vm=${vm:-unknown}"
        echo "host_cpu_avg=${host_cpu_avg}"
        echo "boot_script_completed=${boot_script_completed}"
        if [ -n "$boot_script_exit127" ]; then
            echo "boot_script_exit127=${boot_script_exit127}"
        fi
        echo "timer_tick_count=${timer_tick_count:-0}"
        echo "post_wfi_count=${post_wfi_count:-0}"
    } >"$run_dir/summary.txt"

    cat "$run_dir/summary.txt"
done

cleanup_qemu
