#!/usr/bin/env bash
set -euo pipefail
selection=${1:-all}
[ "$#" -le 1 ] && [[ "$selection" = all || "$selection" = --migration-cycle ]] || exit 2
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
[ -f "$tmp/run/breenfs-system.img" ] && [ -f "$tmp/run/breenfs-home.img" ] || fail 'missing BreenFS image'
python3 - "$tmp/run" <<'PYUUID'
import json, subprocess, sys, uuid
from pathlib import Path
run = Path(sys.argv[1])
guids = json.loads((run / 'volume-guids.json').read_text())
for role in ('system', 'home'):
    output = subprocess.check_output(['debugfs', '-R', 'stats', run / (role + '.img')], text=True)
    assert dict(line.split(':', 1) for line in output.splitlines() if ':' in line)['Filesystem UUID'].strip() == guids[role]
    with (run / f'breenfs-{role}.img').open('rb') as image:
        image.seek(17 * 4096)
        superblock = image.read(32)
    assert superblock[:8] == b'BREENFSS' and superblock[16:32] == uuid.UUID(guids['breenfs-' + role]).bytes
PYUUID
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
[ ! -f "$root/scripts/vm_slots.py" ] || cp "$root/scripts/vm_slots.py" "$fixture/scripts/vm_slots.py"
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
printf '%s\n' "${BREENIX_QEMU_SUSPEND_CLOCK:-}" > "$CAPTURE/qemu-clock"
SH
printf '#!/bin/sh\nexit 0\n' > "$fixture/bin/defaults"
chmod +x "$fixture/scripts/"*.sh "$fixture/bin/"*
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
if [ "$selection" = all ]; then
for mode in console desktop; do
    boot_fixture --mode "$mode" || fail "boot refused mode $mode"
    python3 - "$fixture" "$mode" <<'PY'
from pathlib import Path
import sys
root, mode = Path(sys.argv[1]), sys.argv[2]
args = (root / 'qemu-args').read_text().splitlines()
settings = [args[i + 1] for i, arg in enumerate(args[:-1]) if arg == '-fw_cfg']
assert settings == [f'name=opt/breenix/mode,string={mode}'], args
assert 'i6300esb' in args and 'watchdog=reset' in args
# QEMU assigns virtio-mmio buses backwards; root must enumerate before home.
assert args.index('virtio-blk-device,drive=home') < args.index('virtio-blk-device,drive=ext2'), args
if sys.platform == 'darwin':
    assert 'virtio-gpu-gl-pci' in args and 'cocoa,gl=es' in args, args
    assert (root / 'qemu-clock').read_text().strip() == '1'
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

boot_fixture --mode console || fail 'RAW boot failed'
cp "$fixture/qemu-args" "$fixture/raw-args"
cp "$fixture/kernel" "$fixture/run/kernel"
python3 "$fixture/scripts/vm-artifacts.py" seal-slot "$fixture" "$fixture/run" "$fixture/run/kernel"
python3 - "$fixture/run" <<'PY'
import hashlib, json, sys
from pathlib import Path
run = Path(sys.argv[1])
entry = dict(kernel='kernel', kernelSha256=hashlib.sha256((run / 'kernel').read_bytes()).hexdigest(), userland='gen-1')
(run / 'slots.json').write_text(json.dumps(dict(current=entry, previous=None, next=None)))
PY
boot_fixture --mode console --slot current || fail 'slot boot failed'
python3 - "$fixture" <<'PY'
from pathlib import Path
import sys
root = Path(sys.argv[1]); run = root / 'run'
raw = (root / 'raw-args').read_text().splitlines()
args = (root / 'qemu-args').read_text().splitlines()
i = args.index('-fw_cfg', args.index('-fw_cfg') + 1)
assert args[i:i+2] == ['-fw_cfg', f'name=opt/breenix/intent,file={run}/boot-intent']
assert (run / 'boot-intent').read_bytes() == b'BREENIX-BOOT 2\ntarget persistent\nuserland gen-1\n'
del args[i:i+2]
raw = [x for x in raw if not x.startswith('user,id=net0,hostfwd=')]
args = [x for x in args if not x.startswith('user,id=net0,hostfwd=')]
assert args == raw, (args, raw)
PY
rm -f "$fixture/qemu-args"
if boot_fixture --mode console --slot next > /dev/null 2>&1; then fail 'missing next slot accepted'; fi
[ ! -e "$fixture/qemu-args" ] || fail 'missing slot reached QEMU'
printf tampered > "$fixture/run/kernel"
if boot_fixture --mode console --slot current > /dev/null 2>&1; then fail 'tampered slot accepted'; fi
[ ! -e "$fixture/qemu-args" ] || fail 'tampered slot reached QEMU'
rm "$fixture/run/slots.json"
fi

