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
printf 'root:x:0:0:root:/root:/bin/bsh\n' > "$tmp/tree/etc/passwd"
printf 'root:x:0:\nwheel:x:10:\n' > "$tmp/tree/etc/group"
printf 'root:!:::::::\n' > "$tmp/tree/etc/shadow"
ssh-keygen -q -t ed25519 -N '' -f "$tmp/operator-key"
export BREENIX_VM_SITE="$tmp/site"
python3 - "$root" "$tmp" <<'PYSITE'
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1] + '/scripts')
import vm_site
root = Path(sys.argv[2])
values = dict(OPERATOR_USER='operator', OPERATOR_UID='1234', OPERATOR_PASSWORD='test-password',
              OPERATOR_PASSWORD_HASH='$6$fixture$' + 'A' * 86,
              OPERATOR_SSH_PUBLIC_KEY=(root / 'operator-key.pub').read_text().strip(),
              CONSOLE_USER='root', CONSOLE_PASSWORD='console-password',
              ROOT_PASSWORD_HASH='$6$console$' + 'B' * 86, SNTP_SERVERS='192.0.2.123')
site = root / 'site'
site.touch(mode=0o600)
site.write_text(''.join(k + '=' + v + '\n' for k, v in values.items()))
(root / 'operator').write_text(vm_site.metal_root.operator_account(values))
PYSITE
printf 'sentinel\n' > "$tmp/tree/etc/breenix/normal-image"
printf 'suite users\n' > "$tmp/tree/etc/breenix/boot-target"
chmod 600 "$tmp/operator"
truncate -s 16M "$tmp/source.img"
mke2fs -q -t ext2 -F -d "$tmp/tree" "$tmp/source.img"
"$launcher" qemu build --system-image "$tmp/source.img" --operator-account "$tmp/operator" --run-dir "$tmp/run" > /dev/null
[ "$(stat -f '%Lp' "$tmp/run" 2>/dev/null || stat -c '%a' "$tmp/run")" = 700 ] || fail 'run directory is not private'
[ -f "$tmp/run/system.img" ] && [ -f "$tmp/run/home.img" ] || fail 'missing disk image'
[ "$(stat -f '%Lp' "$tmp/run/power-token" 2>/dev/null || stat -c '%a' "$tmp/run/power-token")" = 600 ] || fail 'host token mode'
[[ $(cat "$tmp/run/power-token") =~ ^[a-f0-9]{64}$ ]] && [ "$(wc -c < "$tmp/run/power-token" | tr -d ' ')" = 64 ] || fail 'invalid token'
[ "$(debugfs -R 'cat /etc/breenix/power-token' "$tmp/run/system.img" 2>/dev/null)" = "$(cat "$tmp/run/power-token")" ] || fail 'guest token differs'
debugfs -R 'stat /etc/breenix/power-token' "$tmp/run/system.img" 2>/dev/null | grep -Eq 'Mode: +0400' || fail 'guest token mode'
listing=$(debugfs -R 'ls -p /etc/breenix' "$tmp/run/system.img" 2>/dev/null)
[[ "$listing" == *'/normal-image/'* ]] || fail 'normal image contents missing'
[[ "$listing" == *'/operator-account/'* ]] || fail 'operator account missing'
[[ "$listing" != *'/boot-target/'* ]] || fail 'boot target still present'
[ "$(debugfs -R 'cat /etc/breenix/operator-account' "$tmp/run/system.img" 2>/dev/null)" = "$(cat "$tmp/operator")" ] || fail 'operator account differs'
[ "$(debugfs -R 'cat /.breenix-shared/etc/passwd' "$tmp/run/system.img" 2>/dev/null)" = 'operator:x:1234:1234:operator:/home/operator:/bin/bsh' ] || fail 'operator store account missing'
[ "$(debugfs -R 'cat /.breenix-shared/etc/group' "$tmp/run/system.img" 2>/dev/null)" = "$(printf 'operator:x:1234:\nwheel:x:10:operator')" ] || fail 'operator store groups missing'
[ "$(debugfs -R 'cat /.breenix-shared/var/lib/accounts/pending/operator' "$tmp/run/system.img" 2>/dev/null)" = "$(cat "$tmp/operator-key.pub")" ] || fail 'operator authorized key not staged'
[ "$(debugfs -R 'cat /etc/shadow' "$tmp/run/system.img" 2>/dev/null)" = 'root:$6$console$BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB:::::::' ] || fail 'root console hash missing'
[ "$(debugfs -R 'cat /etc/breenix/sntp-servers' "$tmp/run/system.img" 2>/dev/null)" = '192.0.2.123' ] || fail 'SNTP source missing'
debugfs -R 'stat /.breenix-shared' "$tmp/run/system.img" 2>/dev/null | grep -q 'Mode:  0700' || fail 'machine store is not private'
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
mkdir -p "$fixture/scripts/metal" "$fixture/bin" "$fixture/run"
git -C "$fixture" init -q
git -C "$fixture" -c user.name=Fixture -c user.email=fixture@example.invalid commit -q --allow-empty -m fixture
cp "$root/scripts/metal/metal_lib.py" "$fixture/scripts/metal/metal_lib.py"
cp "$launcher" "$fixture/scripts/vm-persistent.sh"
cp "$root/scripts/vm-serial-pump.py" "$fixture/scripts/vm-serial-pump.py"
cp "$root/scripts/vm-artifacts.py" "$fixture/scripts/vm-artifacts.py"
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
cat > "$fixture/scripts/build-qemu-gl.sh" <<'SH'
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
printf '%064d\n' 0 > "$fixture/run/power-token"
touch "$fixture/kernel" "$fixture/run/system.img" "$fixture/run/home.img" "$fixture/run/.breenix-vm-persistent"
python3 "$fixture/scripts/vm-artifacts.py" seal "$fixture" "$fixture/run" ""
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
# QEMU assigns virtio-mmio buses backwards; root must enumerate before home.
assert args.index('virtio-blk-device,drive=home') < args.index('virtio-blk-device,drive=ext2'), args
if sys.platform == 'darwin':
    assert 'virtio-gpu-gl-pci' in args and 'cocoa,gl=es' in args, args
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

