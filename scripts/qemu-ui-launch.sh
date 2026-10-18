#!/usr/bin/env bash
# Runs, or prints the command that runs, qemu-system-aarch64 so that QEMU's threads, its vCPU
# threads included, run in macOS's user-interactive priority band (scripts/qemu-ui-launch.c).
#
# Under load, a QEMU started from a shell runs its vCPU threads at the default priority, 31, below
# what the operator's other work runs at, and the guest's timers fire late when the host does not
# run those threads. On a Mac this builds scripts/qemu-ui-launch.c and scripts/qemu-ui-threads.c
# once into the cache directory (again whenever either changes). Elsewhere, with
# BREENIX_QEMU_UI=0 (an explicit policy bypass for diagnostics), the launcher is `env`.
# Launcher build failures stop the boot rather than inheriting an unknown policy.
#
#     qemu-ui-launch.sh qemu-system-aarch64 ...   prepares the launcher and execs it with the command
#     qemu-ui-launch.sh                           prepares the launcher and prints its path
#
# Shell launchers use qemu-arm64.py; the host-lock library exports this script as
# BREENIX_QEMU_UI_LAUNCHER, so preparation waits until QEMU launches. Each exec keeps the
# process ID, so QEMU keeps the one a watcher or `timeout` started.
set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CACHE="${BREENIX_QEMU_APP_DIR:-$HOME/Library/Caches/breenix}"
HASH="$(cat "$HERE/qemu-ui-launch.c" "$HERE/qemu-ui-threads.c" "$HERE/qemu-unclamp.py" 2>/dev/null | shasum -a 256 | cut -c1-16)"
DIR="$CACHE/qemu-ui-$HASH"

published() { [ -x "$DIR/qemu-ui-launch" ] && [ -f "$DIR/qemu-ui-threads.dylib" ] && [ -f "$DIR/qemu-unclamp.py" ]; }

# Runs under the cache lock. An entry another caller finished while this one waited is kept.
# Otherwise both are built in a private directory and renamed into the entry one file at a time,
# the launcher last, so a published file is never removed or seen half-written.
publish() {
    published && return 0
    local tmp
    tmp="$(mktemp -d "$CACHE/qemu-ui-build.XXXXXX")" || return 1
    if taskpolicy -b cc -O2 -Wall -Werror -dynamiclib -o "$tmp/qemu-ui-threads.dylib" "$HERE/qemu-ui-threads.c" &&
        taskpolicy -b cc -O2 -Wall -Werror -o "$tmp/qemu-ui-launch" "$HERE/qemu-ui-launch.c" &&
        mkdir -p "$DIR" &&
        cp "$HERE/qemu-unclamp.py" "$DIR/qemu-unclamp.py" &&
        mv -f "$tmp/qemu-ui-threads.dylib" "$DIR/qemu-ui-threads.dylib" &&
        mv -f "$tmp/qemu-ui-launch" "$DIR/qemu-ui-launch"; then
        rm -rf "$tmp"
        return 0
    fi
    rm -rf "$tmp"
    return 1
}

if [ -n "${_BREENIX_QEMU_UI_PUBLISH:-}" ]; then
    publish
    exit
fi

LAUNCHER=env
if [ "$(uname -s)" = Darwin ] && [ "${BREENIX_QEMU_UI:-1}" != 0 ]; then
    if published || { mkdir -p "$CACHE" &&
        _BREENIX_QEMU_UI_PUBLISH=1 /usr/bin/lockf -k -t 300 "$CACHE/qemu-ui.lock" "$BASH" "${BASH_SOURCE[0]}"; }; then
        LAUNCHER="$DIR/qemu-ui-launch"
    else
        echo "qemu-ui-launch.sh: cannot build the boot priority launcher" >&2
        exit 126
    fi
fi
if [ "$#" -eq 0 ]; then
    echo "$LAUNCHER"
    exit 0
fi
exec "$LAUNCHER" "$@"
