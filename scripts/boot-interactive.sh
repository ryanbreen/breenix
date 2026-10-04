#!/bin/bash
# Build the ARM64 kernel and userspace disk, then boot it once with the serial
# console on this terminal. In the default `tests` mode this is the boot-path gate
# (docs/boot-path.md) with a console you can watch and type into.
#
#   scripts/boot-interactive.sh [--mode MODE] [--program PATH] [--suite ID] [--serial-log FILE]
#                               [--idle-exit SECONDS] [--display | --no-display]
#                               [--qmp SOCKET] [--fbconsole log] [--no-build]
#
#   --mode MODE          tests (default): the testing kernel and its test loader
#                        probe | shell | desktop | program | suite: the production kernel, told the mode
#                        via -fw_cfg name=opt/breenix/mode (see "Boot modes" in docs/boot-path.md)
#   --program PATH       for --mode program: the absolute guest path /sbin/probe runs (probe --run PATH)
#   --suite ID           for --mode suite: the effort suite (docs/suites/ID.json) run as PID 1, /sbin/suite-ID
#   --qmp SOCKET         open a QMP socket so another process can screenshot the VM screen:
#                        scripts/qmp-screendump.py SOCKET out.png (works with --no-display)
#   --fbconsole log      draw kernel log lines on the VM screen instead of the boot screen
#   --serial-log FILE    also write everything the guest prints to FILE (default: $TMPDIR/breenix-boot/serial.txt)
#   --idle-exit SECONDS  stop the VM after this many seconds without new serial output (default 300; 0 = never)
#   --display            open QEMU's display window as well (default: serial only; on for desktop)
#   --no-display         serial only, even for desktop
#   --no-build           boot the kernel and disk already in target/
#
# Ctrl-A X quits QEMU; Ctrl-A C toggles the QEMU monitor. Ctrl-C goes to the guest.
# Each step prints a "==> " line so a watcher can tell building from booting.
# --help prints this header.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERIAL_LOG="${TMPDIR:-/tmp}/breenix-boot/serial.txt"
IDLE_EXIT=300
DISPLAY_MODE=
BUILD=1
MODE=tests
PROGRAM=
SUITE=
QMP_SOCKET=
FBCONSOLE=

while [ "$#" -gt 0 ]; do
    case "$1" in
        --mode|--program|--suite|--serial-log|--idle-exit|--qmp|--fbconsole)
            [ "$#" -ge 2 ] || { echo "$1 needs a value" >&2; exit 2; } ;;
    esac
    case "$1" in
        --mode) MODE="$2"; shift 2 ;;
        --program) PROGRAM="$2"; shift 2 ;;
        --suite) SUITE="$2"; shift 2 ;;
        --qmp) QMP_SOCKET="$2"; shift 2 ;;
        --fbconsole) FBCONSOLE="$2"; shift 2 ;;
        --serial-log) SERIAL_LOG="$2"; shift 2 ;;
        --idle-exit) IDLE_EXIT="$2"; shift 2 ;;
        --display) DISPLAY_MODE=cocoa; shift ;;
        --no-display) DISPLAY_MODE=none; shift ;;
        --no-build) BUILD=0; shift ;;
        -h|--help) sed -n '2,/^# --help/p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
case "$MODE" in
    tests|probe|shell|desktop|program|suite) ;;
    *) echo "unknown mode: $MODE (expected tests, probe, shell, desktop, program or suite)" >&2; exit 2 ;;
