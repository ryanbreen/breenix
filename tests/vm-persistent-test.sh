#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
launcher="$root/scripts/vm-persistent.sh"
path="$PATH:/opt/homebrew/opt/e2fsprogs/sbin:/usr/local/opt/e2fsprogs/sbin"
export PATH="$path"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/vm-persistent-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
reject() { if "$launcher" "$@" > /dev/null 2>&1; then fail "accepted: $*"; fi; }
reject
reject qemu
reject qemu build
reject qemu unknown
reject qemu stop
reject qemu build --system-image
mkdir -p "$tmp/tree/etc/breenix"
printf 'sentinel\n' > "$tmp/tree/etc/breenix/normal-image"
printf 'suite users\n' > "$tmp/tree/etc/breenix/boot-target"
printf 'user fixture\nuid 1234\n' > "$tmp/operator"
chmod 600 "$tmp/operator"
truncate -s 16M "$tmp/source.img"
mke2fs -q -t ext2 -F -d "$tmp/tree" "$tmp/source.img"
"$launcher" qemu build --system-image "$tmp/source.img" --operator-account "$tmp/operator" --run-dir "$tmp/run" > /dev/null
[ "$(stat -f '%Lp' "$tmp/run" 2>/dev/null || stat -c '%a' "$tmp/run")" = 700 ] || fail 'run directory is not private'
[ -f "$tmp/run/system.img" ] && [ -f "$tmp/run/home.img" ] || fail 'missing disk image'
listing=$(debugfs -R 'ls -p /etc/breenix' "$tmp/run/system.img" 2>/dev/null)
[[ "$listing" == *'/normal-image/'* ]] || fail 'normal image contents missing'
[[ "$listing" == *'/operator-account/'* ]] || fail 'operator account missing'
[[ "$listing" != *'/boot-target/'* ]] || fail 'boot target still present'
[ "$(debugfs -R 'cat /etc/breenix/operator-account' "$tmp/run/system.img" 2>/dev/null)" = "$(cat "$tmp/operator")" ] || fail 'operator account differs'
debugfs -R 'ls -p /' "$tmp/run/home.img" 2>/dev/null | grep -q '/lost+found/' || fail 'home is not ext2'
debugfs -R 'ls -p /etc/breenix' "$tmp/source.img" 2>/dev/null | grep -q '/boot-target/' || fail 'source image was modified'
reject qemu build --system-image "$tmp/source.img" --operator-account "$tmp/operator" --run-dir "$tmp/run"
"$launcher" qemu stop --run-dir "$tmp/run"
[ ! -e "$tmp/run" ] || fail 'stop did not delete run directory'
echo 'vm-persistent argument and image layout checks passed'

# Exercise the launcher through its script helpers, without a VM or host slot.
# Keep socket paths short even when macOS supplies a long TMPDIR.
fixture=$(mktemp -d /tmp/vpm-test.XXXXXX)
trap 'rm -rf "$tmp" "$fixture"' EXIT
mkdir -p "$fixture/scripts" "$fixture/bin" "$fixture/run"
cp "$launcher" "$fixture/scripts/vm-persistent.sh"
cp "$root/scripts/vm-serial-pump.py" "$fixture/scripts/vm-serial-pump.py"
cp "$root/scripts/qemu-arm64.py" "$fixture/scripts/qemu-arm64.py"
cat > "$fixture/scripts/host-slots.sh" <<'SH'
host_slots_start() { exec "$@"; }
host_slot_acquire() { :; }
host_slot_serial() { :; }
host_slot_quiet() { :; }
host_slot_header() { :; }
SH
cat > "$fixture/scripts/qemu-1x-app.sh" <<'SH'
#!/bin/sh
echo qemu-system-aarch64
SH
cat > "$fixture/scripts/vigil-record.sh" <<'SH'
#!/bin/sh
if [ "$1" = start ]; then
    printf '%s\n' "$@" > "$CAPTURE/vigil-args"
    echo fixture-boot
fi
SH
cat > "$fixture/bin/qemu-system-aarch64" <<'SH'
#!/bin/sh
printf '%s\n' "$@" > "$CAPTURE/qemu-args"
SH
chmod +x "$fixture/scripts/"*.sh "$fixture/bin/qemu-system-aarch64"
touch "$fixture/kernel" "$fixture/run/system.img" "$fixture/run/home.img" "$fixture/run/.breenix-vm-persistent"
boot_fixture() {
    env -u BREENIX_VM_PERSISTENT_SUPERVISED \
        PATH="$fixture/bin:$PATH" CAPTURE="$fixture" TMPDIR="$fixture" \
        BREENIX_QEMU_UI_LAUNCHER=env \
        "$fixture/scripts/vm-persistent.sh" qemu boot \
        --run-dir "$fixture/run" --kernel "$fixture/kernel" "$@"
}
for mode in console desktop; do
    boot_fixture --mode "$mode" || fail "boot refused mode $mode"
    python3 - "$fixture" "$mode" <<'PY'
from pathlib import Path
import sys
root, mode = Path(sys.argv[1]), sys.argv[2]
args = (root / 'qemu-args').read_text().splitlines()
settings = [args[i + 1] for i, arg in enumerate(args[:-1]) if arg == '-fw_cfg']
assert settings == [f'name=opt/breenix/mode,string={mode}'], args
assert (root / 'run/mode').read_text() == mode + '\n'
assert (root / 'vigil-args').read_text().splitlines()[:3] == ['start', 'qemu', mode]
assert not (root / 'run/boot.lock').exists()
PY
done
for invalid in missing unknown; do
    rm -f "$fixture/qemu-args" "$fixture/vigil-args" "$fixture/run/mode"
    args=()
    [ "$invalid" = missing ] || args=(--mode unknown)
    code=0
    boot_fixture "${args[@]}" > "$fixture/rejected-output" 2>&1 || code=$?
    [ "$code" = 2 ] || fail "$invalid mode returned $code instead of usage error"
    [ ! -e "$fixture/qemu-args" ] && [ ! -e "$fixture/vigil-args" ] && \
        [ ! -e "$fixture/run/mode" ] && [ ! -d "$fixture/run/boot.lock" ] || fail "$invalid mode reached launch setup"
done
echo 'vm-persistent boot mode checks passed'
