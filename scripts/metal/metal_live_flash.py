"""Live kernel flash on a powered persistent lease. No guest raw disk access."""
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import sys
import time

GUEST = '/root/.breenix-flash'
TOKEN = re.compile(r'[0-9a-f]{32}')
COMMIT = re.compile(r'[0-9a-f]{40}')
BOOT_ID = re.compile(r'[0-9a-f]+-[0-9a-f]+-[0-9a-f]+')
HEALTH = 'test "$(readlink /proc/1/exe)" = /sbin/init && cat /run/breenix/boot-id'
DEADLINE = 600


def record_path(m, token):
    if not TOKEN.fullmatch(token):
        raise ValueError('invalid flash token')
    return m.STATE / 'live-flash' / (token + '.json')


def candidate(m, token, commit):
    entry = m.read_json(record_path(m, token))
    if not entry or entry.get('commit') != commit:
        raise ValueError('live candidate has no matching build record')
    return entry


def trial(m, token, commit):
    import metal_persist
    entry = candidate(m, token, commit)
    current = metal_persist.read_disk()['slots']['current']
    entry.update(baseCommit=current['commit'], baseInstall=current['installedBy'])
    m.write_json(record_path(m, token), entry)


def promoted(m, token, commit):
    import metal_persist
    entry = candidate(m, token, commit)
    with m.locked('disk'):
        disk = metal_persist.read_disk()
        slots = disk['slots']
        current = slots['current']
        if current.get('liveFlash') == token:
            return
        if (current.get('commit') != entry['baseCommit']
                or current.get('installedBy') != entry['baseInstall']):
            raise ValueError('live promotion disagrees with the recorded base slot')
        slots['previous'] = current
        slots['current'] = {key: entry[key] for key in
                            ('commit', 'installedBy', 'userland', 'writableRoot', 'liveFlash', 'size', 'sha256')}
        m.write_json(m.STATE / 'disk.json', disk)


def site(m):
    import metal_persist
    import metal_migration
    disk = metal_persist.read_disk()
    lease = m.read_lease() or {}
    layout, slots = disk.get('layout') or {}, disk.get('slots') or {}
    if (lease.get('kind') != 'persistent' or lease.get('state') != 'powered'
            or layout.get('version') != 3 or not slots.get('current', {}).get('writableRoot')
            or slots.get('next') or not disk.get('liveManager')):
        raise ValueError('live flash needs the updated manager, a powered persistent lease and writable format-3 CURRENT')
    metal_migration.require_closed(layout)
    fields = m.load_site()
    keys = ('SERVER_SSH_HOST', 'SERVER_SSH_PORT', 'OPERATOR_USER', 'OPERATOR_SSH_PUBLIC_KEY')
    return dict(lease=lease['id'], current=slots['current'],
                partition=metal_persist.persistent_partition_guids(layout)[0],
                **{key: fields.get(key) for key in keys})


def prepare(m, inputs_id, token, commit, lease_id, broken, acknowledged):
    import metal_image
    import metal_persist
    context = site(m)
    if context['lease'] != lease_id:
        raise ValueError('the persistent lease changed while building')
    if not re.fullmatch(r'\d{8}T\d{6}Z-metal', inputs_id) or not COMMIT.fullmatch(commit):
        raise ValueError('invalid flash build identity')
    state = record_path(m, token)
    state.parent.mkdir(parents=True, exist_ok=True)
    stage = m.STATE / 'stage' / inputs_id
    inputs = stage / 'inputs'
    build = m.read_json(inputs / 'build.json')
    if (not build or build.get('profile') != 'production' or build.get('commit') != commit
            or build.get('writablePersistentRoot') is not True):
        raise ValueError('live flash requires a production kernel with writable persistent root support')
    current = context['current']
    session = m.read_json(m.boot_dir(lease_id) / 'run.json') or {}
    if acknowledged and session.get('entry') == 'trial' and session.get('liveFlash') == acknowledged:
        current = candidate(m, acknowledged, session['bootedCommit'])
    generation = metal_persist.slot_userland(current)
    if not generation:
        raise ValueError('CURRENT has no userland generation')
    layout = metal_persist.layout()
    slot = stage / 'trial.img'
    files = metal_image.extract_esp(inputs / 'uefi.img', stage / 'esp')
    if broken:
        loader = stage / 'broken-loader.efi'
        loader.write_bytes(b'deliberately invalid verification bootloader\n')
        files['efi/boot/bootx64.efi'] = loader
    metal_persist.slot_image(slot, files, layout, stage, generation, writable_root=True, compact=True)
    record = dict(metal_persist.file_facts(slot), commit=commit, installedBy=token, liveFlash=token,
                  userland=generation, writableRoot=True, baseCommit=current['commit'],
                  baseInstall=current['installedBy'])
    metal_persist.store_definition(inputs / 'definition', commit)
    m.write_json(state, record)
    base = metal_persist.slot_note(current['commit'], current['installedBy']).encode()
    return dict(record, image=str(slot), baseNoteSha256=hashlib.sha256(base).hexdigest())


