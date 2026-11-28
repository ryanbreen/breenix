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
import shutil
import subprocess
import sys
import time
import tarfile
import tempfile

GUEST = '/root/.breenix-flash'
TOKEN = re.compile(r'[0-9a-f]{32}')
COMMIT = re.compile(r'[0-9a-f]{40}')
STAGE = '/run/breenix/paired/stage'
PENDING = '/var/lib/accounts/pending'
DIGEST = re.compile(r'[0-9a-f]{64}')
BOOT_ID = re.compile(r'[0-9a-f]+-[0-9a-f]+-[0-9a-f]+')
BUILD_ID = re.compile(r'[0-9]{8}T[0-9]{6}Z-metal')
ROOT_NAME = re.compile(r'gen-[A-Za-z0-9-]{1,64}')
HEALTH = 'test "$(sudo -n /bin/readlink /proc/1/exe)" = /sbin/init && cat /run/breenix/boot-id'
DEADLINE = 600
MAX_IMAGE = 512 * 1024 * 1024
MANIFEST_MAX = {1: 255, 2: 512}


def retained_roots(slots, attempts, booted):
    """Keep the physical slot roots even when an acknowledged trial is running."""
    import metal_persist
    if not ROOT_NAME.fullmatch(booted):
        raise ValueError('the running root does not identify a generation')
    return {booted} | {
        'gen-' + generation for slot in [slots.get('current'), slots.get('previous'), *attempts]
        if (generation := metal_persist.slot_userland(slot))
    }


def manifest(token, entry, version):
    size = entry['size']
    if not TOKEN.fullmatch(token) or not 0 < size <= MAX_IMAGE:
        raise ValueError('invalid live flash manifest token or image size')
    if not COMMIT.fullmatch(entry['commit']) or any(
            not re.fullmatch(r'[0-9a-f]{64}', entry[field]) for field in ('sha256', 'baseNoteSha256')):
        raise ValueError('invalid live flash commit or digest')
    lines = [f'BREENIX-FLASH {version}', token, entry['commit'], str(size),
             entry['sha256'], entry['baseNoteSha256']]
    if version == 2:
        if not BUILD_ID.fullmatch(entry['userland']):
            raise ValueError('invalid paired generation')
        lines.append('gen-' + entry['userland'])
    text = ('\n'.join(lines) + '\n').encode('ascii')
    if version not in MANIFEST_MAX or len(text) > MANIFEST_MAX[version]:
        raise ValueError('manifest does not fit the installed manager')
    return text


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
    previous = metal_persist.slot_userland(current)
    entry.update(baseCommit=current['commit'], baseInstall=current['installedBy'],
                 previousRoot='gen-' + previous if previous else None, status='trial')
    m.write_json(record_path(m, token), entry)


def transition(m, token, status):
    path = record_path(m, token)
    entry = m.read_json(path)
    if not entry or not entry.get('paired'):
        raise ValueError('unknown paired flash attempt')
    if entry.get('status') in ('rolled-back', 'promoted') and status not in ('promoted', 'rolled-back'):
        return
    entry['status'] = status
    if status == 'arming':
        entry['armedAt'] = m.now()
    m.write_json(path, entry)



def request_reset(m, lease_id, token):
    if not BUILD_ID.fullmatch(lease_id):
        raise ValueError('invalid persistent lease')
    with m.locked('lease'):
        lease = m.read_lease() or {}
        entry = m.read_json(record_path(m, token)) or {}
        if (lease.get('id'), lease.get('state'), lease.get('kind')) != (lease_id, 'powered', 'persistent'):
            raise ValueError('paired reset requires the active powered persistent lease')
        if entry.get('lease') == lease_id and entry.get('status') == 'rolled-back':
            return
        if (entry.get('lease') != lease_id or not entry.get('paired')
                or entry.get('status') not in ('arming', 'trial')):
            raise ValueError('paired reset does not name an unacknowledged attempt on this lease')
        m.write_json(m.boot_dir(lease_id) / 'paired-reset.json', {'token': token, 'requestedAt': m.now()})