esac
if [ "$MODE" = program ]; then
    case "$PROGRAM" in
        /*) ;;
        "") echo "--mode program needs --program PATH" >&2; exit 2 ;;
        *) echo "--program needs an absolute guest path, got: $PROGRAM" >&2; exit 2 ;;
    esac
elif [ -n "$PROGRAM" ]; then
    echo "--program is only for --mode program" >&2; exit 2
fi
if [ "$MODE" = suite ]; then
    [ -n "$SUITE" ] || { echo "--mode suite needs --suite ID" >&2; exit 2; }
    if [[ ! "$SUITE" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]]; then
        echo "--suite needs a suite id (lowercase words joined by '-'), got: $SUITE" >&2; exit 2
    fi
elif [ -n "$SUITE" ]; then
    echo "--suite is only for --mode suite" >&2; exit 2
fi
case "$FBCONSOLE" in
    ""|log) ;;
    *) echo "unknown --fbconsole value: $FBCONSOLE (expected log)" >&2; exit 2 ;;
esac
if [ -z "$DISPLAY_MODE" ]; then
    if [ "$MODE" = desktop ]; then DISPLAY_MODE=cocoa; else DISPLAY_MODE=none; fi
fi
DISPLAY_ARGS=(-display "$DISPLAY_MODE")
MODE_ARGS=()
KERNEL_FEATURES=(--features testing)
if [ "$MODE" != tests ]; then
    MODE_ARGS=(-fw_cfg "name=opt/breenix/mode,string=$MODE")
    KERNEL_FEATURES=()
fi
if [ "$MODE" = program ]; then
    MODE_ARGS+=(-fw_cfg "name=opt/breenix/program,string=$PROGRAM")
fi
if [ "$MODE" = suite ]; then
    MODE_ARGS+=(-fw_cfg "name=opt/breenix/suite,string=$SUITE")
fi
if [ -n "$FBCONSOLE" ]; then
    MODE_ARGS+=(-fw_cfg "name=opt/breenix/fbconsole,string=$FBCONSOLE")
fi
QMP_ARGS=()
if [ -n "$QMP_SOCKET" ]; then
    case "$QMP_SOCKET" in /*) ;; *) QMP_SOCKET="$PWD/$QMP_SOCKET" ;; esac
    rm -f "$QMP_SOCKET"
    QMP_ARGS=(-qmp "unix:$QMP_SOCKET,server=on,wait=off")
fi
case "$SERIAL_LOG" in /*) ;; *) SERIAL_LOG="$PWD/$SERIAL_LOG" ;; esac

cd "$ROOT"
KERNEL="$ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
DISK="$ROOT/target/ext2-aarch64.img"

case "$MODE" in
    program) echo "==> Mode: program $PROGRAM" ;;
    suite) echo "==> Mode: suite $SUITE" ;;
    *) echo "==> Mode: $MODE" ;;
esac
if [ "$BUILD" -eq 1 ]; then
    echo "==> Building userspace"
    userspace/programs/build.sh --arch aarch64
    echo "==> Building the ext2 disk"
    scripts/create_ext2_disk.sh --arch aarch64
    if [ "$MODE" = tests ]; then
        echo "==> Building the testing kernel"
    else
        echo "==> Building the production kernel"
    fi
    # The production build is the prod-profile gate's command: no features at all.
    cargo build --release ${KERNEL_FEATURES[@]+"${KERNEL_FEATURES[@]}"} --target aarch64-breenix-kernel.json \
        -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
    scripts/check-kernel-no-neon.sh "$KERNEL"
fi
[ -f "$KERNEL" ] || { echo "No kernel at $KERNEL" >&2; exit 1; }
[ -f "$DISK" ] || { echo "No disk at $DISK" >&2; exit 1; }

mkdir -p "$(dirname "$SERIAL_LOG")"
: > "$SERIAL_LOG"
WRITABLE="$(dirname "$SERIAL_LOG")/ext2-writable.img"
cp "$DISK" "$WRITABLE"

# shellcheck source=../docker/qemu/lib/qemu-host-lock.sh
source "$ROOT/docker/qemu/lib/qemu-host-lock.sh"
qemu_host_lock_acquire

# Vigil shows the boot while it runs and scores it when it ends (does nothing without Vigil).
VIGIL_ID=$("$ROOT/scripts/vigil-record.sh" start qemu "$MODE" "$SUITE" "$SERIAL_LOG")
echo "==> Booting (serial: $SERIAL_LOG; Ctrl-A X quits)"
[ -z "$QMP_SOCKET" ] || echo "==> QMP: $QMP_SOCKET (scripts/qmp-screendump.py $QMP_SOCKET out.png)"
# Without job control a background job's stdin is /dev/null, so hand it the terminal explicitly.
exec 3<&0
qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
    -kernel "$KERNEL" \
    "${DISPLAY_ARGS[@]}" -no-reboot \
    ${MODE_ARGS[@]+"${MODE_ARGS[@]}"} \
    ${QMP_ARGS[@]+"${QMP_ARGS[@]}"} \
    -device virtio-gpu-device \
    -device virtio-keyboard-device \
    -device virtio-tablet-device \
    -device virtio-blk-device,drive=ext2 \
    -drive if=none,id=ext2,format=raw,file="$WRITABLE" \
    -device virtio-net-device,netdev=net0 \
    -netdev user,id=net0 \
    -chardev stdio,id=con,mux=on,signal=off,logfile="$SERIAL_LOG" \
    -serial chardev:con -mon chardev=con,mode=readline <&3 &
QEMU_PID=$!
qemu_host_lock_track_pid "$QEMU_PID"

if [ "$IDLE_EXIT" -gt 0 ]; then
    (
        last=-1; quiet=0
        while kill -0 "$QEMU_PID" 2>/dev/null; do
            sleep 5
            size=$(wc -c < "$SERIAL_LOG" 2>/dev/null || echo 0)
            if [ "$size" -eq "$last" ]; then quiet=$((quiet + 5)); else quiet=0; last=$size; fi
            if [ "$quiet" -ge "$IDLE_EXIT" ]; then
                printf '\r\n==> No serial output for %ss; stopping the VM\r\n' "$IDLE_EXIT"
                kill -TERM "$QEMU_PID" 2>/dev/null || true
                break
            fi
        done
    ) &
fi

set +e
wait "$QEMU_PID"
code=$?
set -e
echo
echo "==> VM stopped (qemu exit $code)"
"$ROOT/scripts/vigil-record.sh" finish "$VIGIL_ID" "$code"
