#!/bin/bash
# failure-trace-capture PR-5: drains a guest's in-flight BXCAP capture before
# a gate sends SIGTERM/KILL, on a non-PASS outcome only, and classifies what
# ended up on the wire. See
# docs/planning/green-program/failure-capture/PLAN-2026-09-05.md section 6
# (PR-5) and this round's own doc,
# docs/planning/green-program/failure-capture/PR-5-2026-09-06.md, for the
# design and the measured bound.
#
# The wire format this reads is kernel/src/capture/mod.rs's BXCAP v1:
# `[BXCAP:BEGIN v=1 seq=<n> edge=<EDGE> cpu=<n> ...]` opens one capture and
# `[BXCAP:END v=1 seq=<n> edge=<EDGE> verdict=<complete|partial> records=<n>
# ...]` closes it. This file's own `capture=` value below reuses that exact
# vocabulary (`complete`/`partial`) rather than the plan's original
# `complete|truncated|absent`, because `complete|partial` is what the landed
# emitter (PR-3, kernel/src/capture/mod.rs) already prints on its own `END`
# line for the in-capture read; `absent` is this file's own addition for "no
# BEGIN was ever written," a case the emitter itself has no line to report.
#
# What "drain" means here, precisely -- called by a gate ONLY on a non-PASS
# outcome, and ONLY before that gate's own kill line runs:
#   1. unconditionally sleep GCD_SETTLE_MS -- a flat wait, not a stability
#      poll -- because a kill can land mid-write on ANY serial line, not only
#      inside a BXCAP block (#826's boot 16 ends mid-line: the SIGTERM landed
#      inside a guest write the gate itself caused);
#   2. re-read the file; if its last BXCAP:BEGIN has no matching END, wait
#      for the file to stop growing -- byte-stable for GCD_QUIET_MS -- bounded
#      by GCD_MAX_MS total, then stop waiting regardless of what is on the
#      wire at that point.
# A PASS outcome never calls gcd_drain_and_report: it costs a passing boot
# exactly 0ms of added latency, and gcd_pass_report below prints
# `capture=n/a` unconditionally for that case, deliberately not read from the
# file -- see the round doc's "PASS semantics" section for why n/a rather
# than a real classification (the short answer: a PASS verdict does not need
# capture evidence, and reading the file for a boot this function never
# waited on would let the field imply verification it did not do).
# claim-lint:ok: read directly off gcd_drain_and_report's own body below,
# which has no third call site -- docker/qemu/lib/gate-capture-drain.sh.
#
# The worst-case added latency on a non-PASS boot is bounded:
# GCD_SETTLE_MS + GCD_MAX_MS (the settle wait always runs once; the second
# wait runs at most once, only when a capture was genuinely left open).
# claim-lint:ok: read directly off gcd_drain_and_report's own body below,
# which has exactly those two steps -- docker/qemu/lib/gate-capture-drain.sh.
#
# All bounds are overridable so a caller -- or a test -- can shrink them or
# disable draining outright (the four env-var reads immediately below).
# claim-lint:ok: tests/gate_capture_drain_structure.rs's oracle test
# overrides three of the four to shrink them for test speed.
GCD_SETTLE_MS="${BREENIX_GATE_DRAIN_SETTLE_MS:-300}"
GCD_QUIET_MS="${BREENIX_GATE_DRAIN_QUIET_MS:-250}"
GCD_MAX_MS="${BREENIX_GATE_DRAIN_MAX_MS:-3000}"
# How many trailing [BXCAP:EV] records last_events= carries.
GCD_LAST_EVENTS_N="${BREENIX_GATE_DRAIN_EVENTS_N:-8}"
# BREENIX_GATE_DRAIN_DISABLE=1 skips both waits in gcd_drain_and_report and
# classifies the file exactly as it stands the instant this function is
# called -- i.e. the file as it would have looked had the caller's kill line
# run immediately instead of waiting. This is the mutation this PR's own
# oracle and ratchet are measured against: draining a boot whose capture was
# still open at that instant reads `partial` with the wait disabled and
# `complete` with it enabled, from the identical underlying serial content.

# Host wall clock in whole milliseconds. Same primitive
# lib/gate-boot-facts.sh's gbf_host_ms_now uses, duplicated (not sourced)
# so this file carries no load-order dependency on that one.
gcd_now_ms() {
    python3 -c 'import time; print(int(time.time() * 1000))' 2>/dev/null || echo 0
}

