#!/usr/bin/env bash
# Shows that chords sent by scripts/hostkeys.py reach a running guest.
#
# Usage: scripts/hostkeys-selftest.sh <backend> <serial-log> [chord ...]
#
# The guest is one of the default (desktop) boots, whose heartbeat prints
# `kbd_nonzero=<N>` once a second: N counts the non-empty USB keyboard reports the guest
# received. Each chord is sent alone and passes when N rises within 15 seconds. A chord
# the backend cannot press prints "no host key path" and counts as not reached, which is
# the answer the suite cases record for it.
#
# QEMU's virtio keyboard has no such counter. Its self-test is the shell mode: boot
# `scripts/boot-interactive.sh --mode shell` with stdin from a fifo, write
# `echo SUITE selftest HOSTKEY keys/letter seq=0 keys=z` into it, and the watcher boot-interactive.sh starts
# presses z; the shell then shows `bsh /> z` after its prompt.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKEND="${1:?backend (prlctl:<vm> or vnc:<host>:<port>)}"
SERIAL="${2:?serial log}"
shift 2
CHORDS=("$@")
if [ "${#CHORDS[@]}" -eq 0 ]; then
    CHORDS=(a alt-f3 ctrl-alt-f2 alt-f7)
    [[ "$BACKEND" = prlctl:* ]] || CHORDS+=(left alt-left alt-right)
fi

count() { grep -aoE 'kbd_nonzero=[0-9]+' "$SERIAL" | tail -1 | cut -d= -f2; }

status=0
for chord in "${CHORDS[@]}"; do
    before="$(count)"
    [ -n "$before" ] || { echo "no heartbeat in $SERIAL yet"; exit 2; }
    if ! python3 "$ROOT/scripts/hostkeys.py" send --backend "$BACKEND" "$chord"; then
        echo "NOT REACHED  $chord  (no host key path)"
        status=1
        continue
    fi
    reached=0
    for _ in $(seq 1 15); do
        sleep 1
        after="$(count)"
        if [ "${after:-0}" -gt "$before" ]; then reached=1; break; fi
    done
    if [ "$reached" -eq 1 ]; then
        echo "REACHED      $chord  (kbd_nonzero $before -> $after)"
    else
        echo "NOT REACHED  $chord  (kbd_nonzero stayed at $before)"
        status=1
    fi
done
exit "$status"