def pending_pair(m, lease_id, token=None):
    """An active host deploy owns the initial SSH deadline for its trial."""
    active = []
    paths = [record_path(m, token)] if token else (m.STATE / 'live-flash').glob('*.json')
    for path in paths:
        entry = m.read_json(path) or {}
        if (entry.get('lease') == lease_id and entry.get('paired')
                and entry.get('status') in ('arming', 'trial')
                and 0 <= m.now() - entry.get('armedAt', 0) <= 2 * DEADLINE + 180):
            active.append(entry)
    return max(active, key=lambda entry: entry['armedAt'], default=None)



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
        if entry.get('paired'):
            transition(m, token, 'promoted')


def unresolved_attempts(m, slots):
    resolved = {slot.get('liveFlash') for slot in slots.values() if isinstance(slot, dict)}
    attempts = []
    for path in (m.STATE / 'live-flash').glob('*.json'):
        if not TOKEN.fullmatch(path.stem) or path.stem in resolved:
            continue
        entry = m.read_json(path)
        if entry and entry.get('paired') and entry.get('status') in ('arming', 'trial', 'acknowledged'):
            attempts.append(entry)
    return attempts


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
    attempts = unresolved_attempts(m, slots)
    current_root = metal_persist.slot_userland(slots['current'])
    retained = sorted(retained_roots(slots, attempts, 'gen-' + current_root)) if current_root else []
    return dict(lease=lease['id'], current=slots['current'], retainedRoots=retained,
                partition=metal_persist.persistent_partition_guids(layout)[0],
                manifestVersion=disk['liveManager'].get('manifestVersion', 1),
                **{key: fields.get(key) for key in keys})


def prepare(m, inputs_id, token, commit, lease_id, broken, acknowledged, paired=False):
    import metal_image
    import metal_persist
    context = site(m)
    if context['lease'] != lease_id:
        raise ValueError('the persistent lease changed while building')
    if not BUILD_ID.fullmatch(inputs_id) or not COMMIT.fullmatch(commit):
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
    previous = metal_persist.slot_userland(current)
    if not previous:
        raise ValueError('CURRENT has no userland generation')
    generation = inputs_id if paired else previous
    layout = metal_persist.layout()
    slot = stage / 'trial.img'
    files = metal_image.extract_esp(inputs / 'uefi.img', stage / 'esp')
    if broken:
        loader = stage / 'broken-loader.efi'
        loader.write_bytes(b'deliberately invalid verification bootloader\n')
        files['efi/boot/bootx64.efi'] = loader
    metal_persist.slot_image(slot, files, layout, stage, generation, writable_root=True, compact=True)
    if paired:
        # BreenFS v1 admits only BASE. Require source evidence for this kernel
        # format before the running system is frozen.
        if build.get('breenfsIncompat') != 1:
            raise ValueError('paired candidate does not declare the supported BreenFS incompatibility mask')
        prepare_inventory(stage, inputs)
    record = dict(metal_persist.file_facts(slot), commit=commit, installedBy=token, liveFlash=token,
                  paired=paired, status='built', lease=lease_id,
                  userland=generation, writableRoot=True, previousRoot='gen-' + previous, baseCommit=current['commit'],
                  baseInstall=current['installedBy'])
    metal_persist.store_definition(inputs / 'definition', commit)
    m.write_json(state, record)
    base = metal_persist.slot_note(current['commit'], current['installedBy']).encode()
    return dict(record, image=str(slot), baseNoteSha256=hashlib.sha256(base).hexdigest())