# Total byte size across one or more files (0 for any that do not exist yet
# -- a gate mid-launch, or a QEMU with more than one `-serial file:` sink
# where BXCAP output could land on either). Every gcd_* function below takes
# one or more files for exactly this reason: aarch64's single `-serial
# file:` sink needs one, x86's `serial_user.txt`/`serial_kernel.txt` pair
# needs both scanned together.
# claim-lint:ok: by construction -- every gcd_* signature below takes "$@",
# proven against a 2-file fixture in tests/gate_capture_drain_structure.rs.
gcd_size_bytes() {
    local total=0 f sz
    for f in "$@"; do
        if [ -f "$f" ]; then
            sz="$(wc -c <"$f" 2>/dev/null | tr -d ' ')"
            total=$((total + ${sz:-0}))
        fi
    done
    echo "$total"
}

# Waits until the total size of $3.. (one or more files) has been byte-stable
# for $1 ms, bounded by a TOTAL of $2 ms. Prints the number of milliseconds
# actually spent to stdout.
#
# The bound is enforced by a poll-COUNT ceiling, not a wall-clock read: if
# gcd_now_ms's python3 dependency is unavailable it falls back to a constant
# "0", and bounding a while-loop on a clock read that can be stuck at a
# constant would hang forever instead of draining. `max_polls` is computed
# once from $2 before the loop starts and decremented by a plain counter, so
# the loop always terminates in at most `max_polls` iterations of
# `poll_ms` each regardless of whether the clock reads are trustworthy; the
# clock is used only to report how long this took, which is best-effort.
# claim-lint:ok: the poll-count loop immediately below is the mechanism this
# describes; tests/gate_capture_drain_structure.rs runs it end to end.
gcd_wait_stable() {
    local quiet_ms="$1" max_ms="$2"
    shift 2
    local poll_ms=50
    local quiet_polls max_polls
    quiet_polls=$(((quiet_ms + poll_ms - 1) / poll_ms))
    [ "$quiet_polls" -ge 1 ] || quiet_polls=1
    max_polls=$(((max_ms + poll_ms - 1) / poll_ms))
    [ "$max_polls" -ge 1 ] || max_polls=1

    local start_ms end_ms last_size cur_size stable_count i
    start_ms="$(gcd_now_ms)"
    last_size="$(gcd_size_bytes "$@")"
    stable_count=0
    i=0
    while [ "$i" -lt "$max_polls" ]; do
        sleep 0.05
        i=$((i + 1))
        cur_size="$(gcd_size_bytes "$@")"
        if [ "$cur_size" = "$last_size" ]; then
            stable_count=$((stable_count + 1))
        else
            stable_count=0
            last_size="$cur_size"
        fi
        if [ "$stable_count" -ge "$quiet_polls" ]; then
            break
        fi
    done
    end_ms="$(gcd_now_ms)"
    echo "$((end_ms - start_ms))"
}

# Extracts one key's value from a bracketed BXCAP record line. BXCAP values
# never contain a space or `]` (the schema's own rule, PLAN section 4), so
# bounding a match on either is exact -- no risk of grabbing past this
# field into the next.
# claim-lint:ok: the no-spaces rule is stated directly in
# kernel/src/capture/record.rs's own doc comment; a property of the format
# definition, not a sampled measurement.
gcd_field() {
    local line="$1" key="$2"
    printf '%s\n' "$line" | grep -oE "(^|[ ])${key}=[^] ]+" | tail -1 | sed "s/^ //; s/^${key}=//"
}

# Classifies the LAST capture across one or more serial files, reading them
# exactly as they stand -- no waiting. Prints five space-joined tokens:
# `<capture> <seq> <edge> <cpu> <records>`, each `-` when not applicable.
# When a caller's guest writes BXCAP output to more than one `-serial file:`
# sink (x86's kernel/user split), all of them are read here with `grep -h`,
# so a BEGIN on one file and its END on another still pair up correctly --
# the pairing key is `seq=`, not which file a line landed in.
# claim-lint:ok: the 2-file BEGIN/END-split fixture in
# tests/gate_capture_drain_structure.rs exercises exactly this.
gcd_classify() {
    local existing=()
    local f
    for f in "$@"; do
        [ -f "$f" ] && existing+=("$f")
    done
    local begin_line seq edge cpu end_line records
    if [ "${#existing[@]}" -eq 0 ]; then
        echo "absent - - - -"
        return
    fi
    begin_line="$(grep -ahoE '\[BXCAP:BEGIN[^]]*\]' "${existing[@]}" 2>/dev/null | tail -1 || true)"
    if [ -z "$begin_line" ]; then
        echo "absent - - - -"
        return
    fi
    seq="$(gcd_field "$begin_line" seq)"
    edge="$(gcd_field "$begin_line" edge)"
    cpu="$(gcd_field "$begin_line" cpu)"
    end_line="$(grep -ahoE '\[BXCAP:END[^]]*\]' "${existing[@]}" 2>/dev/null | grep -F " seq=${seq} " | tail -1 || true)"
    if [ -n "$end_line" ]; then
        records="$(gcd_field "$end_line" records)"
        echo "complete ${seq:--} ${edge:--} ${cpu:--} ${records:--}"
    else
        echo "partial ${seq:--} ${edge:--} ${cpu:--} -"
    fi
}