@contextlib.contextmanager
def guest_access(context):
    import tempfile
    user, host = context['OPERATOR_USER'], context['SERVER_SSH_HOST']
    if not user or not host or not context['OPERATOR_SSH_PUBLIC_KEY']:
        raise ValueError('live flash needs the operator SSH site fields')
    public = context['OPERATOR_SSH_PUBLIC_KEY'].split()[:2]
    identity = next((path.with_suffix('') for path in (Path.home() / '.ssh').glob('*.pub')
                     if path.read_text().split()[:2] == public and path.with_suffix('').is_file()), None)
    if identity is None:
        raise ValueError('no private key for the operator')
    key_id = hashlib.sha256((context['partition'] + ':' + host).encode()).hexdigest()
    state = Path.home() / '.local/state/breenix-metal'
    pinned = state / ('pinned-hostkey-' + key_id + '.json')
    if not pinned.is_file():
        raise ValueError('no trusted host-key pin for CURRENT; verify it with the documented recovery path')
    with tempfile.TemporaryDirectory(prefix='metal-live-') as scratch:
        known = Path(scratch) / 'known_hosts'
        known.write_text(json.loads(pinned.read_text())['known'])
        ssh = ['ssh', '-F', '/dev/null', '-p', str(context['SERVER_SSH_PORT'] or 22), '-o', 'ConnectTimeout=4',
               '-o', 'ConnectionAttempts=1', '-o', 'ServerAliveInterval=5', '-o', 'ServerAliveCountMax=3',
               '-o', 'StrictHostKeyChecking=yes', '-o', 'UserKnownHostsFile=' + str(known),
               '-o', 'GlobalKnownHostsFile=/dev/null', '-o', 'HostKeyAlgorithms=ssh-ed25519', '-o', 'IdentitiesOnly=yes',
               '-o', 'IdentityAgent=none', '-o', 'BatchMode=yes', '-i', str(identity), user + '@' + host]

        def run(script, timeout=12, **kwargs):
            try:
                return subprocess.run(ssh + ['sh -c ' + shlex.quote(script)], capture_output=True, timeout=timeout, **kwargs)
            except subprocess.TimeoutExpired:
                return subprocess.CompletedProcess([], 124, b'', b'SSH command timed out')

        yield run


def publish(guest, name, data, length=None, digest=None, guard=''):
    options = {'input': data} if isinstance(data, bytes) else {'stdin': data}
    if isinstance(data, bytes):
        length, digest = len(data), hashlib.sha256(data).hexdigest()
    command = f'sudo -n /bin/bput {GUEST}/{name} {length} {digest} 600'
    result = guest(guard + command, timeout=DEADLINE, **options)
    if result.returncode or result.stdout.strip() != b'published':
        raise ValueError(f'guest could not publish {name}: {result.stderr.decode(errors="replace").strip()}')


def monitor(serial, token, previous_boot):
    """Associate init's SSH-readable boot ID with the preceding manager selection."""
    selected = None
    rolled_back = False
    for line in serial.splitlines():
        if 'breenix-manager: started:' in line:
            selected = None
        elif f'breenix-manager: live flash: trial {token} ' in line:
            selected = ('trial', None)
        elif f'breenix-manager: live flash: rollback {token}' in line:
            rolled_back = True
        elif found := re.search(r'breenix-manager: booting current ([0-9a-f]{40})$', line):
            selected = ('rollback', found[1]) if rolled_back else None
        elif selected and (found := re.search(r'\[init\] log boot-id=([0-9a-f-]+)$', line)):
            if found[1] != previous_boot and BOOT_ID.fullmatch(found[1]):
                selected = (*selected[:2], found[1])
    return selected if selected and len(selected) == 3 else None


