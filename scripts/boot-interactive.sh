#!/bin/bash
# Build the ARM64 kernel and userspace disk, then boot it once with the serial
# console on this terminal. In the default `tests` mode this is the boot-path gate
# (docs/boot-path.md) with a console you can watch and type into.
#
#   scripts/boot-interactive.sh [--mode MODE] [--program PATH] [--suite ID [--case CATEGORY]] [--serial-log FILE]
#                               [--idle-exit SECONDS] [--gate-timeout SECONDS] [--display | --no-display]
#                               [--qmp SOCKET] [--no-build] [--cpus N] [--gpu-gl]
#
#   Suite mode observes DONE, holds the panel briefly, then exits with its verdict.
#   --mode MODE          tests (default): the testing kernel and its test loader
#                        probe | shell | desktop | program | suite: the production kernel, told the mode
#                        via -fw_cfg name=opt/breenix/mode (see "Boot modes" in docs/boot-path.md);
#                        desktop's window manager needs VirGL, which this QEMU GPU lacks, so it exits here
#   --program PATH       for --mode program: the absolute guest path /sbin/probe runs (probe --run PATH)
#   --case CATEGORY       for --mode suite: run only this category (not full-suite evidence; not filed in Vigil)
#   --suite ID           for --mode suite: the effort suite (docs/suites/ID.json) run as PID 1, /sbin/suite-ID
#   --qmp SOCKET         open a QMP socket so another process can screenshot the VM screen:
#                        scripts/qmp-screendump.py SOCKET out.png (works with --no-display)
#   --serial-log FILE    also write everything the guest prints to FILE (default: a unique file under $TMPDIR/breenix-boot/)
#   --idle-exit SECONDS  stop the VM after this many seconds without new serial output (default 300; 0 turns it off)
#   --gate-timeout SECONDS  suite DONE deadline (default 1800), even with --idle-exit 0
#   BREENIX_SUITE_HOLD    suite panel hold after a 2 s render delay (default 5 s)
#   --display            open QEMU's display window (the default on a Mac desktop session; BREENIX_QEMU_NOGUI=1 turns it off)
#   --no-display         serial only, even for desktop
#   --no-build           boot the kernel and disk already in target/
#   --cpus N             guest processors, 1 to 8, the ARM64 kernel's MAX_CPUS (default 4); suite cases
#                        needing more processors than this fail with their CPU prerequisite
#   --accel hvf|tcg      ARM64 accelerator (default hvf on Apple Silicon, tcg elsewhere)
#   --gpu-gl             also attach virtio-gpu-gl-pci, on the GL QEMU that scripts/build-qemu-gl.sh installs
#                        (VirGL on ANGLE's Metal backend), as the window's display: the kernel's boot screen and
#                        console draw through VirGL on it. The window is -display cocoa,gl=es, a run without
#                        one has SPICE GL on a local socket. After the VM stops, scripts/check-host-renderer.py
#                        judges the run's host renderer attestation, host-renderer.json beside the serial log,
#                        and a refused attestation fails the boot. BREENIX_QEMU_GL_BINARY names another GL
#                        QEMU, such as the planted software-renderer copy (scripts/qemu-gl/plant-software-renderer.sh).
#
# Ctrl-A X quits QEMU; Ctrl-A C toggles the QEMU monitor. Ctrl-C goes to the guest.
# Each step prints a "==> " line so a watcher can tell building from booting.
# --help prints this header.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/host-slots.sh"
host_slots_start "$ROOT/scripts/$(basename "${BASH_SOURCE[0]}")" "$@"
SERIAL_LOG="${TMPDIR:-/tmp}/breenix-boot/serial-$(date -u +%Y%m%dT%H%M%SZ)-$$.txt"
IDLE_EXIT=300
GATE_TIMEOUT=1800
DISPLAY_MODE=
BUILD=1
CPUS=4
MODE=tests
PROGRAM=
SUITE=
ONLY=
QMP_SOCKET=
QMP_REQUESTED=0
GPU_GL=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --mode|--program|--suite|--case|--serial-log|--idle-exit|--gate-timeout|--qmp|--accel|--cpus)
            [ "$#" -ge 2 ] || { echo "$1 needs a value" >&2; exit 2; } ;;
    esac
    case "$1" in
        --mode) MODE="$2"; shift 2 ;;
        --accel) export BREENIX_ARM64_QEMU_ACCEL="$2"; shift 2 ;;
        --program) PROGRAM="$2"; shift 2 ;;
        --suite) SUITE="$2"; shift 2 ;;
        --case) [ -n "$2" ] || { echo "--case needs a category id" >&2; exit 2; }; ONLY="$2"; shift 2 ;;
        --qmp) QMP_SOCKET="$2"; QMP_REQUESTED=1; shift 2 ;;
        --serial-log) SERIAL_LOG="$2"; shift 2 ;;
        --idle-exit) IDLE_EXIT="$2"; shift 2 ;;
        --gate-timeout) GATE_TIMEOUT="$2"; shift 2 ;;
        --display) DISPLAY_MODE=cocoa; shift ;;
        --no-display) DISPLAY_MODE=none; shift ;;
        --no-build) BUILD=0; shift ;;
        --cpus) CPUS="$2"; shift 2 ;;
        --gpu-gl) GPU_GL=1; shift ;;
        -h|--help) sed -n '2,/^# --help/p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[[ "$CPUS" =~ ^[1-8]$ ]] || { echo "--cpus needs 1 to 8 (the ARM64 kernel's MAX_CPUS), got: $CPUS" >&2; exit 2; }
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
if [ -n "$ONLY" ]; then
    [ "$MODE" = suite ] && [[ "$ONLY" =~ ^[a-zA-Z0-9_./-]+$ ]] || {
        echo "--case needs --mode suite and a category id" >&2; exit 2;
    }
