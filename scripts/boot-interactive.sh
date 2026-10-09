#!/bin/bash
# Build the ARM64 kernel and userspace disk, then boot it once with the serial
# console on this terminal. In the default `tests` mode this is the boot-path gate
# (docs/boot-path.md) with a console you can watch and type into.
#
#   scripts/boot-interactive.sh [--mode MODE] [--program PATH] [--suite ID] [--serial-log FILE]
#                               [--idle-exit SECONDS] [--gate-timeout SECONDS] [--display | --no-display]
#                               [--qmp SOCKET] [--fbconsole log] [--no-build]
#
#   Suite mode observes DONE, holds the scored panel, then exits with the suite verdict.
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
#   --gate-timeout SECONDS  suite DONE deadline (default 1800), even with --idle-exit 0
#   BREENIX_SUITE_HOLD    suite panel hold after a 2 s render delay (default 5 s)
#   --display            open QEMU's display window as well (default: serial only; on for desktop)
#   --no-display         serial only, even for desktop
#   --no-build           boot the kernel and disk already in target/
#
# Ctrl-A X quits QEMU; Ctrl-A C toggles the QEMU monitor. Ctrl-C goes to the guest.
# Each step prints a "==> " line so a watcher can tell building from booting.
# --help prints this header.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/host-slots.sh"
host_slots_start "$ROOT/scripts/$(basename "${BASH_SOURCE[0]}")" "$@"
SERIAL_LOG="${TMPDIR:-/tmp}/breenix-boot/serial.txt"
IDLE_EXIT=300
GATE_TIMEOUT=1800
DISPLAY_MODE=
BUILD=1
MODE=tests
PROGRAM=
SUITE=
QMP_SOCKET=
FBCONSOLE=

while [ "$#" -gt 0 ]; do
    case "$1" in
        --mode|--program|--suite|--serial-log|--idle-exit|--gate-timeout|--qmp|--fbconsole)
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
        --gate-timeout) GATE_TIMEOUT="$2"; shift 2 ;;
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
# Explicit regression/profile features supplement the selected normal profile.
if [ -n "${BREENIX_KERNEL_FEATURES:-}" ]; then
    [[ "$BREENIX_KERNEL_FEATURES" =~ ^[a-zA-Z0-9_-]+(,[a-zA-Z0-9_-]+)*$ ]] || { echo "invalid kernel features" >&2; exit 2; }
    if [ "$MODE" = tests ]; then
        KERNEL_FEATURES=(--features "testing,$BREENIX_KERNEL_FEATURES")
    else
        KERNEL_FEATURES=(--features "$BREENIX_KERNEL_FEATURES")
    fi
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
    "$ROOT/userspace/programs/build.sh" --arch aarch64
    echo "==> Building the ext2 disk"
    "$ROOT/scripts/create_ext2_disk.sh" --arch aarch64
    if [ "$MODE" = tests ]; then
        echo "==> Building the testing kernel"
    else
        echo "==> Building the production kernel"
    fi
    # The production build is the prod-profile gate's command: no features at all.
    cargo build --release ${KERNEL_FEATURES[@]+"${KERNEL_FEATURES[@]}"} --target aarch64-breenix-kernel.json \
        -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
    "$ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"
fi
[ -f "$KERNEL" ] || { echo "No kernel at $KERNEL" >&2; exit 1; }
[ -f "$DISK" ] || { echo "No disk at $DISK" >&2; exit 1; }

host_slot_acquire mac-boot
host_slot_serial "$SERIAL_LOG"
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
# Wait interruptibly and register the watcher so a signal to this script also
# stops and reaps its QEMU before the host lock is released.
trap 'exit 143' TERM
trap 'exit 129' HUP
trap 'exit 130' INT
exec 3<&0
set +e
python3 "$ROOT/scripts/watch-qemu.py" --serial "$SERIAL_LOG" --mode "$MODE" \
    --suite "$SUITE" --idle-exit "$IDLE_EXIT" --gate-timeout "$GATE_TIMEOUT" --disk "$WRITABLE" -- qemu-system-aarch64 \
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
WATCHER_PID=$!
qemu_host_lock_track_pid "$WATCHER_PID"
wait "$WATCHER_PID"
code=$?
set -e
echo
echo "==> VM stopped (exit $code)"
host_slot_header "$SERIAL_LOG"
"$ROOT/scripts/vigil-record.sh" finish "$VIGIL_ID" "$code"

# Non-suite interactive modes retain their existing shell exit behavior; Vigil
# still receives QEMU's status. Suites must propagate their scored verdict.
if [ "$MODE" = suite ]; then exit "$code"; fi