# Exercise the actual launcher and pump with a socket-speaking QEMU stand-in.
# The stub writes guest disk bytes and supplies either an authenticated poweroff
# or a QMP termination reason, without starting a virtual machine.
cat > "$fixture/bin/qemu-system-aarch64" <<'PY'
#!/usr/bin/env python3
import hashlib, json, os, socket, subprocess, sys, time
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
if scenario in ('migrate', 'cutover', 'rescue', 'return'):
    if scenario == 'migrate':
        source = root / os.environ['MIGRATE_TREE']
        guest = subprocess.run([root / 'migrate-guest', source, '--vm-poweroff'],
                               capture_output=True, timeout=10)
        serial.sendall(guest.stdout + guest.stderr)
        # Model the VM power boundary. A userspace exit does not power off QEMU.
        if not (source / 'poweroff-requested').is_file():
            sys.exit('migrator exited without requesting guest poweroff')
        nonce = 'a' * 64
        token = (run / 'power-token').read_text().strip()
        receipt = hashlib.sha256((token + nonce).encode()).hexdigest()
        serial.sendall(f'[init] shutdown nonce={nonce}\n[shutdown] requested poweroff token={receipt}\n'.encode())
    else:
        kind = 'ext2' if scenario == 'rescue' else 'breenfs'
        serial.sendall(f'[boot] {kind} root filesystem mounted\n'.encode())
        serial.sendall(b'bsshd: listening on 0.0.0.0:22\n')
        nonce = 'a' * 64
        token = (run / 'power-token').read_text().strip()
        receipt = hashlib.sha256((token + nonce).encode()).hexdigest()
        serial.sendall(f'[init] shutdown nonce={nonce}\n[shutdown] requested poweroff token={receipt}\n'.encode())
        time.sleep(.1)
    reason = 'guest-shutdown'
elif scenario == 'poweroff':
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
if [ "$selection" = all ]; then
cp "$fixture/kernel" "$fixture/run/kernel"
cp "$fixture/kernel" "$fixture/run/fault-kernel"
printf "panic-reboot\n" > "$fixture/run/fault-kind"
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
assert 'reboot=shutdown' in args and 'watchdog=reset' in args
assert 'i6300esb' in args
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

# Watchdog faults retain both disks and stage the requested intent before launch.
reject_fault() {
    if env -u BREENIX_VM_PERSISTENT_SUPERVISED PATH="$fixture/bin:$PATH" CAPTURE="$fixture" TMPDIR="$fixture" BREENIX_QEMU_UI_LAUNCHER=env STUB_SCENARIO=host "$fixture/scripts/vm-persistent.sh" qemu fault-boot --run-dir "$fixture/run" "$@" > /dev/null 2>&1; then fail "accepted fault: $*"; fi
}
reject_fault --inject feeder-hang
reject_fault --inject unknown
rm "$fixture/run/fault-kind" "$fixture/run/fault-kernel"
python3 "$fixture/scripts/vm-artifacts.py" seal "$fixture" "$fixture/run" "$(git -C "$fixture" rev-parse HEAD)"
reject_fault --inject feeder-hang
# Use a real ext2 image for debugfs staging and seal it as a watchdog build.
cp "$tmp/source.img" "$fixture/run/system.img"
cp "$fixture/kernel" "$fixture/run/fault-kernel"
printf 'watchdog\n' > "$fixture/run/fault-kind"
rm -f "$fixture/run/continuation.json"
python3 "$fixture/scripts/vm-artifacts.py" seal "$fixture" "$fixture/run" "$(git -C "$fixture" rev-parse HEAD)"
for inject in feeder-hang health-hang; do
    env -u BREENIX_VM_PERSISTENT_SUPERVISED PATH="$fixture/bin:$PATH" CAPTURE="$fixture" TMPDIR="$fixture" BREENIX_QEMU_UI_LAUNCHER=env STUB_SCENARIO=host "$fixture/scripts/vm-persistent.sh" qemu fault-boot --run-dir "$fixture/run" --inject "$inject" || fail "watchdog fault boot $inject"
    [ "$(debugfs -R 'cat /etc/breenix/watchdog-fault' "$fixture/run/system.img" 2>/dev/null)" = "$inject" ] || fail 'watchdog intent readback'
    python3 - "$fixture/qemu-args" <<'PYARGS'
