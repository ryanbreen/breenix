#!/usr/bin/env bash
# Disposable ARM64 QEMU disks. Build from the normal ARM image and a prepared
# installer-format operator-account file; boot stays in the foreground so it
# holds the mac-boot lease. Stop from another terminal to remove the run.
#
# vm-persistent.sh qemu build --run-dir DIR [--system-image IMAGE] --operator-account FILE [--fault panic-reboot|watchdog]
# vm-persistent.sh qemu boot --run-dir DIR --mode console|desktop [--kernel FILE]
# vm-persistent.sh qemu fault-boot --run-dir DIR [--inject feeder-hang|health-hang]
# vm-persistent.sh qemu reboot --run-dir DIR
# vm-persistent.sh qemu stop --run-dir DIR
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
umask 077
usage() { echo 'usage: vm-persistent.sh qemu build|boot|fault-boot|reboot|stop --run-dir DIR [--operator-account FILE [--system-image IMAGE | --fault panic-reboot|watchdog] | --mode console|desktop [--kernel FILE] | --inject feeder-hang|health-hang]' >&2; exit 2; }
[ "$#" -ge 2 ] && [ "$1" = qemu ] || usage
op=$2; shift 2
case "$op" in build|boot|fault-boot|reboot|stop) ;; *) usage ;; esac
run= system= account= kernel= mode= fault= inject=
while [ "$#" -gt 0 ]; do
    [ "$#" -ge 2 ] && [ -n "$2" ] || usage
    case "$1" in
        --run-dir) [ -z "$run" ] || usage; run=$2 ;;
        --system-image) [ -z "$system" ] || usage; system=$2 ;;
        --operator-account) [ -z "$account" ] || usage; account=$2 ;;
        --kernel) [ -z "$kernel" ] || usage; kernel=$2 ;;
        --fault) [ -z "$fault" ] || usage; fault=$2 ;;
        --mode) [ -z "$mode" ] || usage; mode=$2 ;;
        --inject) [ -z "$inject" ] || usage; inject=$2 ;;
        *) usage ;;
    esac
    shift 2
done
[ -n "$run" ] || usage
case "$op" in
    build) [ -z "$inject" ] && [ -n "$account" ] && [ -z "$kernel" ] && [ -z "$mode" ] || usage
           case "$fault" in ""|panic-reboot|watchdog) ;; *) usage ;; esac
           [ -z "$fault" ] || [ -z "$system" ] || usage ;;
    boot) [ -z "$inject" ] && [ -z "$fault" ] && [ -z "$system" ] && [ -z "$account" ] || usage
          case "$mode" in console|desktop) ;; *) usage ;; esac ;;
    fault-boot) [ -z "$system$account$kernel$mode$fault" ] || usage
                case "$inject" in ""|feeder-hang|health-hang) ;; *) usage ;; esac; mode=console ;;
    reboot|stop) [ -z "$inject" ] && [ -z "$fault" ] && [ -z "$system" ] && [ -z "$account" ] && [ -z "$kernel" ] && [ -z "$mode" ] || usage ;;