@contextlib.contextmanager
def guest_access(context):
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

        config = Path(scratch) / 'ssh_config'
        options = [ssh[index + 1] for index, item in enumerate(ssh) if item == '-o']
        if not re.fullmatch(r'[A-Za-z0-9.:%_-]+', host):
            raise ValueError('invalid operator SSH hostname')
        quote = lambda path: json.dumps(str(path), ensure_ascii=False)
        options[options.index('UserKnownHostsFile=' + str(known))] = 'UserKnownHostsFile ' + quote(known)
        config.write_text('Host paired-guest\n  HostName ' + host + '\n  Port '
                          + str(int(context['SERVER_SSH_PORT'] or 22)) + '\n  IdentityFile '
                          + quote(identity) + '\n  ' + '\n  '.join(options) + '\n')
        run.upload = lambda local, remote: subprocess.run(
            [str(Path(scratch) / 'bsync'), '--sudo', '--ssh-config', str(config),
             str(local), f'{user}@paired-guest:{remote}'], check=True, timeout=DEADLINE)
        def install_uploader(data):
            binary = Path(scratch) / 'bsync'
            binary.write_bytes(data)
            binary.chmod(0o700)
        run.install_uploader = install_uploader
        yield run


def publish(guest, name, data, length=None, digest=None, guard='', mode='0600'):
    path = name if name.startswith(GUEST + '/') else GUEST + '/' + name
    options = {'input': data} if isinstance(data, bytes) else {'stdin': data}
    if isinstance(data, bytes):
        length, digest = len(data), hashlib.sha256(data).hexdigest()
    command = f'sudo -n /bin/bput {shlex.quote(path)} {length} {digest} {mode}'
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
            rolled_back = False
        elif found := re.search(rf'breenix-manager: live flash: trial {token} ([0-9a-f]{{40}})$', line):
            selected = ('trial', found[1])
        elif line.endswith(f'breenix-manager: live flash: rollback {token}'):
            rolled_back = True
        elif found := re.search(r'breenix-manager: booting current ([0-9a-f]{40})$', line):
            selected = ('rollback', found[1]) if rolled_back else None
        elif selected and (found := re.search(r'\[init\] log boot-id=([0-9a-f-]+)$', line)):
            if found[1] != previous_boot and BOOT_ID.fullmatch(found[1]):
                selected = (*selected[:2], found[1])
    return selected if selected and len(selected) == 3 else None


def force_rollback(token, previous_boot, trial_boot, previous_commit, previous_root,
                   read_serial, guest_facts, reboot, reset):
    """Wait for the manager's selection and an independent SSH root receipt."""
    serial = None
    if trial_boot:
        start = len(read_serial())
        if reboot(trial_boot) is not False:
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                output = read_serial()
                if 'breenix-manager: started:' in output[start:]:
                    serial = output
                    break
                time.sleep(2)
            else:
                reset(token)
        else:
            reset(token)
    else:
        reset(token)

    deadline = time.monotonic() + DEADLINE
    while time.monotonic() < deadline:
        observed = monitor(serial if serial is not None else read_serial(), token, previous_boot)
        serial = None
        if observed and observed[:2] == ('rollback', previous_commit):
            boot_id = observed[2]
            if guest_facts() == (boot_id, previous_root, 'rw'):
                return boot_id
        time.sleep(2)
    raise ValueError('rollback did not return the previous kernel and root; use the documented network recovery route')


def prepare_inventory(stage, inputs):
    from metal_root import require_e2fsprogs
    from migrate_manifest import Ext2
    debugfs, = require_e2fsprogs('debugfs')
    image = Ext2(debugfs, str(inputs / 'ext2.img'))
    rows = list(image.payload_rows())
    if not rows:
        raise ValueError('the build did not deliver a userspace payload')
    publisher = inputs / 'inventory.tsv'
    if publisher.is_file():
        # Use the publisher's rows and look up their inodes in the delivered
        # image. Object extraction checks the resulting bytes.
        by_path = {row.split('\t', 1)[0]: (row, inode) for row, inode in rows}
        expected = publisher.read_text().splitlines(keepends=True)
        if not expected or any(line.split('\t', 1)[0] not in by_path for line in expected):
            raise ValueError('publisher inventory names paths missing from the built image')
        rows = [(line, by_path[line.split('\t', 1)[0]][1]) for line in expected]
    (stage / 'inventory.tsv').write_text(''.join(row for row, _ in rows))
    objects = {}
    for row, inode in rows:
        path, kind, _, _, _, _, size, _, digest = row.strip().split('\t')
        if kind in ('reg', 'symlink'):
            objects[path] = [kind, int(size), digest, inode]
    (stage / 'objects.json').write_text(json.dumps(objects))