import sys
a = open(sys.argv[1]).read().splitlines()
assert 'i6300esb' in a and 'watchdog=reset' in a and 'reboot=shutdown' not in a
assert 'virtio-blk-device,drive=home' in a and 'virtio-blk-device,drive=ext2' in a
PYARGS
done
echo 'vm-persistent watchdog fault checks passed'
fi

# Cold readers run against disposable fixture files, with the fake guest
# supplying only the distinct serial outcome expected for each cycle stage.
for name in metal_image metal_persist metal_probe metal_root migrate_manifest; do
    cp "$root/scripts/metal/$name.py" "$fixture/scripts/metal/"
done
run="$fixture/run"
rm -f "$run/continuation.json" "$run/fault-kind" "$run/fault-kernel" "$run/slot-kernels.json"
cp "$fixture/kernel" "$run/kernel"
mkdir -p "$fixture/old-system/.breenix-shared" "$fixture/old-system/root" \
    "$fixture/old-system/.breenix-generations/gen-1" "$fixture/old-home"
printf data > "$fixture/old-system/.breenix-generations/gen-1/payload"
for role in system home; do
    truncate -s 8M "$run/$role.img"
    mke2fs -q -F -t ext2 -d "$fixture/old-$role" "$run/$role.img"
done
cp "$run/system.img" "$run/test-root.img"
touch "$run/breenfs-system.img" "$run/breenfs-home.img"
cat > "$run/volume-guids.json" <<'JSON'
{"test-root":"11111111-1111-1111-1111-111111111111","system":"22222222-2222-2222-2222-222222222222","home":"33333333-3333-3333-3333-333333333333","breenfs-system":"44444444-4444-4444-4444-444444444444","breenfs-home":"55555555-5555-5555-5555-555555555555"}
JSON
python3 "$fixture/scripts/vm-artifacts.py" seal "$fixture" "$run" "$(git -C "$fixture" rev-parse HEAD)"
python3 "$fixture/scripts/vm-artifacts.py" seal-slot "$fixture" "$run" "$run/kernel"
python3 - "$run" <<'PYSLOT'
import hashlib, json, sys
from pathlib import Path
run = Path(sys.argv[1])
entry = dict(kernel='kernel', kernelSha256=hashlib.sha256((run / 'kernel').read_bytes()).hexdigest(), userland='gen-1')
(run / 'slots.json').write_text(json.dumps(dict(current=entry, previous=None, next=None)))
PYSLOT
reader="$fixture/scripts/metal/migrate_manifest.py"
python3 "$reader" ext2 "$run/system.img" --subtree /.breenix-generations/gen-1 > "$fixture/generation.tsv"
python3 "$reader" ext2 "$run/system.img" | awk -F '\t' '$1 == "2f" || $1 == "2f2e627265656e69782d736861726564" || $1 == "2f726f6f74"' > "$fixture/shared.tsv"
python3 "$reader" ext2 "$run/home.img" > "$fixture/home.tsv"
cat > "$fixture/bin/breenfs-check" <<'FAKECHECK'
#!/bin/sh
[ "$1" = --full ] && [ -f "$2" ]
FAKECHECK
cat > "$fixture/bin/breenfs-extract" <<'FAKEEXTRACT'
#!/bin/sh
set -eu
case "${1##*/}:$*" in
    breenfs-system.img:*' migration-manifest --root gen-gen-1') cat "$CAPTURE/generation.tsv" ;;
    breenfs-system.img:*' migration-manifest') cat "$CAPTURE/shared.tsv" ;;
    breenfs-home.img:*' migration-manifest') cat "$CAPTURE/home.tsv" ;;
    *) exit 2 ;;
