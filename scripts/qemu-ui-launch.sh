#!/usr/bin/env bash
# Prints the command to put in front of a qemu-system-aarch64 launch so that QEMU's threads, its
# vCPU threads included, run in macOS's user-interactive priority band (scripts/qemu-ui-launch.c).
#
# Under load, a QEMU started from a shell runs its vCPU threads at the default priority, 31, below
# what the operator's other work runs at, and the guest's timers fire late when the host does not
# run those threads. On a Mac this builds scripts/qemu-ui-launch.c and scripts/qemu-ui-threads.c
# once into the cache directory (again whenever either changes) and prints the launcher's path. Elsewhere, with
# BREENIX_QEMU_UI=0 (a launch at today's default priority, for comparison), or when the build
# fails, it prints `env`, which runs QEMU unchanged. Either way the caller writes
#     QEMU_UI="$("$ROOT/scripts/qemu-ui-launch.sh")"
#     "$QEMU_UI" qemu-system-aarch64 ...
# The launcher execs QEMU in place, so QEMU keeps the process ID a watcher or `timeout` started.
set -u
if [ "$(uname -s)" != Darwin ] || [ "${BREENIX_QEMU_UI:-1}" = 0 ]; then
    echo env
    exit 0
fi
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CACHE="${BREENIX_QEMU_APP_DIR:-$HOME/Library/Caches/breenix}"
HASH="$(cat "$HERE/qemu-ui-launch.c" "$HERE/qemu-ui-threads.c" 2>/dev/null | shasum -a 256 | cut -c1-16)"
DIR="$CACHE/qemu-ui-$HASH"
if [ ! -x "$DIR/qemu-ui-launch" ] || [ ! -f "$DIR/qemu-ui-threads.dylib" ]; then
    TMP="$DIR.tmp.$$"
    rm -rf "$TMP"
    if mkdir -p "$TMP" &&
        cc -O2 -Wall -Werror -dynamiclib -o "$TMP/qemu-ui-threads.dylib" "$HERE/qemu-ui-threads.c" &&
        cc -O2 -Wall -Werror -o "$TMP/qemu-ui-launch" "$HERE/qemu-ui-launch.c" &&
        { rm -rf "$DIR"; mv "$TMP" "$DIR"; }; then
        :
    else
        rm -rf "$TMP"
        echo "qemu-ui-launch.sh: cannot build the launcher from $HERE; QEMU runs at the default priority" >&2
        echo env
        exit 0
    fi
fi
echo "$DIR/qemu-ui-launch"