def export_objects(stage, skipped, output):
    from metal_root import require_e2fsprogs
    from migrate_manifest import Ext2, check_debugfs
    index = json.loads((stage / 'objects.json').read_text())
    objects = {}
    for row, (kind, size, sha, inode) in index.items():
        if row not in skipped:
            objects[sha] = (kind, size, inode)
    total = sum(size for _, size, _ in objects.values())
    if total > 1 << 30:
        raise ValueError(f'object set is {total} bytes; stop and ask the coordinator')
    print(f'[metal-run] staging {len(objects)} objects, {total} bytes', file=sys.stderr, flush=True)
    debugfs, = require_e2fsprogs('debugfs')
    image = Ext2(debugfs, str(stage / 'inputs/ext2.img'))
    with tarfile.open(fileobj=output, mode='w|') as archive:
        for sha, (kind, size, inode) in sorted(objects.items()):
            if not DIGEST.fullmatch(sha) or type(inode) is not int:
                raise ValueError('invalid inventory object index')
            with tempfile.TemporaryFile() as content:
                if kind == 'symlink':
                    command = f'stat <{inode}>'
                    _, _, fast = image.read(command).partition(b'\nFast link dest: "')
                    if fast[size:] == b'"\n':
                        content.write(fast[:size])
                    else:
                        dump_object(image, inode, content, check_debugfs)
                else:
                    dump_object(image, inode, content, check_debugfs)
                content.seek(0)
                scan = hashlib.sha256()
                while chunk := content.read(1 << 20):
                    scan.update(chunk)
                if content.tell() != size or scan.hexdigest() != sha:
                    raise ValueError(f'object readback differs for {sha}')
                content.seek(0)
                entry = tarfile.TarInfo(sha)
                entry.size, entry.mode = size, 0o600
                archive.addfile(entry, content)


def dump_object(image, inode, output, check_debugfs):
    command = f'cat <{inode}>'
    result = subprocess.run(image.argv(command), stdout=output, stderr=subprocess.PIPE, env=image.env)
    check_debugfs(command, result.returncode, result.stderr)


def paired_reboot_guard(boot_id):
    if not BOOT_ID.fullmatch(boot_id):
        raise ValueError('invalid boot ID in paired reboot guard')
    # -L rejects a symlink even when its target would appear empty.
    return (f'test "$(cat /run/breenix/boot-id)" = {boot_id} && '
            f'sudo -n /bin/sh -c \'test ! -L {PENDING} && '
            f'{{ test ! -e {PENDING} || {{ test -d {PENDING} && '
            f'pending=$(ls -A {PENDING}) && test -z "$pending"; }}; }}\' && ')


def verify_line(output, id, digest):
    return re.fullmatch(rf'paired verify {re.escape(id)} rows=[1-9][0-9]* sha256={digest}', output) is not None


def stage_pair(id, tool_path, tool, inventory, roots, put, paired, upload, inventory_sha):
    put(tool_path, tool, '0700')
    put(f'{GUEST}/{id}/inventory.tsv', inventory, '0600')
    if paired('protocol') != 'BREENIX-PAIRED 1':
        raise ValueError('staged installer did not identify the paired protocol')
    paired('reclaim', *roots)
    paired('begin', id, *roots)
    upload(paired('have', id))
    result = json.loads(paired('apply', id).split('\n')[-1])
    if not isinstance(result, dict) or set(result) != {'carried', 'deletedOnBox', 'onBox'}:
        raise ValueError('invalid paired apply receipt')
    if not verify_line(paired('verify', id), id, inventory_sha):
        raise ValueError('staged root readback did not match the published inventory')
    paired('publish', id)
    return result