# Last N `[BXCAP:EV ...]` records across one or more serial files, in
# whatever order the underlying `grep -h` visits the files (see gcd_classify
# for why more than one) then truncated to the trailing N -- oldest first,
# newest last WITHIN a single file; not merged by timestamp across files,
# which is an approximation this function's caller (gcd_drain_and_report)
# accepts because it is diagnostic context on a FAIL line, not a scored
# field. Prints `none` when there are zero EV records anywhere (a real
# reading: the capture ran but the ring it copied from was empty) or none of
# the files exist yet.
# claim-lint:ok: the zero-EV-records `none` reading is exercised in
# tests/gate_capture_drain_structure.rs::classify_pairs_a_begin_and_end_split_across_two_serial_files_by_seq.
gcd_last_events() {
    local n="$1"
    shift
    local existing=()
    local f
    for f in "$@"; do
        [ -f "$f" ] && existing+=("$f")
    done
    local lines out entry name payload
    if [ "${#existing[@]}" -eq 0 ]; then
        echo "none"
        return
    fi
    lines="$(grep -ahoE '\[BXCAP:EV[^]]*\]' "${existing[@]}" 2>/dev/null | tail -n "$n" || true)"
    if [ -z "$lines" ]; then
        echo "none"
        return
    fi
    out=""
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        name="$(gcd_field "$line" n)"
        payload="$(gcd_field "$line" p)"
        entry="${name:-?}(${payload:-?})"
        if [ -z "$out" ]; then
            out="$entry"
        else
            out="$out,$entry"
        fi
    done <<GCD_EOF
$lines
GCD_EOF
    echo "${out:-none}"
}

# The main non-PASS entry point. Call this AFTER a non-PASS outcome is known
# and BEFORE the caller's own `kill $QEMU_PID` line. Takes one or more
# serial files (see gcd_classify for why more than one). Prints two bracket
# lines to stdout, ready to echo/append verbatim:
#   [CAPTURE_DRAIN:capture=<v>:seq=<v>:edge=<v>:cpu=<v>:records=<v>:drain_ms=<v>]
#   [CAPTURE_DRAIN_EVENTS:last_events=<v>]
gcd_drain_and_report() {
    local settle_ms=0 wait_ms=0 total_ms
    local cls capture seq edge cpu records events

    if [ "${BREENIX_GATE_DRAIN_DISABLE:-}" != "1" ]; then
        # Step 1: the unconditional flat settle -- catches a kill landing
        # mid-write on any line, BXCAP or not.
        sleep "$(awk -v ms="$GCD_SETTLE_MS" 'BEGIN { printf "%.3f", ms / 1000 }')"
        settle_ms="$GCD_SETTLE_MS"
        # Step 2: only if a capture was left open after the settle does this
        # gate pay the longer, bounded wait.
        cls="$(gcd_classify "$@")"
        capture="${cls%% *}"
        if [ "$capture" = "partial" ]; then
            wait_ms="$(gcd_wait_stable "$GCD_QUIET_MS" "$GCD_MAX_MS" "$@")"
        fi
    fi

    cls="$(gcd_classify "$@")"
    # Parsed with a fixed, explicit IFS on the `read` itself (not relying on
    # the ambient one, which this codebase's own shells have been observed
    # to carry stray bytes in) -- five whitespace-separated tokens in, five
    # named variables out.
    IFS=' ' read -r capture seq edge cpu records <<GCD_CLS_EOF
$cls
GCD_CLS_EOF
    total_ms=$((settle_ms + wait_ms))
    events="$(gcd_last_events "$GCD_LAST_EVENTS_N" "$@")"

    printf '[CAPTURE_DRAIN:capture=%s:seq=%s:edge=%s:cpu=%s:records=%s:drain_ms=%s]\n' \
        "$capture" "$seq" "$edge" "$cpu" "$records" "$total_ms"
    printf '[CAPTURE_DRAIN_EVENTS:last_events=%s]\n' "$events"
}

# The PASS-outcome line. Deliberately not read from the file -- see this
# file's header. The two printf literals immediately below, unconditional
# and untimed, are this function's entire body.
gcd_pass_report() {
    printf '[CAPTURE_DRAIN:capture=n/a:seq=n/a:edge=n/a:cpu=n/a:records=n/a:drain_ms=0]\n'
    printf '[CAPTURE_DRAIN_EVENTS:last_events=n/a]\n'
}
