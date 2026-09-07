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
# Q-1/PR-6 fix pass: docs/planning/green-program/failure-capture/PR-6-2026-09-07.md
# section 5 hit this exact wall producing its own evidence and worked around
# it with a manual, unshipped /tmp symlink. The QMP unix-socket BIND path is
# bounded by struct sockaddr_un's sun_path -- 104 bytes on macOS/BSD, 108 on
# Linux, NUL-terminated -- a separate, much tighter ceiling than
# an ordinary filesystem path. The gate's own output/profile directories are
# correctly lane-scoped under the (long) worktree-scoped BREENIX_GATE_TMP
# this repo's own convention mandates, but a socket path built by appending
# "qmp.sock" under one of those directories routinely exceeds that ceiling
# on that exact convention: qemu-system-aarch64 then refuses to start
# ("Path must be less than 104 bytes", exit 1), not the graceful
# qmp_socket_missing degradation gqb_dump_and_report reports for an
# evidence-only miss -- empty serial output, no per-boot artifact naming the
# real cause. The listener therefore lives in its own short-named directory
# directly under the real /tmp, bypassing $TMPDIR/$BREENIX_GATE_TMP;
# only the ephemeral bind socket needs this -- the durable dump/decode
# artifacts gqb_dump_and_report writes still land wherever the caller's own
# (long, lane-scoped) output directory already puts them.
GQB_SOCK_LIMIT_BYTES=100

gqb_alloc_socket() {
    local dir path
    dir="$(mktemp -d /tmp/bxqmp.XXXXXX)" || {
        echo "gqb_alloc_socket: mktemp failed" >&2
        return 1
    }
    chmod 700 "$dir"
    path="$dir/q.sock"
    # Defensive, not expected to trip: the mktemp template above is fixed
    # width, so this only fires if that template is ever widened later.
    if [ "${#path}" -gt "$GQB_SOCK_LIMIT_BYTES" ]; then
        echo "gqb_alloc_socket: generated path exceeds ${GQB_SOCK_LIMIT_BYTES} bytes: $path" >&2
        rm -rf -- "$dir"
        return 1
    fi
    printf '%s' "$path"
}

gqb_free_socket() {
    # $1: a path previously returned by gqb_alloc_socket. The case below
    # restricts removal to directory strings matching /tmp/bxqmp.*.
    local sock="$1" dir
    [ -n "$sock" ] || return 0
    dir="$(dirname -- "$sock")"
    case "$dir" in
        /tmp/bxqmp.*) rm -rf -- "$dir" ;;
    esac
}

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