def confirmed_pair(facts, boot_id, id, inventory_sha, identity, paired):
    return (facts == (boot_id, 'gen-' + id, 'rw') and identity == id
            and verify_line(paired('verify', id, '--booted'), id, inventory_sha))


def guest_facts(guest):
    result = guest(HEALTH + ' && cat /proc/mounts')
    if result.returncode:
        return None
    lines = result.stdout.decode(errors='replace').splitlines()
    if not lines or not BOOT_ID.fullmatch(lines[0]):
        return None
    for line in lines[1:]:
        parts = line.split()
        if len(parts) >= 4 and parts[1:3] == ['/', 'breenfs'] and ROOT_NAME.fullmatch(parts[0]):
            return lines[0], parts[0], parts[3].split(',')[0]
    return None


def invoke_paired(guest, tool_path, guard, *args):
    command = f'sudo -n {shlex.quote(tool_path)} paired ' + shlex.join(args)
    result = guest(guard + command, timeout=DEADLINE)
    if result.returncode:
        raise ValueError(result.stderr.decode(errors='replace').strip() or f'paired {args[0]} failed')
    return result.stdout.decode().strip()


def stage_objects(operation, guest, id, have):
    if any(not re.fullmatch(r'(?:[0-9a-f]{2})+', row) for row in have.splitlines()):
        raise ValueError('invalid paired have receipt')
    with tempfile.TemporaryDirectory(prefix='paired-objects-') as temporary, tempfile.TemporaryFile() as packed:
        operation('objects', id, input=have.encode(), stdout=packed, check=True)
        packed.seek(0)
        root = Path(temporary)
        with tarfile.open(fileobj=packed, mode='r|') as archive:
            for member in archive:
                if not member.isfile() or not DIGEST.fullmatch(member.name):
                    raise ValueError('invalid streamed object name or type')
                path = root / member.name
                with path.open('xb') as output, archive.extractfile(member) as source:
                    shutil.copyfileobj(source, output)
                path.chmod(0o600)
        guest.upload(root, f'{STAGE}/.breenix-incoming/objects')

