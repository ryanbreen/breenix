#!/bin/bash
# failure-trace-capture PR-6: passive QMP evidence for a non-PASS boot.
# Call after freezing the verdict and before the gate's own first SIGTERM.
# One QMP_DUMP line per outcome; complete means a nonempty core exists,
# not that decoding or validation succeeded (consult qmp-decode.log).
#
# Paths and budget are explicit per-call arguments, rather than source-time
# cached globals: a caller's current timeout override must reach this call.
# The outer timeout bounds the entire forensic child, independently of its
# internal polling or wall-clock readings. A kill-after also bounds a child
# that ignores TERM. Decoding has its own timeout; dump_ms measures capture.
# PASS just prints its contract and does not access the socket.
#
# Same clock primitive as gcd_now_ms, duplicated rather than sourced to
# avoid a load-order dependency on the sibling drain library. This clock
# is best-effort reporting only; no termination decision trusts its value.
gqb_now_ms() {
    python3 -c 'import time; print(int(time.time() * 1000))' 2>/dev/null || echo 0
}

gqb_pass_report() {
    printf '[QMP_DUMP:capture=n/a:reason=n/a:core=n/a:decoded_events=n/a:dump_ms=0]\n'
}

gqb_dump_and_report() {
    local sock="$1" output_dir="$2" fc_sh="$3" decode_py="$4" kernel_elf="$5"
    local budget_s="${6:-30}"
    local start_ms end_ms dump_ms core="$output_dir/guest-memory.elf"
    mkdir -p "$output_dir"
    start_ms="$(gqb_now_ms)"
    # timeout treats 0 as unbounded; refuse it (and malformed budgets).
    if ! [[ "$budget_s" =~ ^[1-9][0-9]*$ ]]; then
        printf '[QMP_DUMP:capture=partial:reason=invalid_budget:core=-:decoded_events=-:dump_ms=0]\n'
        return
    fi
    if [ ! -S "$sock" ]; then
        end_ms="$(gqb_now_ms)"
        printf '[QMP_DUMP:capture=partial:reason=qmp_socket_missing:core=-:decoded_events=-:dump_ms=%s]\n' "$((end_ms - start_ms))"
        return
    fi
    # A repeated call must not classify a previous dump as this attempt's.
    rm -f "$core"
    timeout --kill-after=1 "$budget_s" bash "$fc_sh" --qmp "$sock" --output-dir "$output_dir" \
        >"$output_dir/qmp-backstop.log" 2>&1 || true
    end_ms="$(gqb_now_ms)"
    dump_ms=$((end_ms - start_ms))
    if [ -s "$core" ]; then
        local decode_out decoded
        decode_out="$(timeout --kill-after=1 "$budget_s" python3 "$decode_py" --parse "$core" --kernel "$kernel_elf" --validate 2>&1)" || true
        printf '%s\n' "$decode_out" > "$output_dir/qmp-decode.log"
        decoded="$(printf '%s\n' "$decode_out" | grep -oE 'TRACE_DECODED_EVENTS:[0-9]+' | head -1 | cut -d: -f2)" || true
        printf '[QMP_DUMP:capture=complete:reason=-:core=%s:decoded_events=%s:dump_ms=%s]\n' \
            "$core" "${decoded:--}" "$dump_ms"
    else
        printf '[QMP_DUMP:capture=partial:reason=qmp_timeout:core=-:decoded_events=-:dump_ms=%s]\n' "$dump_ms"
    fi
}