fi
if [ -z "$DISPLAY_MODE" ]; then
    # On the operator's Mac every boot opens QEMU's window, as Parallels and VMware boots do, so a boot can be watched.
    # BREENIX_QEMU_NOGUI=1 (unattended automation), an ssh session or a host without a window server stays serial-only.
    if [ "$MODE" = desktop ]; then
        DISPLAY_MODE=cocoa
    elif [ "$(uname -s)" = Darwin ] && [ -z "${BREENIX_QEMU_NOGUI:-}" ] && [ -z "${SSH_CONNECTION:-}" ]; then
        DISPLAY_MODE=cocoa
    else
        DISPLAY_MODE=none
    fi
fi
DISPLAY_ARGS=(-display "$DISPLAY_MODE")
QEMU=qemu-system-aarch64
GPU_ARGS=()
if [ "$GPU_GL" -eq 1 ]; then
    QEMU="${BREENIX_QEMU_GL_BINARY:-$("$ROOT/scripts/build-qemu-gl.sh" --print-path)}" || {
        echo "--gpu-gl needs the GL QEMU: run scripts/build-qemu-gl.sh" >&2; exit 1;
    }
    # The bundle draws one guest pixel per device pixel (a 2560x1600 guest in a 1280x800-point
    # window) and, like QEMU-1x, is kept out of App Nap (#1292).
    defaults write org.breenix.qemu-gl NSAppSleepDisabled -bool YES
    # UTM's default ANGLE backend, Metal, unless the caller sets the variable (empty: ANGLE's own
    # default, its OpenGL backend).
    export ANGLE_DEFAULT_PLATFORM="${ANGLE_DEFAULT_PLATFORM-metal}"
    if [ "$DISPLAY_MODE" = cocoa ]; then
        DISPLAY_ARGS=(-display cocoa,gl=es)
    else
        SPICE_SOCKET="/tmp/breenix-spice-$$.sock"
        DISPLAY_ARGS=(-display none -spice "unix=on,addr=$SPICE_SOCKET,disable-ticketing=on,gl=es")
    fi
    # Register this display before MMIO below, so PRESENT updates console 0 in the window.
    GPU_ARGS=(-device virtio-gpu-gl-pci)