def run_paired(runner, args):
    start = time.monotonic()
    env = runner['host_env']()
    state, tools = runner['STATE'], runner['TOOLS']

    def service(*words, **kwargs):
        return runner['in_metal'](env, *words, **kwargs)

    def operation(*words, **kwargs):
        return service('python3', f'{tools}/metal_live_flash.py', *words, **kwargs)

    def query(*words):
        return json.loads(operation(*words, capture_output=True, check=True).stdout)

    state_dir = Path.home() / '.local/state/breenix-metal'
    state_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    stages = []
    try:
        with (state_dir / 'live-flash.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            context = query('site')
            lease = context['lease']
            with guest_access(context) as guest:
                old = guest_facts(guest)
                if not old or old[2] != 'rw':
                    raise ValueError('persistent SSH did not identify a healthy writable generation')
                boot_id, booted, _ = old
                branch = args.branch or 'main'
                commit = args.sha or runner['resolve'](env, branch)
                if args.sha:
                    runner['verify_target'](env, commit, branch)
                # The installed tools pin their own sources independently of the
                # production kernel and payload being deployed.
                tools_commit = runner['paired_tools_commit']()
                id = time.strftime('%Y%m%dT%H%M%SZ', time.gmtime()) + '-metal'
                tool_id = id
                if tools_commit != commit:
                    tool_id = time.strftime('%Y%m%dT%H%M%SZ', time.gmtime(time.time() + 1)) + '-metal'
                for build_id, sha in dict([(id, commit), (tool_id, tools_commit)]).items():
                    stages.append(build_id)
                    if runner['build'](build_id, sha, live=True, paired_tools=build_id == tool_id):
                        raise ValueError(f'paired input build failed for {sha}')
                token = secrets.token_hex(16)
                receipt = guest(f'sudo -n /bin/cat {GUEST}/committed')
                ack = receipt.stdout.decode(errors='replace').strip()
                if receipt.returncode or not TOKEN.fullmatch(ack):
                    ack = ''
                entry = query('prepare', id, token, commit, lease, '0', ack, '--paired')
                context = query('site')
                if context['lease'] != lease or guest_facts(guest) != old:
                    raise ValueError('the persistent session changed before paired staging')
                if not context['retainedRoots']:
                    raise ValueError('physical CURRENT does not identify a retained generation')
                roots = sorted(set(context['retainedRoots']) | {booted})
                inventory = service('cat', f'{state}/stage/{id}/inventory.tsv', capture_output=True, check=True).stdout
                inventory_sha = hashlib.sha256(inventory).hexdigest()
                uploader = service('cat', f'{state}/stage/{tool_id}/inputs/paired-bsync',
                                   capture_output=True, check=True).stdout
                guest.install_uploader(uploader)
                tool = operation('installer', tool_id, capture_output=True, check=True).stdout
                tool_sha = hashlib.sha256(tool).hexdigest()
                tool_path = f'{GUEST}/tools/{tool_sha}/persistent-install'
                guard = f'test "$(cat /run/breenix/boot-id)" = {boot_id} && '
                directory = f'{state}/boots/{lease}'
                offset = int(service('stat', '-c', '%s', directory + '/serial.txt', capture_output=True, check=True).stdout)
                paired = lambda *words: invoke_paired(guest, tool_path, guard, *words)
                if guest(guard + f'sudo -n mkdir -p -m 700 {GUEST} {GUEST}/tools {GUEST}/tools/{tool_sha} {GUEST}/{id}').returncode:
                    raise ValueError('could not prepare paired deploy directories')
                reboot_requested = False
                arming = False

                def put(path, data, mode):
                    publish(guest, path, data, guard=guard, mode=mode)

                try:
                    changes = stage_pair(id, tool_path, tool, inventory, roots, put, paired,
                        lambda have: stage_objects(operation, guest, id, have), inventory_sha)
                    version = context['manifestVersion']
                    body = manifest(token, entry, version)
                    with tempfile_image(service, entry['image']) as image:
                        publish(guest, 'kernel.img', image, entry['size'], entry['sha256'], guard)
                    operation('state', token, 'arming', check=True)
                    arming = True
                    publish(guest, 'manifest', body, guard=guard)
                    print(f'[metal-run] armed pair {token} for {commit} and gen-{id}', flush=True)
                    from_reboot = time.monotonic()
                    rc = guest(paired_reboot_guard(boot_id) + 'sudo -n /sbin/reboot', timeout=70).returncode
                    if rc not in (0, 124, 255):
                        raise ValueError('the boot ID or pending account keys refused the paired reboot')
                    reboot_requested = True
                except BaseException:
                    if not reboot_requested:
                        try:
                            if arming:
                                # If the publish was interrupted its rename may
                                # have won. Disarm this token before thawing.
                                command = (guard + f'sudo -n /bin/sh -c \'if test -f {GUEST}/manifest && '
                                    f'test "$(sed -n 2p {GUEST}/manifest)" = {token}; then '
                                    f'rm {GUEST}/manifest && sync; fi\'')
                                if guest(command).returncode:
                                    raise ValueError('could not disarm the paired attempt')
                                operation('state', token, 'cancelled', check=True)
                            paired('abort', id)
                        except (ValueError, OSError, subprocess.CalledProcessError) as error:
                            print(f'[metal-run] paired cleanup: {error}', file=sys.stderr)
                    raise

                def read_serial():
                    if service('test', '-e', directory + '/ended').returncode == 0:
                        raise ValueError('paired attempt lost the persistent lease; use the documented network recovery route')
                    return service('tail', '-c', '+' + str(offset + 1), directory + '/serial.txt',
                                   capture_output=True, text=True, errors='replace', check=True).stdout

                def reset(attempt):
                    service('python3', f'{tools}/metal-boot', 'paired-reset', lease, attempt, check=True)

                def reboot(trial_boot):
                    command = f'test "$(cat /run/breenix/boot-id)" = {trial_boot} && sudo -n /sbin/reboot'
                    return guest(command, timeout=70).returncode in (0, 124, 255)

                def verify_trial(*words):
                    if time.monotonic() >= deadline:
                        raise ValueError('the paired trial reached its confirmation deadline')
                    check = guest(f'sudo -n /bin/sha256sum {tool_path}')
                    if check.returncode or check.stdout.decode().split() != [tool_sha, tool_path]:
                        raise ValueError('the shared installer changed across the reboot')
                    return invoke_paired(guest, tool_path,
                        f'test "$(cat /run/breenix/boot-id)" = {trial_boot} && ', *words)

                outcome = None
                trial_boot = None
                marker = None
                deadline = from_reboot + DEADLINE
                injected = args.inject == 'paired-no-commit'
                while time.monotonic() < deadline:
                    serial = read_serial()
                    observed = monitor(serial, token, boot_id)
                    facts = guest_facts(guest)
                    if observed and facts and facts[0] == observed[2]:
                        phase, fallback, selected_boot = observed
                        recorded = query('record', token)
                        entry.update(baseCommit=recorded['baseCommit'], previousRoot=recorded['previousRoot'])
                        if phase == 'rollback' and fallback == entry['baseCommit'] and facts[1:] == (entry['previousRoot'], 'rw'):
                            outcome = 'rolled-back'
                            break
                        if phase == 'trial':
                            if fallback != commit:
                                raise ValueError('the manager trial commit disagrees with the staged kernel')
                            trial_boot = selected_boot
                            identity = guest('cat /usr/share/factory/generation')
                            identity = identity.stdout.decode().strip() if identity.returncode == 0 else None
                            try:
                                verified = confirmed_pair(facts, trial_boot, id, inventory_sha, identity, verify_trial)
                                if injected and verified:
                                    marker = f'/usr/share/factory/.paired-no-commit-{token}'
                                    data = (token + '\n').encode()
                                    cmd = (f'test "$(cat /run/breenix/boot-id)" = {trial_boot} && '
                                           f'sudo -n /bin/bput {marker} {len(data)} {hashlib.sha256(data).hexdigest()} 600')
                                    if guest(cmd, input=data).returncode:
                                        marker = None
                                        raise ValueError('could not write the trial rollback marker')
                                elif verified and time.monotonic() < deadline:
                                    publish(guest, 'committed', (token + '\n').encode(),
                                            guard=f'test "$(cat /run/breenix/boot-id)" = {trial_boot} && ')
                                    outcome = 'committed'
                                    break
                            except (ValueError, subprocess.CalledProcessError) as error:
                                print(f'[metal-run] trial confirmation failed: {error}', file=sys.stderr)
                            break
                    # Refusal leaves CURRENT running without an attempted trial.
                    # It is distinct from a manager-confirmed rollback.
                    if f'live flash: stale base for {token}' in serial or 'live flash refused' in serial:
                        raise ValueError('the boot manager refused to try the staged pair')
                    time.sleep(2)
                if outcome is None:
                    recorded = query('record', token)
                    entry.update(baseCommit=recorded['baseCommit'], previousRoot=recorded['previousRoot'])
                    # If the committed bput lost its reply, do not reset a
                    # trial whose acknowledgement may already be durable.
                    if trial_boot:
                        check = guest(f'sudo -n /bin/cat {GUEST}/committed')
                        if check.returncode == 0 and check.stdout.strip() == token.encode():
                            if guest_facts(guest) != (trial_boot, 'gen-' + id, 'rw'):
                                raise ValueError('the acknowledged paired trial lost its SSH identity')
                            selected_boot, outcome = trial_boot, 'committed'
                    if outcome is None:
                        selected_boot = force_rollback(token, boot_id, trial_boot, entry['baseCommit'],
                            entry['previousRoot'], read_serial, lambda: guest_facts(guest), reboot, reset)
                        outcome = 'rolled-back'
                if marker and outcome == 'rolled-back' and guest(f'test ! -e {marker}').returncode:
                    raise ValueError('the failed trial marker is visible after rollback')
                operation('state', token, 'acknowledged' if outcome == 'committed' else outcome, check=True)
                at_ssh = time.monotonic()
                receipt = dict(result=outcome, token=token, commit=commit, previous=entry['baseCommit'],
                               bootId=selected_boot, lease=lease, pid1='/sbin/init', root='gen-' + id,
                               previousRoot=entry['previousRoot'], inventorySha256=inventory_sha,
                               manifestVersion=version, manifestBytes=len(body), **changes,
                               commandToSsh=round(at_ssh - start, 3), rebootToSsh=round(at_ssh - from_reboot, 3))
                service('tee', directory + '/live-flash.json', input=json.dumps(receipt) + '\n',
                        text=True, capture_output=True, check=True)
                print(json.dumps(receipt), flush=True)
                return int((outcome == 'rolled-back') != injected or injected and not marker)
    except (ValueError, OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        print(f'metal-run: {error}', file=sys.stderr)
        return 1
    finally:
        for id in stages:
            service('rm', '-rf', f'{state}/stage/{id}')

def run(runner, args):
    if getattr(args, 'paired', False):
        return run_paired(runner, args)
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
                manifest_body = manifest(token, entry, 1)
                capture = service('stat', '-c', '%s', directory + '/serial.txt', capture_output=True, check=True)
                offset = int(capture.stdout)
                publish(guest, 'manifest', manifest_body, guard=guard)
                print(f'[metal-run] armed {token} for {commit} on {lease}', flush=True)
                from_reboot = time.monotonic()
                reboot = guest(guard + 'sudo -n /sbin/reboot', timeout=70)
                if reboot.returncode not in (0, 124, 255):
                    raise ValueError('guest refused the requested reboot')
                while time.monotonic() - from_reboot < DEADLINE:
                    if service('test', '-e', directory + '/ended').returncode == 0:
                        raise ValueError('the persistent lease ended before SSH confirmation')
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
                            if phase == 'trial' and fallback_commit == commit and not broken:
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


def main(m, args):
    command, *words = args
    if command == 'site' and not words:
        print(json.dumps(site(m)))
    elif command == 'prepare':
        build_id, token, commit, lease, broken, acknowledged, *options = words
        if broken not in ('0', '1') or options not in ([], ['--paired']):
            raise ValueError('invalid live flash prepare options')
        print(json.dumps(prepare(m, build_id, token, commit, lease, broken == '1', acknowledged, bool(options))))
    elif command in ('objects', 'installer') and len(words) == 1 and BUILD_ID.fullmatch(words[0]):
        stage = m.STATE / 'stage' / words[0]
        if command == 'objects':
            export_objects(stage, set(sys.stdin.read().splitlines()), sys.stdout.buffer)
        else:
            from metal_root import require_e2fsprogs
            from migrate_manifest import Ext2, check_debugfs
            debugfs, = require_e2fsprogs('debugfs')
            image = Ext2(debugfs, str(stage / 'inputs/ext2.img'))
            inode = image.children(image.children(2)[b'sbin'])[b'persistent-install']
            dump_object(image, inode, sys.stdout.buffer, check_debugfs)
    elif command == 'state' and len(words) == 2 and words[1] in ('arming', 'cancelled', 'acknowledged', 'rolled-back'):
        with m.locked('live-flash-record'):
            transition(m, *words)
    elif command == 'record' and len(words) == 1:
        print(json.dumps(m.read_json(record_path(m, words[0]))))
    else:
        raise ValueError('invalid live flash operation')


if __name__ == '__main__':
    import metal_lib as m
    try:
        main(m, sys.argv[1:])
    except (ValueError, OSError, KeyError) as error:
        sys.exit(str(error))