def run(runner, args):
    start = time.monotonic()
    env = runner['host_env']()
    tools, state = runner['TOOLS'], runner['STATE']

    def service(*words, **kwargs):
        return runner['in_metal'](env, *words, **kwargs)

    def operation(*words, **kwargs):
        return service('python3', f'{tools}/metal_live_flash.py', *words, **kwargs)

    state_dir = Path.home() / '.local/state/breenix-metal'
    state_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    inputs_id = None
    try:
        with (state_dir / 'live-flash.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            query = operation('site', capture_output=True, text=True)
            if query.returncode:
                raise ValueError(query.stderr.strip() or 'cannot inspect persistent lease')
            context = json.loads(query.stdout)
            lease = context['lease']
            with guest_access(context) as guest:
                old = guest(HEALTH)
                boot_id = old.stdout.decode(errors='replace').strip()
                if old.returncode or not BOOT_ID.fullmatch(boot_id):
                    raise ValueError('persistent SSH did not identify /sbin/init as PID 1; use --recovery')
                branch = args.branch or 'main'
                commit = args.sha or runner['resolve'](env, branch)
                if args.sha:
                    runner['verify_target'](env, commit, branch)
                token = secrets.token_hex(16)
                inputs_id = time.strftime('%Y%m%dT%H%M%SZ', time.gmtime()) + '-metal'
                if runner['build'](inputs_id, commit, kernel_only=True, live=True):
                    raise ValueError('live candidate build failed')
                broken = args.inject == 'upgrade-no-ssh'
                receipt = guest(f'sudo -n /bin/cat {GUEST}/committed')
                acknowledged = receipt.stdout.decode(errors='replace').strip()
                if receipt.returncode or not TOKEN.fullmatch(acknowledged):
                    acknowledged = ''
                result = operation('prepare', inputs_id, token, commit, lease, str(int(broken)), acknowledged,
                                   capture_output=True, text=True, check=True)
                entry = json.loads(result.stdout)
                directory = f'{state}/boots/{lease}'
                if (guest(HEALTH).stdout.strip() != old.stdout.strip()
                        or json.loads(operation('site', capture_output=True, check=True).stdout)['lease'] != lease):
                    raise ValueError('the persistent session changed before staging')
                guard = f'test "$(cat /run/breenix/boot-id)" = {boot_id} && '
                if guest(guard + f'sudo -n mkdir -p -m 700 {GUEST}').returncode:
                    raise ValueError('sudo -n could not create the staging directory')
                # Use a file descriptor for SSH stdin so a large image stays off the host Python heap.
                with tempfile_image(service, entry['image']) as image:
                    publish(guest, 'kernel.img', image, entry['size'], entry['sha256'], guard)
                manifest = (f"BREENIX-FLASH 1\n{token}\n{commit}\n{entry['size']}\n{entry['sha256']}\n"
                            f"{entry['baseNoteSha256']}\n").encode()
                capture = service('stat', '-c', '%s', directory + '/serial.txt', capture_output=True, check=True)
                offset = int(capture.stdout)
                publish(guest, 'manifest', manifest, guard=guard)
                print(f'[metal-run] armed {token} for {commit} on {lease}', flush=True)
                from_reboot = time.monotonic()
                reboot = guest(guard + 'sudo -n /sbin/reboot', timeout=70)
                if reboot.returncode not in (0, 124, 255):
                    raise ValueError('guest refused the requested reboot')
                while time.monotonic() - from_reboot < DEADLINE:
                    serial = service('tail', '-c', '+' + str(offset + 1), directory + '/serial.txt',
                                     capture_output=True, text=True, errors='replace', check=True).stdout
                    observed = monitor(serial, token, boot_id)
                    if observed:
                        phase, fallback_commit, new_boot = observed
                        health = guest(HEALTH)
                        if health.returncode == 0 and health.stdout.strip() == new_boot.encode():
                            at_ssh = time.monotonic()
                            recorded = json.loads(service('cat', f'{state}/live-flash/{token}.json',
                                                          capture_output=True, check=True).stdout)
                            entry['baseCommit'] = recorded['baseCommit']
                            if phase == 'trial' and not broken:
                                publish(guest, 'committed', (token + '\n').encode(),
                                        guard=f'test "$({HEALTH})" = {new_boot} && ')
                                outcome = 'committed'
                            elif phase == 'rollback' and fallback_commit == entry['baseCommit']:
                                outcome = 'rolled-back'
                            else:
                                raise ValueError('unexpected kernel after the flash')
                            receipt = dict(result=outcome, token=token, commit=commit, previous=entry['baseCommit'],
                                           bootId=new_boot, lease=lease, pid1='/sbin/init',
                                           commandToSsh=round(at_ssh - start, 3),
                                           rebootToSsh=round(at_ssh - from_reboot, 3))
                            service('tee', directory + '/live-flash.json', stdin=None,
                                    input=json.dumps(receipt) + '\n', text=True, capture_output=True, check=True)
                            print(json.dumps(receipt), flush=True)
                            return int((outcome == 'rolled-back') != broken)
                    time.sleep(2)
                raise ValueError('no matching SSH and PID 1 receipt after reboot; CURRENT will boot on the next power cycle')
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'metal-run: {error}', file=sys.stderr)
        return 1
    finally:
        if inputs_id:
            service('rm', '-rf', f'{state}/stage/{inputs_id}')


@contextlib.contextmanager
def tempfile_image(service, image):
    import tempfile
    with tempfile.TemporaryFile() as temporary:
        service('cat', image, stdout=temporary, check=True)
        temporary.seek(0)
        yield temporary


if __name__ == '__main__':
    import metal_lib as m
    try:
        if sys.argv[1:] == ['site']:
            print(json.dumps(site(m)))
        else:
            command, build_id, token, commit, lease, broken, acknowledged = sys.argv[1:]
            if command != 'prepare' or broken not in ('0', '1'):
                raise ValueError('invalid live flash operation')
            print(json.dumps(prepare(m, build_id, token, commit, lease, broken == '1', acknowledged)))
    except (ValueError, OSError) as error:
        sys.exit(str(error))