esac
# debugfs command files and QEMU comma-separated options cannot safely encode
# these characters; reject them instead of interpreting caller-supplied syntax.
case "$run" in *[!a-zA-Z0-9_./-]*) echo 'invalid run directory path' >&2; exit 2 ;; esac
case "$run" in /*) ;; *) run="$PWD/$run" ;; esac
if [ "$op" = build ]; then
    [ -f "$account" ] && { [ -z "$system" ] || [ -f "$system" ]; } || { echo 'system image and operator account must exist' >&2; exit 1; }
    mkdir -m 700 "$run" || exit 1
    run=$(cd "$run" && pwd -P)
    trap 'rm -rf -- "$run"' EXIT
    # Supplied images remain usable for diagnostics, but cannot attest a commit.
    # The source path rebuilds both outputs before sealing their actual bytes.
    source_commit=
    if [ -z "$system" ]; then
        [ -z "$(git -C "$ROOT" status --porcelain)" ] || { echo 'source build requires a clean checkout' >&2; exit 1; }
        source_commit=$(git -C "$ROOT" rev-parse HEAD)
        export CARGO_BUILD_JOBS=4
        unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR
        (cd "$ROOT" && taskpolicy -b cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64)
        cp "$ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64" "$run/kernel"
        if [ -n "$fault" ]; then
            feature=server_reboot_fault
            [ "$fault" != watchdog ] || feature=watchdog_fault_inject
            (cd "$ROOT" && taskpolicy -b cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 --features "$feature")
            cp "$ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64" "$run/fault-kernel"
            printf '%s\n' "$fault" > "$run/fault-kind"
            cp "$run/kernel" "$ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
        fi
        taskpolicy -b "$ROOT/userspace/programs/build.sh" --arch aarch64
        BREENIX_REPRODUCIBLE_EXT2=1 taskpolicy -b "$ROOT/scripts/create_ext2_disk.sh" --arch aarch64
        [ "$source_commit" = "$(git -C "$ROOT" rev-parse HEAD)" ] && [ -z "$(git -C "$ROOT" status --porcelain)" ] || { echo 'sources changed during build' >&2; exit 1; }
        system="$ROOT/target/ext2-aarch64.img"
    fi
    cp "$system" "$run/system.img"
    cp "$account" "$run/operator-account"
    chmod 600 "$run/system.img" "$run/operator-account"
    PATH="$PATH:/opt/homebrew/opt/e2fsprogs/sbin:/usr/local/opt/e2fsprogs/sbin"
    export PATH
    command -v debugfs >/dev/null && command -v mke2fs >/dev/null || { echo 'e2fsprogs is required' >&2; exit 1; }
    python3 "$ROOT/scripts/vm_site.py" power-token --out "$run/power-token"
    # debugfs returns success even when a command fails; read back each change.
    debugfs -w -R 'rm /etc/breenix/boot-target' "$run/system.img" >/dev/null 2>&1 || true
    debugfs -w -R 'rm /etc/breenix/power-token' "$run/system.img" >/dev/null 2>&1 || true
    debugfs -w -R "write $run/power-token /etc/breenix/power-token" "$run/system.img" >/dev/null 2>&1
    debugfs -w -R 'set_inode_field /etc/breenix/power-token mode 0100400' "$run/system.img" >/dev/null 2>&1
    debugfs -w -R 'set_inode_field /etc/breenix/power-token uid 0' "$run/system.img" >/dev/null 2>&1
    debugfs -w -R 'set_inode_field /etc/breenix/power-token gid 0' "$run/system.img" >/dev/null 2>&1
    debugfs -R 'cat /etc/breenix/power-token' "$run/system.img" 2>/dev/null > "$run/readback"
    cmp -s "$run/power-token" "$run/readback" || { echo 'power token staging failed' >&2; exit 1; }
    token_stat=$(debugfs -R 'stat /etc/breenix/power-token' "$run/system.img" 2>/dev/null)
    [[ $token_stat =~ Mode:[[:space:]]+0400 ]] && [[ $token_stat =~ User:[[:space:]]+0[[:space:]]+Group:[[:space:]]+0 ]] || { echo 'power token metadata staging failed' >&2; exit 1; }
    debugfs -w -R 'rm /etc/breenix/operator-account' "$run/system.img" >/dev/null 2>&1 || true
    debugfs -w -R "write $run/operator-account /etc/breenix/operator-account" "$run/system.img" >/dev/null 2>&1
    debugfs -w -R 'set_inode_field /etc/breenix/operator-account mode 0100600' "$run/system.img" >/dev/null 2>&1
    debugfs -R 'cat /etc/breenix/operator-account' "$run/system.img" 2>/dev/null > "$run/readback"
    cmp -s "$run/operator-account" "$run/readback" || { echo 'operator account staging failed' >&2; exit 1; }
    if debugfs -R 'stat /etc/breenix/boot-target' "$run/system.img" 2>/dev/null | grep -q 'Inode:'; then
        echo 'boot target removal failed' >&2; exit 1
    fi
    rm "$run/operator-account" "$run/readback"
    python3 "$ROOT/scripts/vm-image.py" "$run/system.img" "$account"
    truncate -s 1G "$run/home.img"
    mke2fs -q -t ext2 -F "$run/home.img"
    python3 "$ROOT/scripts/vm-artifacts.py" seal "$ROOT" "$run" "$source_commit"
    : > "$run/.breenix-vm-persistent"
    trap - EXIT
    echo "$run"
    exit 0
fi
[ -d "$run" ] && [ ! -L "$run" ] && [ -f "$run/.breenix-vm-persistent" ] || { echo 'not a persistent VM run directory' >&2; exit 1; }
run=$(cd "$run" && pwd -P)
if [ "$op" = reboot ]; then
    exec python3 "$ROOT/scripts/vm-reboot.py" "$run"
fi
if [ "$op" = stop ]; then
    if [ -d "$run/boot.lock" ]; then
        python3 - "$run/qmp.sock" <<'PY'
import json, socket, sys
s = socket.socket(socket.AF_UNIX)
s.settimeout(10)
s.connect(sys.argv[1])
f = s.makefile('rwb', buffering=0)
def response():
    while True:
        message = json.loads(f.readline())
        if 'return' in message or 'error' in message:
            if 'error' in message:
                raise RuntimeError(message['error'])
            return
json.loads(f.readline())
f.write(b'{"execute":"qmp_capabilities"}\n')
response()
f.write(b'{"execute":"quit"}\n')
response()
PY
        # The boot owner removes this only after QEMU has exited.
        for _ in {1..100}; do [ -d "$run/boot.lock" ] || break; sleep .1; done
        [ ! -d "$run/boot.lock" ] || { echo 'QEMU has not exited; run retained' >&2; exit 1; }
    fi
    rm -rf -- "$run"
    exit 0
fi
if [ "$op" = fault-boot ]; then kernel="$run/fault-kernel"; fi
if [ -z "$kernel" ]; then
    kernel="$run/kernel"
    [ -f "$kernel" ] || kernel="$ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
fi
[ -f "$kernel" ] && [ -f "$run/system.img" ] && [ -f "$run/home.img" ] || { echo 'missing kernel or image' >&2; exit 1; }
[ ${#run} -lt 90 ] || { echo 'run path is too long for unix sockets' >&2; exit 1; }
# Re-exec under the slot supervisor before taking the run lock.
if [ -z "${BREENIX_VM_PERSISTENT_SUPERVISED:-}" ]; then
    export BREENIX_VM_PERSISTENT_SUPERVISED=1
    source "$ROOT/scripts/host-slots.sh"
    if [ "$op" = fault-boot ]; then
        args=()
        [ -z "$inject" ] || args=(--inject "$inject")
        host_slots_start "$ROOT/scripts/vm-persistent.sh" qemu fault-boot --run-dir "$run" "${args[@]}"
    else
        host_slots_start "$ROOT/scripts/vm-persistent.sh" qemu boot --run-dir "$run" --kernel "$kernel" --mode "$mode"
    fi
fi
source "$ROOT/scripts/host-slots.sh"
mkdir "$run/boot.lock" || { echo 'run is already booting' >&2; exit 1; }
pump=
cleanup() {
    if [ -n "$pump" ]; then
        kill -TERM "$pump" 2>/dev/null || true
        wait "$pump" 2>/dev/null || true
    fi
    rmdir "$run/boot.lock" 2>/dev/null || true; }
trap cleanup EXIT
verify=verify
if [ "$op" = fault-boot ]; then verify=verify-fault; fi
python3 "$ROOT/scripts/vm-artifacts.py" "$verify" "$ROOT" "$run" "$kernel"
fault_kind=
if [ "$op" = fault-boot ]; then
    fault_kind=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("faultKind", ""))' "$run/verified-build.json")
    case "$fault_kind:$inject" in
        panic-reboot:|watchdog:feeder-hang|watchdog:health-hang) ;;
        *) echo 'fault injection does not match the built kernel' >&2; exit 1 ;;
    esac
    if [ "$fault_kind" = watchdog ]; then
        PATH="$PATH:/opt/homebrew/opt/e2fsprogs/sbin:/usr/local/opt/e2fsprogs/sbin"
        printf '%s\n' "$inject" > "$run/watchdog-intent"
        debugfs -w -R 'rm /etc/breenix/watchdog-fault' "$run/system.img" >/dev/null 2>&1 || true
        debugfs -w -R "write $run/watchdog-intent /etc/breenix/watchdog-fault" "$run/system.img" >/dev/null 2>&1
        debugfs -R 'cat /etc/breenix/watchdog-fault' "$run/system.img" 2>/dev/null > "$run/watchdog-readback"
        cmp -s "$run/watchdog-intent" "$run/watchdog-readback" || { echo 'watchdog intent staging failed' >&2; exit 1; }
        python3 "$ROOT/scripts/vm-artifacts.py" stage-intent "$ROOT" "$run" ''
        rm "$run/watchdog-intent" "$run/watchdog-readback"
    fi
fi
if [ "$op" = fault-boot ]; then printf "fault-test\n" > "$run/boot-kind"; else rm -f "$run/boot-kind"; fi
kernel="$run/boot-kernel"
host_slot_acquire mac-boot
printf '%s\n' "$mode" > "$run/mode"
git -C "$ROOT" rev-parse HEAD > "$run/tools-commit"
mkdir -p "${TMPDIR:-/tmp}/breenix-boot"
serial=$(mktemp "${TMPDIR:-/tmp}/breenix-boot/vm-persistent.XXXXXX")
printf '%s\n' "$serial" > "$run/serial-path"
host_slot_serial "$serial"
host_slot_quiet
rm -f "$run/serial.sock" "$run/console.sock"
python3 "$ROOT/scripts/vm-serial-pump.py" "$run" "$serial" &
pump=$!
for _ in {1..100}; do
    [ -S "$run/serial.sock" ] && [ -S "$run/console.sock" ] && break
    kill -0 "$pump" 2>/dev/null || { echo 'serial pump exited' >&2; exit 1; }
    sleep .05
done
[ -S "$run/serial.sock" ] && [ -S "$run/console.sock" ] || { echo 'serial pump did not start' >&2; exit 1; }
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
printf '%s\n' "$port" > "$run/ssh-port"
qemu=qemu-system-aarch64
display=(-display none)
gpu=()
if [ "$(uname -s)" = Darwin ]; then
    qemu="${BREENIX_QEMU_GL_BINARY:-$("$ROOT/scripts/build-qemu-gl.sh" --print-path)}"
    defaults write org.breenix.qemu-gl NSAppSleepDisabled -bool YES
    export ANGLE_DEFAULT_PLATFORM="${ANGLE_DEFAULT_PLATFORM-metal}"
    display=(-display cocoa,gl=es)
    gpu=(-device virtio-gpu-gl-pci)
fi
profile=$(python3 "$ROOT/scripts/qemu-arm64.py" --print-profile "$qemu" -M virt,gic-version=3 -cpu max)
printf '%s\n' "$profile" > "$run/qemu-profile"
export BREENIX_VIGIL_ARCH=aarch64 BREENIX_VIGIL_DISPLAY=false
[ "$(uname -s)" != Darwin ] || export BREENIX_VIGIL_DISPLAY=true
vigil=$("$ROOT/scripts/vigil-record.sh" start qemu "$mode" '' "$serial" '' "$profile")
# On interruption stop and reap only this launch before releasing its run lock.
child=
stop_child() {
    if [ -n "$child" ]; then
        kill -TERM "$child" 2>/dev/null || true
        wait "$child" 2>/dev/null || true
        child=
    fi
    "$ROOT/scripts/vigil-record.sh" finish "$vigil" 143
    exit 143
}
trap stop_child TERM HUP INT
disks=(-device virtio-blk-device,drive=home -drive "if=none,id=home,format=raw,file=$run/home.img"
       -device virtio-blk-device,drive=ext2 -drive "if=none,id=ext2,format=raw,file=$run/system.img")
action=(-device i6300esb -action watchdog=reset)
if [ "$op" = fault-boot ] && [ "$fault_kind" = panic-reboot ]; then disks=(); action+=(-action reboot=shutdown); fi
set +e
python3 "$ROOT/scripts/qemu-arm64.py" "$qemu" -rtc base=utc -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
    -kernel "$kernel" -fw_cfg "name=opt/breenix/mode,string=$mode" "${display[@]}" -qmp "unix:$run/qmp.sock,server=on,wait=off" -qmp "unix:$run/events.sock,server=on,wait=off" \
    "${gpu[@]}" -device virtio-gpu-device -device virtio-keyboard-device -device virtio-tablet-device \
    "${disks[@]}" "${action[@]}" \
    -device virtio-rng-device -device virtio-net-device,netdev=net0 \
    -netdev "user,id=net0,hostfwd=tcp:127.0.0.1:$port-:22" \
    -chardev "socket,id=con,path=$run/serial.sock,server=off" -serial chardev:con &
child=$!
wait "$child"
code=$?
child=
# Allow the pump to consume the socket's remaining bytes before stopping it.
for _ in {1..100}; do
    kill -0 "$pump" 2>/dev/null || break
    sleep .05
done
if kill -0 "$pump" 2>/dev/null; then
    kill -TERM "$pump" 2>/dev/null || true
fi
wait "$pump" 2>/dev/null || true
pump=
set -e
if [ "$code" -eq 0 ] && [ "$op" = boot ]; then
    python3 "$ROOT/scripts/vm-artifacts.py" continuation "$ROOT" "$run" ""
fi
host_slot_header "$serial"
"$ROOT/scripts/vigil-record.sh" finish "$vigil" "$code"
exit "$code"