esac
FAKEEXTRACT
chmod +x "$fixture/bin/breenfs-check" "$fixture/bin/breenfs-extract"
export BREENFS_TOOLS_DIR="$fixture/bin" CAPTURE="$fixture"
cycle() { "$fixture/scripts/vm-persistent.sh" qemu "$1" --run-dir "$run"; }
intent_is() { cmp -s "$run/boot-intent" "$1" || fail "boot intent differs from $1"; }
record_is() {
    python3 - "$run/filesystem-persistent-root.json" "$@" <<'PYRESULT'
import json, sys
from pathlib import Path
result = json.loads(Path(sys.argv[1]).read_text())
assert result['milestone'] == 'filesystem/persistent-root'
assert [row['id'] for row in result['cases']] == [
    'persistent-root/' + case for case in ('migrate-manifests-equal', 'host-manifests-equal',
                                          'cutover-verified', 'rescue-and-return')]
assert [row['result'] for row in result['cases']] == sys.argv[2:], result
PYRESULT
}
cat > "$fixture/migrate.intent" <<'INTENT'
BREENIX-BOOT 3
target persistent-migrate
mount 11111111-1111-1111-1111-111111111111 / ro
mount 22222222-2222-2222-2222-222222222222 /migrate/ext2-system ro
mount 33333333-3333-3333-3333-333333333333 /migrate/ext2-home ro
mount 44444444-4444-4444-4444-444444444444 /migrate/system rw
mount 55555555-5555-5555-5555-555555555555 /migrate/home rw
userland gen-1
INTENT
cat > "$fixture/cutover.intent" <<'INTENT'
BREENIX-BOOT 3
target persistent
mount 44444444-4444-4444-4444-444444444444 / rw
mount 55555555-5555-5555-5555-555555555555 /home rw
userland gen-1
INTENT
cat > "$fixture/rescue.intent" <<'INTENT'
BREENIX-BOOT 3
target persistent
mount 22222222-2222-2222-2222-222222222222 / ro
mount 33333333-3333-3333-3333-333333333333 /home ro
userland gen-1
INTENT
cycle migrate
intent_is "$fixture/migrate.intent"
CARGO_BUILD_JOBS=4 "$root/scripts/build-background.sh" rustc --edition=2021 -Dwarnings \
    "$root/tests/persistent_migrate_fixture.rs" -o "$fixture/migrate-guest"
python3 - "$root" "$fixture" <<'PYMIGRATE'
import sys
from pathlib import Path
sys.path.insert(0, str(Path(sys.argv[1]) / 'tests'))
from persistent_migrate_test import PersistentMigration
for tree, users_layout in [('migration-ok', True), ('migration-failed', False)]:
    PersistentMigration().fixture(Path(sys.argv[2]) / tree, users_layout=users_layout)
PYMIGRATE
if STUB_SCENARIO=migrate MIGRATE_TREE=migration-failed boot_fixture --mode console > "$fixture/migrate-failure.log" 2>&1; then
    fail 'failed migration returned success'
fi
[ -f "$fixture/migration-failed/poweroff-requested" ] || fail 'failed migration did not power off'
grep -q 'migrate boot did not supply its verification receipts' "$fixture/migrate-failure.log" || fail 'failed migration did not reach stock finish'
grep -Fxq '[migrate] exit code: 1' "$run/boots/$(cat "$run/current")/serial.txt" || fail 'missing failed migration receipt'
record_is FAIL FAIL FAIL FAIL
STUB_SCENARIO=migrate MIGRATE_TREE=migration-ok boot_fixture --mode console
[ -f "$fixture/migration-ok/poweroff-requested" ] || fail 'successful migration did not power off'
cp "$fixture/home.tsv" "$fixture/home-good.tsv"
sed '1s/0755/0700/' "$fixture/home-good.tsv" > "$fixture/home.tsv"
if cycle check-manifests; then fail 'manifest mismatch passed'; fi
record_is PASS FAIL FAIL FAIL
mv "$fixture/home-good.tsv" "$fixture/home.tsv"
printf x >> "$run/system.img"
if cycle check-manifests; then fail 'changed ext2 digest passed'; fi
record_is PASS FAIL FAIL FAIL
truncate -s 8M "$run/system.img"
cycle check-manifests
record_is PASS PASS FAIL FAIL
cycle cutover
intent_is "$fixture/cutover.intent"
if cycle promote; then fail 'unverified cutover promoted'; fi
STUB_SCENARIO=cutover boot_fixture --mode console
cycle promote
cycle rescue
intent_is "$fixture/rescue.intent"
STUB_SCENARIO=rescue boot_fixture --mode console
cycle return
intent_is "$fixture/cutover.intent"
STUB_SCENARIO=return boot_fixture --mode console
record_is PASS PASS PASS PASS
echo 'vm-persistent cold manifests and migration cycle checks passed'
