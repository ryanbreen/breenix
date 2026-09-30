#!/bin/bash
# Build the ARM64 kernel and userspace disk, then boot it once with the serial
# console on this terminal. In the default `tests` mode this is the boot-path gate
# (docs/boot-path.md) with a console you can watch and type into.
#
#   scripts/boot-interactive.sh [--mode MODE] [--serial-log FILE] [--idle-exit SECONDS]
#                               [--display | --no-display] [--no-build]
#
#   --mode MODE          tests (default): the testing kernel and its test loader
#                        probe | shell | desktop: the production kernel, told the mode via
#                        -fw_cfg name=opt/breenix/mode (see "Boot modes" in docs/boot-path.md)
#   --serial-log FILE    also write everything the guest prints to FILE (default: $TMPDIR/breenix-boot/serial.txt)
#   --idle-exit SECONDS  stop the VM after this many seconds without new serial output (default 300; 0 = never)
#   --display            open QEMU's display window as well (default: serial only; on for desktop)
#   --no-display         serial only, even for desktop
#   --no-build           boot the kernel and disk already in target/
#
# Ctrl-A X quits QEMU; Ctrl-A C toggles the QEMU monitor. Ctrl-C goes to the guest.
# Each step prints a "==> " line so a watcher can tell building from booting.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERIAL_LOG="${TMPDIR:-/tmp}/breenix-boot/serial.txt"
IDLE_EXIT=300
DISPLAY_MODE=
BUILD=1
MODE=tests

while [ "$#" -gt 0 ]; do
    case "$1" in
        --mode) MODE="$2"; shift 2 ;;
        --serial-log) SERIAL_LOG="$2"; shift 2 ;;
        --idle-exit) IDLE_EXIT="$2"; shift 2 ;;
        --display) DISPLAY_MODE=cocoa; shift ;;
        --no-display) DISPLAY_MODE=none; shift ;;
        --no-build) BUILD=0; shift ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
case "$MODE" in
    tests|probe|shell|desktop) ;;
    *) echo "unknown mode: $MODE (expected tests, probe, shell or desktop)" >&2; exit 2 ;;
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
case "$SERIAL_LOG" in /*) ;; *) SERIAL_LOG="$PWD/$SERIAL_LOG" ;; esac

cd "$ROOT"
KERNEL="$ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
DISK="$ROOT/target/ext2-aarch64.img"

echo "==> Mode: $MODE"
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

echo "==> Booting (serial: $SERIAL_LOG; Ctrl-A X quits)"
# Without job control a background job's stdin is /dev/null, so hand it the terminal explicitly.
exec 3<&0
qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
    -kernel "$KERNEL" \
    "${DISPLAY_ARGS[@]}" -no-reboot \
    ${MODE_ARGS[@]+"${MODE_ARGS[@]}"} \
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
