#!/usr/bin/env bash
# #772 diag battery (R111/R112): aggregate one arm's per-boot census.json
# files into arm-level summary numbers, without a JSON parser -- grep+awk
# over the census driver's compact single-line JSON.
#
# Usage: 772-diag-aggregate.sh <arm-dir-containing-boot_NN-subdirs>
set -uo pipefail

ARMDIR="${1:?usage: 772-diag-aggregate.sh <arm-dir>}"

sum_field() {
    # sum_field <field-regex>
    grep -ohE "\"$1\": [0-9]+" "$ARMDIR"/boot_*/census.json 2>/dev/null |
        awk -F': ' '{s+=$2} END {print s+0}'
}

list_field() {
    grep -ohE "\"$1\": [0-9.]+" "$ARMDIR"/boot_*/census.json 2>/dev/null |
        awk -F': ' '{print $2}'
}

echo "=== $ARMDIR ==="
NBOOTS=$(ls -d "$ARMDIR"/boot_* 2>/dev/null | wc -l | tr -d ' ')
echo "boot_dirs=$NBOOTS"

echo "--- proxySerial (no_progress_proxy_pct, per boot) ---"
list_field 'no_progress_proxy_pct' | tr '\n' ' '
echo

echo "--- data_latency_ms (per boot) ---"
list_field 'data_latency_ms' | tr '\n' ' '
echo

echo "--- restores_total sum, no_progress_proxy sum ---"
echo "restores_total_sum=$(sum_field restores_total)"
echo "no_progress_proxy_sum=$(sum_field no_progress_proxy)"

echo "--- DISPATCH_* counter sums (across all boots in this arm) ---"
for f in DISPATCH_NO_PROGRESS_CPU0 DISPATCH_NO_PROGRESS_REFUSED_CPU0 \
         DISPATCH_KERNEL_RESTORE_TOTAL_CPU0 DISPATCH_GATE_PREEMPT_ACTIVE_CPU0 \
         DISPATCH_SWITCH_ROLLED_BACK_CPU0 DISPATCH_SWITCH_IDLE_REDIRECT_CPU0 \
         DISPATCH_EXC_IDLE_REDIRECT_CPU0 \
         DISPATCH_SAVE_REASON_USER_PREEMPT_CPU0 DISPATCH_SAVE_REASON_USER_MANDATORY_CPU0 \
         DISPATCH_SAVE_REASON_KERNEL_BLOCKED_PREEMPT_CPU0 DISPATCH_SAVE_REASON_KERNEL_BLOCKED_MANDATORY_CPU0 \
         DISPATCH_SAVE_REASON_KTHREAD_PREEMPT_CPU0 DISPATCH_SAVE_REASON_KTHREAD_MANDATORY_CPU0 \
         DISPATCH_NOPROGRESS_SAVE_USER_PREEMPT_CPU0 DISPATCH_NOPROGRESS_SAVE_USER_MANDATORY_CPU0 \
         DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_PREEMPT_CPU0 DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_MANDATORY_CPU0 \
         DISPATCH_NOPROGRESS_SAVE_KTHREAD_PREEMPT_CPU0 DISPATCH_NOPROGRESS_SAVE_KTHREAD_MANDATORY_CPU0; do
    echo "$f=$(sum_field "$f")"
done

echo "--- episode turns (all boots, from the 'turns' field inside episodes[]) ---"
grep -ohE '"turns": [0-9]+' "$ARMDIR"/boot_*/census.json 2>/dev/null | awk -F': ' '{print $2}' | sort -n | uniq -c

echo "--- verdict pass/fail (from boot_NN.driver.log RESULT lines) ---"
grep -h '^RESULT ' "$ARMDIR"/*.driver.log 2>/dev/null