elif [ "$DISPLAY_MODE" = cocoa ] && [ "$(uname -s)" = Darwin ]; then
    # Draw the window at one guest pixel per point, as large as a VMware or Parallels window
    # (QEMU's Retina rendering otherwise shows the guest at a quarter of the area).
    QEMU="$("$ROOT/scripts/qemu-1x-app.sh" qemu-system-aarch64)"
    if [ "$QEMU" != qemu-system-aarch64 ] && QEMU_REAL="$(realpath "$(command -v qemu-system-aarch64)")"; then
        # The copy lives outside Homebrew, so name the firmware and keymap directory it would find beside itself.
        DISPLAY_ARGS+=(-L "$(dirname "$(dirname "$QEMU_REAL")")/share/qemu")
    fi
fi
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
    # Declares the host key path before the suite starts (docs/console/contract.md, "Host-injected keys").
    MODE_ARGS+=(-fw_cfg "name=opt/breenix/hostkeys,string=qmp")
    [ -z "$ONLY" ] || MODE_ARGS+=(-fw_cfg "name=opt/breenix/case,string=$ONLY")
fi
if [ -n "$QMP_SOCKET" ]; then
    case "$QMP_SOCKET" in /*) ;; *) QMP_SOCKET="$PWD/$QMP_SOCKET" ;; esac
fi
case "$SERIAL_LOG" in /*) ;; *) SERIAL_LOG="$PWD/$SERIAL_LOG" ;; esac

cd "$ROOT"
echo "==> Commit: $(git rev-parse HEAD) dirty=$(test -z "$(git status --porcelain)" && echo 0 || echo 1)"
KERNEL="$ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
DISK="$ROOT/target/ext2-aarch64.img"

case "$MODE" in
    program) echo "==> Mode: program $PROGRAM" ;;
    suite) echo "==> Mode: suite $SUITE" ;;
    *) echo "==> Mode: $MODE" ;;
esac
# Background only build subprocesses. A lane may itself be backgrounded; the
# QEMU launcher restores boot policy independently, immediately before exec.
run_build() {
    if [ "$(uname)" = Darwin ]; then
        "$ROOT/scripts/build-background.sh" "$@"
    else
        "$@"
    fi
}
if [ "$BUILD" -eq 1 ]; then
    echo "==> Building userspace"
    run_build "$ROOT/userspace/programs/build.sh" --arch aarch64
    echo "==> Building the ext2 disk"
    run_build "$ROOT/scripts/create_ext2_disk.sh" --arch aarch64
    if [ "$MODE" = tests ]; then
        echo "==> Building the testing kernel"
    else
        echo "==> Building the production kernel"
    fi
    # The production build is the prod-profile gate's command: no features at all.
    run_build cargo build --release ${KERNEL_FEATURES[@]+"${KERNEL_FEATURES[@]}"} --target aarch64-breenix-kernel.json \
        -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
    "$ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"
fi
[ -f "$KERNEL" ] || { echo "No kernel at $KERNEL" >&2; exit 1; }
[ -f "$DISK" ] || { echo "No disk at $DISK" >&2; exit 1; }
if [ "$MODE" = suite ] && [[ ",$SUITE," == *",sdk,"* ]]; then
    # The Developer SDK suite's host side builds the SDK and its test apps and stages
    # them onto a copy of the disk (docs/suites/README.md, "Developer SDK").
    echo "==> Staging the Developer SDK suite's host results"
    SDK_DISK="$ROOT/target/ext2-aarch64-sdk.img"
    cp "$DISK" "$SDK_DISK"
    python3 "$ROOT/scripts/sdk-suite.py" --arch aarch64 --image "$SDK_DISK"
    DISK="$SDK_DISK"
fi

host_slot_acquire mac-boot
host_slot_serial "$SERIAL_LOG"
mkdir -p "$(dirname "$SERIAL_LOG")"
: > "$SERIAL_LOG"
WRITABLE="$(dirname "$SERIAL_LOG")/ext2-writable.img"
cp "$DISK" "$WRITABLE"
if [ "$GPU_GL" -eq 1 ]; then
    export BREENIX_HOST_RENDERER_JSON="$(dirname "$SERIAL_LOG")/host-renderer.json"
    rm -f "$BREENIX_HOST_RENDERER_JSON"
fi
# This disposable image is checked as a raw host file after the suite. Keep
# writes to that file, but do not make each guest barrier wait for APFS's
# power-loss durability. The test never crashes or restarts the host.

# shellcheck source=../docker/qemu/lib/qemu-host-lock.sh
source "$ROOT/docker/qemu/lib/qemu-host-lock.sh"
qemu_host_lock_acquire

# Records the boot's start; a timing-sensitive suite first waits for a quiet host
# (docs/boot-path.md, "Shared host slots").
if [ "$MODE" = suite ]; then
    host_slot_quiet "$ROOT/docs/suites/$SUITE.json"
else
    host_slot_quiet
fi
# Vigil shows the boot while it runs and scores it when it ends (does nothing without Vigil).
# A one-case run is not registered: Vigil would score it as a suite boot.
QEMU_PROFILE_ARGS=(-M virt,gic-version=3 -cpu max)
QEMU_PROFILE=$(python3 "$ROOT/scripts/qemu-arm64.py" --print-profile "$QEMU" "${QEMU_PROFILE_ARGS[@]}")
VIGIL_ID=
# A boot with other than the default 4 processors is recorded with its count, so it never stands for main.
VIGIL_CPUS=
[ "$CPUS" = 4 ] || VIGIL_CPUS=$CPUS
[ -n "$ONLY" ] || VIGIL_ID=$(BREENIX_VIGIL_CPUS="$VIGIL_CPUS" "$ROOT/scripts/vigil-record.sh" start qemu "$MODE" "$SUITE" "$SERIAL_LOG" "" "$QEMU_PROFILE")
# Vigil's BreenixScreen looks for qmp.sock in the registered boot's folder, which vigil-cli makes under the
# account's home as macOS reports it (VIGIL_TEST_HOME in Vigil's tests), not under $HOME.
# Vigil's capture takes socket paths of at most 103 bytes, counted in UTF-8 (macOS itself allows 104).
if [ "$QMP_REQUESTED" -eq 0 ] && [ -n "$VIGIL_ID" ]; then
    VIGIL_HOME=${VIGIL_TEST_HOME:-$(python3 -c 'import os, pwd; print(pwd.getpwuid(os.getuid()).pw_dir)')}
    VIGIL_BOOT_DIR="$VIGIL_HOME/Library/Application Support/Vigil/breenix/boots/$VIGIL_ID"
    VIGIL_QMP_SOCKET="$VIGIL_BOOT_DIR/qmp.sock"
    if [ ! -d "$VIGIL_BOOT_DIR" ]; then
        echo "==> Vigil QMP disabled: no boot folder at $VIGIL_BOOT_DIR"
    elif [ "$(printf '%s' "$VIGIL_QMP_SOCKET" | wc -c)" -le 103 ]; then
        QMP_SOCKET="$VIGIL_QMP_SOCKET"
    else
        echo "==> Vigil QMP disabled: socket path exceeds 103 bytes: $VIGIL_QMP_SOCKET"
    fi
fi
# A suite or shell boot takes host keys (scripts/hostkeys.py): it needs a QMP socket even if Vigil
# registered no socket and the caller named no socket.
HOSTKEYS_SOCKET=
if [ "$MODE" = suite ] || [ "$MODE" = shell ]; then
    HOSTKEYS_SOCKET="${TMPDIR:-/tmp}/breenix-hk-$$.sock"
    [ "$(printf '%s' "$HOSTKEYS_SOCKET" | wc -c)" -le 103 ] || HOSTKEYS_SOCKET="/tmp/breenix-hk-$$.sock"
fi
QMP_ARGS=()
if [ -n "$QMP_SOCKET" ]; then
    rm -f "$QMP_SOCKET"
    QMP_ARGS=(-qmp "unix:$QMP_SOCKET,server=on,wait=off")
fi
if [ -n "$HOSTKEYS_SOCKET" ]; then
    QMP_ARGS+=(-qmp "unix:$HOSTKEYS_SOCKET,server=on,wait=off")
fi
echo "==> Booting (serial: $SERIAL_LOG; Ctrl-A X quits)"
[ -z "$QMP_SOCKET" ] || echo "==> QMP: $QMP_SOCKET (scripts/qmp-screendump.py $(printf '%q' "$QMP_SOCKET") out.png)"
# Wait interruptibly and register the watcher so a signal to this script also
# stops and reaps its QEMU before the host lock is released.
trap 'exit 143' TERM
trap 'exit 129' HUP
trap 'exit 130' INT
exec 3<&0
set +e
HOSTKEYS_PID=
if [ -n "$HOSTKEYS_SOCKET" ]; then
    python3 "$ROOT/scripts/hostkeys.py" watch --serial "$SERIAL_LOG" --backend "qmp:$HOSTKEYS_SOCKET" \
        --log "$SERIAL_LOG.hostkeys" --parent "$$" > /dev/null 2>&1 &
    HOSTKEYS_PID=$!
fi
python3 "$ROOT/scripts/watch-qemu.py" --serial "$SERIAL_LOG" --mode "$MODE" \
    --suite "$SUITE" --case "$ONLY" --idle-exit "$IDLE_EXIT" --gate-timeout "$GATE_TIMEOUT" --disk "$WRITABLE" -- python3 "$ROOT/scripts/qemu-arm64.py" "$QEMU" \
    -rtc base=utc "${QEMU_PROFILE_ARGS[@]}" -m 512 -smp "$CPUS" \
    -kernel "$KERNEL" \
    "${DISPLAY_ARGS[@]}" -no-reboot \
    ${MODE_ARGS[@]+"${MODE_ARGS[@]}"} \
    ${QMP_ARGS[@]+"${QMP_ARGS[@]}"} \
    ${GPU_ARGS[@]+"${GPU_ARGS[@]}"} \
    -device virtio-gpu-device \
    -device virtio-keyboard-device \
    -device virtio-tablet-device \
    -device virtio-blk-device,drive=ext2 \
    -drive if=none,id=ext2,format=raw,cache=unsafe,aio=threads,file="$WRITABLE" \
    -device virtio-net-device,netdev=net0 \
    -netdev user,id=net0 \
    -chardev stdio,id=con,mux=on,signal=off,logfile="$SERIAL_LOG" \
    -serial chardev:con -mon chardev=con,mode=readline <&3 &
WATCHER_PID=$!
qemu_host_lock_track_pid "$WATCHER_PID"
wait "$WATCHER_PID"
code=$?
set -e
[ -z "$HOSTKEYS_PID" ] || kill "$HOSTKEYS_PID" 2>/dev/null || true
[ -z "$HOSTKEYS_SOCKET" ] || rm -f "$HOSTKEYS_SOCKET"
echo
echo "==> VM stopped (exit $code)"
if [ "$GPU_GL" -eq 1 ]; then
    [ -z "${SPICE_SOCKET:-}" ] || rm -f "$SPICE_SOCKET"
    if ! python3 "$ROOT/scripts/check-host-renderer.py" "$BREENIX_HOST_RENDERER_JSON" --serial "$SERIAL_LOG"; then
        [ "$code" -ne 0 ] || code=1
    fi
fi
host_slot_header "$SERIAL_LOG"
"$ROOT/scripts/vigil-record.sh" finish "$VIGIL_ID" "$code"

# Propagate the watcher status; suite, probe and tests include a UTC verdict.
exit "$code"