# Exercise the actual launcher and pump with a socket-speaking QEMU stand-in.
# The stub writes guest disk bytes and supplies either an authenticated poweroff
# or a QMP termination reason, without starting a virtual machine.
cat > "$fixture/bin/qemu-system-aarch64" <<'PY'
#!/usr/bin/env python3
import hashlib, json, os, socket, sys, time
from pathlib import Path
args = sys.argv[1:]
root = Path(os.environ['CAPTURE'])
run = root / 'run'
(root / 'qemu-args').write_text('\n'.join(args) + '\n')
serial = socket.socket(socket.AF_UNIX)
serial.connect(str(run / 'serial.sock'))
listener = socket.socket(socket.AF_UNIX)
path = run / 'events.sock'
path.unlink(missing_ok=True)
listener.bind(str(path)); listener.listen(1); listener.settimeout(5)
qmp, _ = listener.accept()
qmp.sendall(b'{"QMP":{}}\n')
qmp.recv(4096)
scenario = os.environ['STUB_SCENARIO']
if scenario == 'poweroff':
    nonce = 'a' * 64
    token = (run / 'power-token').read_text().strip()
    receipt = hashlib.sha256((token + nonce).encode()).hexdigest()
    serial.sendall(f'[init] shutdown nonce={nonce}\n[shutdown] requested poweroff token={receipt}\n'.encode())
    time.sleep(.2)
    for name in ('system.img', 'home.img'):
        with (run / name).open('ab') as f: f.write(b'guest writes')
    reason = 'guest-shutdown'
elif scenario == 'reset':
    reason = 'guest-reset'
else:
    reason = 'host-qmp-quit'
qmp.sendall(json.dumps({'event':'SHUTDOWN','data':{'reason':reason}}).encode() + b'\n')
time.sleep(.3)
serial.close(); qmp.close(); listener.close(); path.unlink()
PY
chmod +x "$fixture/bin/qemu-system-aarch64"
# Ensure source attestation sees a clean commit while retaining fixture scripts.
printf '*\n' > "$fixture/.gitignore"
for file in docs/suites/server.json docs/suites/users.json docs/efforts/path.json docs/boot-path.json; do
    mkdir -p "$fixture/$(dirname "$file")"
    cp "$root/$file" "$fixture/$file"
    git -C "$fixture" add -f "$file"
done
git -C "$fixture" add -f .gitignore
git -C "$fixture" -c user.name=Fixture -c user.email=fixture@example.invalid commit -qm ignore
cp "$fixture/kernel" "$fixture/run/kernel"
cp "$fixture/kernel" "$fixture/run/fault-kernel"
python3 "$fixture/scripts/vm-artifacts.py" seal "$fixture" "$fixture/run" "$(git -C "$fixture" rev-parse HEAD)"
STUB_SCENARIO=poweroff boot_fixture --mode console || fail 'authenticated poweroff launch failed'
STUB_SCENARIO=host boot_fixture --mode console || fail 'continuation rejected'
python3 - "$fixture/run" <<'PY'
import json, sys
from pathlib import Path
run = Path(sys.argv[1])
assert json.loads((run / 'verified-build.json').read_text())['evidenceEligible'] is True
import hashlib
record = json.loads((run / 'boots' / (run / 'current').read_text().strip() / 'run.json').read_text())
assert record['evidenceEligible'] is True
for disk in ('system.img', 'home.img'):
    assert record['artifacts'][disk] == hashlib.sha256((run / disk).read_bytes()).hexdigest()
PY
printf x >> "$fixture/run/system.img"
if STUB_SCENARIO=host boot_fixture --mode console > /dev/null 2>&1; then fail 'changed continuation disk accepted'; fi
# Restore the sealed continuation bytes before testing the diskless fault run.
truncate -s -1 "$fixture/run/system.img"
for scenario in reset host; do
    env -u BREENIX_VM_PERSISTENT_SUPERVISED PATH="$fixture/bin:$PATH" CAPTURE="$fixture" TMPDIR="$fixture" \
        BREENIX_QEMU_UI_LAUNCHER=env STUB_SCENARIO="$scenario" \
        "$fixture/scripts/vm-persistent.sh" qemu fault-boot --run-dir "$fixture/run" || fail 'fault boot failed'
    python3 - "$fixture" "$scenario" <<'PY'
import json, sys
from pathlib import Path
root, scenario = Path(sys.argv[1]), sys.argv[2]
args = (root / 'qemu-args').read_text().splitlines()
assert args[args.index('-action') + 1] == 'reboot=shutdown'
assert '-drive' not in args and not any('virtio-blk' in a for a in args)
run = root / 'run'
boot = run / 'boots' / (run / 'current').read_text().strip()
record = json.loads((boot / 'run.json').read_text())
assert record['kind'] == 'fault-test' and record['bootedCommit'] == record['toolsCommit']
events = [json.loads(x) for x in (boot / 'events.jsonl').read_text().splitlines()]
assert any(e['event'] == 'panic-reset-observed' for e in events) == (scenario == 'reset')
PY
done
echo 'vm-persistent continuation and fault boot checks passed'
