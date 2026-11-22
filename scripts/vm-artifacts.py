#!/usr/bin/env python3
"""Seal source-built VM artifacts and verify the bytes selected for launch."""
import hashlib
import importlib.util
import json
import secrets
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent))

PERSISTENT_ROOT_CASES = (
    ('migrate-manifests-equal', 'The migrator reports equal source and destination manifests'),
    ('host-manifests-equal', 'Cold filesystem manifests match and both ext2 images are unchanged'),
    ('cutover-verified', 'The BreenFS generation boots and verifies before promotion'),
    ('rescue-and-return', 'The saved ext2 rescue boots read-only and returns to BreenFS'),
)
EXT2_DISKS = ('system.img', 'home.img')
BREENFS_DISKS = ('breenfs-system.img', 'breenfs-home.img')
MIGRATION_DISKS = (*BREENFS_DISKS, 'test-root.img')
CYCLE_RECORD = 'migration-cycle.json'
MIGRATION_PROBE = 'migration-ssh.json'
RETIREMENT_RECORD = 'retirement.json'
SHARED_BINDS = ('/root', '/etc', '/var', '/opt', '/srv', '/tmp', '/run')


def validate_manifest(path):
    manifest = json.loads(Path(path).read_text())
    # The metal tools account for power-cut and ext2-retired.
    actual = [(category['id'], case['id'], case['title'])
              for category in manifest['categories']
              if category.get('hostRunner') is True and category['id'] not in ('power-cut', 'ext2-retired')
              for case in category['cases']]
    expected = [('persistent-root', name, title) for name, title in PERSISTENT_ROOT_CASES]
    if manifest['id'] != 'filesystem' or actual != expected:
        raise ValueError('host case table differs from the manifest in ids, order or titles')


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def disk_digests(run, names):
    return {name: digest(run / name) for name in names}


def slots_module():
    spec = importlib.util.spec_from_file_location('vm_slots', Path(__file__).with_name('vm_slots.py'))
    slots = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(slots)
    return slots


def build_migration_images(run, tools):
    roles = ('system', 'home', 'breenfs-system', 'breenfs-home', 'test-root')
    guids = {role: str(uuid.uuid4()) for role in roles}
    test_root = run / 'test-root.img'
    shutil.copyfile(run / 'system.img', test_root)
    for path in ('/migrate', '/migrate/ext2-system', '/migrate/ext2-home', '/migrate/system', '/migrate/home'):
        subprocess.run(['debugfs', '-w', '-R', 'mkdir ' + path, test_root],
                       check=True, capture_output=True)
        readback = subprocess.check_output(['debugfs', '-R', 'stat ' + path, test_root],
                                           stderr=subprocess.PIPE)
        if b'Type: directory' not in readback:
            raise ValueError('cannot stage migration mount point ' + path)
    for role in ('system', 'home', 'test-root'):
        subprocess.run(['tune2fs', '-U', guids[role], run / (role + '.img')],
                       check=True, stdout=subprocess.DEVNULL)
    for role in ('system', 'home'):
        destination = 'breenfs-' + role
        size = (run / (role + '.img')).stat().st_size
        # Leave working space for COW metadata and a nearly full source volume.
        size = max(64 << 20, size * 2)
        subprocess.run([Path(tools) / 'breenfs-mkfs', '--uuid', guids[destination].replace('-', ''),
                        '--size', str(size), run / (destination + '.img')], check=True)
    slots_module().write_json(run / 'volume-guids.json', guids)


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()


def seal(root, run, source_commit):
    run = Path(run)
    commit = git(root, 'rev-parse', 'HEAD')
    dirty = bool(git(root, 'status', '--porcelain'))
    if source_commit and (source_commit != commit or dirty):
        raise ValueError('sources changed during build')
    artifacts = {name: digest(run / name) for name in ('system.img', 'home.img', 'power-token')}
    if source_commit:
        artifacts['kernel'] = digest(run / 'kernel')
        if (run / 'fault-kernel').exists():
            artifacts['fault-kernel'] = digest(run / 'fault-kernel')
    fault_kind = (run / 'fault-kind').read_text().strip() if (run / 'fault-kind').exists() else None
    if fault_kind not in (None, 'panic-reboot', 'watchdog') or bool(fault_kind) != ('fault-kernel' in artifacts):
        raise ValueError('invalid fault kernel kind')
    build = dict(commit=commit if source_commit else None, dirty=dirty, sourceBuilt=bool(source_commit), artifacts=artifacts, faultKind=fault_kind)
    if (run / 'volume-guids.json').is_file():
        build['volumeGuids'] = json.loads((run / 'volume-guids.json').read_text())
        build['migrationImages'] = disk_digests(run, MIGRATION_DISKS)
    (run / 'build.json').write_text(json.dumps(build) + '\n')


def seal_candidate(root, run, staging, commit, fault):
    run, staging = Path(run), Path(staging)
    if commit != git(root, 'rev-parse', 'HEAD') or git(root, 'status', '--porcelain'):
        raise ValueError('sources changed during candidate staging')
    generation = 'vm-' + secrets.token_hex(16)
    seal = dict(commit=commit, generation=generation, injectedFault=fault or None,
                imageSha256=digest(staging / 'image'), kernelSha256=digest(staging / 'kernel'))
    # Invalidate the old seal before publishing either new artifact.
    (run / 'candidate.json').unlink(missing_ok=True)
    (staging / 'image').replace(run / 'candidate.img')
    (staging / 'kernel').replace(run / 'candidate-kernel')
    temporary = staging / 'candidate.json'
    temporary.write_text(json.dumps(seal) + '\n')
    temporary.replace(run / 'candidate.json')


def verify(root, run, kernel, fault=False, slot_digest=None):
    run = Path(run)
    build = json.loads((run / 'build.json').read_text())
    # QEMU uses this private copy, rather than a mutable shared target output.
    shutil.copyfile(kernel, run / 'boot-kernel')
    if not (run / 'power-token').is_file() or 'power-token' not in build['artifacts']:
        raise ValueError('VM run predates power-token sealing; build a new run')
    if digest(run / 'power-token') != build['artifacts']['power-token']:
        raise ValueError('VM power token differs from the sealed token')
    if slot_digest is not None:
        seals = json.loads((run / 'slot-kernels.json').read_text())
        if seals.get('buildCommit') != build.get('commit') or slot_digest not in seals.get('kernels', []):
            raise ValueError('slot kernel is not sealed for this run')
        if digest(run / 'boot-kernel') != slot_digest:
            raise ValueError('slot kernel differs from its seal')
    if fault and build.get('sourceBuilt') is not True:
        raise ValueError('fault boot requires a source-built kernel')
    eligible = False
    expected = build['artifacts'].copy()
    continuation = run / 'continuation.json'
    if continuation.exists():
        seal = json.loads(continuation.read_text())
        if seal['buildCommit'] != build.get('commit'):
            raise ValueError('continuation belongs to a different build')
        expected.update(seal['disks'])
    if build.get('sourceBuilt') is True:
        for name in ('system.img', 'home.img'):
            if digest(run / name) != expected[name]:
                raise ValueError('VM artifacts differ from the latest seal (disk)')
    if build.get('sourceBuilt') is True:
        kernel_name = 'fault-kernel' if fault else 'kernel'
        if slot_digest is None and digest(run / 'boot-kernel') != expected.get(kernel_name):
            raise ValueError('VM artifacts differ from the source build (kernel)')
        if build['commit'] != git(root, 'rev-parse', 'HEAD') or git(root, 'status', '--porcelain'):
            raise ValueError('source build does not match the clean tools commit')
        eligible = build.get('dirty') is False
    actual = dict(expected)
    actual.update({name: digest(run / name) for name in ('system.img', 'home.img')})
    if slot_digest is not None:
        actual['kernel'] = slot_digest
    receipt = dict(build, artifacts=actual, evidenceEligible=eligible)
    (run / 'verified-build.json').write_text(json.dumps(receipt) + '\n')
    return receipt


def seal_slot(root, run, kernel):
    run, kernel = Path(run), Path(kernel)
    if kernel.is_symlink() or kernel.resolve().parent != run.resolve():
        raise ValueError('slot kernel must be a private file in the run directory')
    build = json.loads((run / 'build.json').read_text())
    path = run / 'slot-kernels.json'
    seals = json.loads(path.read_text()) if path.exists() else {'buildCommit': build.get('commit'), 'kernels': []}
    if seals.get('buildCommit') != build.get('commit'):
        raise ValueError('slot seal belongs to a different build')
    if build.get('sourceBuilt') and (build.get('commit') != git(root, 'rev-parse', 'HEAD') or git(root, 'status', '--porcelain')):
        raise ValueError('source build does not match the clean tools commit')
    sha = digest(kernel)
    if sha not in seals['kernels']:
        seals['kernels'].append(sha)
    tmp = path.with_suffix('.json.tmp')
    tmp.write_text(json.dumps(seals) + '\n')
    tmp.replace(path)


def verify_slot(root, run, slot):
    slots = slots_module()
    selected = slots.select(run, slot)
    if selected is None:
        raise ValueError('slot record missing')
    receipt = verify(root, run, Path(run) / selected['kernel'], slot_digest=selected['kernelSha256'])
    (Path(run) / 'boot-intent').write_bytes(slots.intent(selected))
    return receipt


def named_boot(run, name):
    if not re.fullmatch(r'[0-9]{8}T[0-9]{6}Z-qemu', name):
        raise ValueError('invalid VM boot identity')
    return run / 'boots' / name


def current_boot(run):
    return named_boot(run, (run / 'current').read_text().strip())


def shutdown_verified(events):
    requested = False
    for event in events:
        if event.get('event') == 'requested-shutdown':
            requested = event.get('action') == 'poweroff'
        if event.get('event') == 'SHUTDOWN' and event.get('data', {}).get('reason') == 'guest-shutdown' and requested:
            return True
    return False


def continuation(run):
    run = Path(run)
    boot = current_boot(run)
    events = [json.loads(line) for line in (boot / 'events.jsonl').read_text().splitlines()]
    if shutdown_verified(events):
        build = json.loads((run / 'build.json').read_text())
        seal = {'buildCommit': build.get('commit'), 'disks': {name: digest(run / name) for name in ('system.img', 'home.img')}}
        tmp = run / 'continuation.json.tmp'
        tmp.write_text(json.dumps(seal) + '\n')
        tmp.replace(run / 'continuation.json')


def cycle_state(run):
    return json.loads((run / CYCLE_RECORD).read_text())


def frozen_ext2(run, cycle):
    if disk_digests(run, EXT2_DISKS) != cycle['ext2']:
        raise ValueError('frozen ext2 image SHA-256 changed')


def render_cycle_intent(run, build, cycle):
    slots = slots_module()
    phase = cycle['phase']
    record = slots.read(run)
    slot = slots.migration_slot(phase, record['current'].get('layout') == 'breenfs')
    selected = slots.select(run, slot)
    if selected is None or selected['userland'] != cycle['userland']:
        raise ValueError('migration slot is missing or names a different userland')
    expected = {'cutover': 'breenfs', 'return': 'breenfs', 'rescue': 'ext2-rescue'}.get(phase, 'ext2')
    if selected.get('layout', 'ext2') != expected:
        raise ValueError('migration slot has the wrong filesystem layout')
    intent = slots.migration_intent(phase, cycle['userland'], build['volumeGuids'])
    (run / 'boot-intent').write_bytes(intent)
    return selected


def stage_cycle(run, operation):
    build = json.loads((run / 'build.json').read_text())
    slots = slots_module()
    record = slots.read(run)
    if record is None:
        raise ValueError('migration needs an installed current generation')
    if 'migrationImages' not in build:
        raise ValueError('VM run predates migration images; build a new run')
    if operation == 'migrate':
        if (run / CYCLE_RECORD).exists() or record['next'] is not None:
            raise ValueError('migration or an upgrade is already staged')
        selected = slots.select(run, 'current')
        disks = disk_digests(run, MIGRATION_DISKS)
        if disks != build['migrationImages']:
            raise ValueError('migration needs the sealed empty BreenFS images and test root')
        cycle = dict(userland=selected['userland'], ext2=disk_digests(run, EXT2_DISKS),
                     disks=disks, boots={})
    else:
        cycle = cycle_state(run)
        frozen_ext2(run, cycle)
        current = slots.select(run, 'current')
        promoted = current.get('layout') == 'breenfs'
        if operation == 'cutover':
            next_slot = dict(current, layout='breenfs')
            if (promoted or record['next'] not in (None, next_slot) or not cycle['boots'].get('migrate')
                    or cycle.get('compared') != disk_digests(run, MIGRATION_DISKS)):
                raise ValueError('cutover needs equal guest and cold manifests for the sealed images')
            record['current'] = dict(current, layout='ext2-rescue')
            record['next'] = next_slot
            slots.write(run, record)
            cycle['boots'].pop('cutover', None)
        elif not promoted:
            raise ValueError('rescue and return need a promoted BreenFS slot')
    cycle['phase'] = operation
    render_cycle_intent(run, build, cycle)
    slots.write_json(run / CYCLE_RECORD, cycle)


def promote_cycle(run):
    cycle = cycle_state(run)
    slots = slots_module()
    if cycle['phase'] != 'cutover' or not cycle['boots'].get('cutover'):
        raise ValueError('promotion needs a verified BreenFS cutover boot')
    check_cycle_probe(run, cycle, 'cutover', cycle['boots']['cutover'])
    frozen_ext2(run, cycle)
    if slots.select(run, 'current').get('layout') != 'breenfs':
        if slots.select(run, 'next').get('layout') != 'breenfs':
            raise ValueError('NEXT is not the BreenFS slot')
        slots.promote(run)


def rollback_cycle(run):
    cycle = cycle_state(run)
    slots = slots_module()
    record = slots.read(run)
    current = slots.select(run, 'current')
    if current.get('layout') == 'breenfs':
        raise ValueError('use rescue to select the saved ext2 slot')
    record['current'] = dict(current, layout='ext2-rescue')
    record['next'] = None
    slots.write(run, record)
    cycle['phase'] = 'rescue'
    slots.write_json(run / CYCLE_RECORD, cycle)
    render_cycle_intent(run, json.loads((run / 'build.json').read_text()), cycle)


def verify_cycle(root, run):
    cycle = cycle_state(run)
    frozen_ext2(run, cycle)
    if cycle['disks'] != disk_digests(run, MIGRATION_DISKS):
        raise ValueError('migration images differ from their latest seal')
    build = json.loads((run / 'build.json').read_text())
    selected = render_cycle_intent(run, build, cycle)
    verify(root, run, run / selected['kernel'], slot_digest=selected['kernelSha256'])
    print(cycle['phase'])


def validate_cycle_probe(probe, boot, phase, userland):
    if probe['boot'] != boot or probe['phase'] != phase or probe['generation'] != userland:
        raise ValueError('SSH evidence names a different boot, phase or generation')
    if not re.fullmatch(r'[0-9a-f]{64}', probe['sentinelSha256']):
        raise ValueError('SSH evidence has no verified home sentinel')

    mounts = {}
    for line in probe['mounts'].splitlines():
        _, path, kind, options, _, _ = line.split()
        if path in mounts:
            raise ValueError(f'duplicate mount for {path}')
        mounts[path] = (kind, set(options.split(',')))

    def require_mount(path, kind, mode, *flags):
        actual_kind, options = mounts.get(path, ('', set()))
        if actual_kind != kind or options & {'ro', 'rw'} != {mode} or not set(flags) <= options:
            raise ValueError(f'{path} is not mounted as {kind} {",".join((mode, *flags))}')

    if phase == 'rescue':
        require_mount('/', 'ext2', 'ro')
        require_mount('/home', 'ext2', 'ro')
        for path in '/tmp', '/run':
            require_mount(path, 'breenfs', 'rw')
    else:
        require_mount('/', 'breenfs', 'ro')
        require_mount('/home', 'breenfs', 'rw', 'nosuid', 'nodev')
        for path in SHARED_BINDS:
            require_mount(path, 'breenfs', 'rw')


def check_cycle_probe(run, cycle, phase, boot):
    probe = json.loads((run / 'boots' / boot / MIGRATION_PROBE).read_text())
    validate_cycle_probe(probe, boot, phase, cycle['userland'])


def has_cycle_probe(run, cycle, phase, boot):
    if boot:
        try:
            check_cycle_probe(run, cycle, phase, boot)
            return True
        except (OSError, ValueError, TypeError, KeyError, AttributeError):
            return False
    return False


def probe_cycle(run, identity, public_host_key):
    """Capture a live, read-only operator SSH probe before guest poweroff."""
    import vm_site

    retirement = retirement_state(run) if (run / RETIREMENT_RECORD).is_file() else None
    cycle = cycle_state(run) if retirement is None else None
    phase = (retirement or cycle)['phase']
    if phase not in ('cutover', 'rescue', 'return', 'verify', 'final') or not (run / 'boot.lock').is_dir():
        raise ValueError('SSH verification needs a live migration or retirement verification boot')
    boot = current_boot(run)
    if (boot / 'ended').exists():
        raise ValueError('SSH verification needs a live boot')
    if retirement is not None:
        retirement_launch(boot, retirement)
    site = vm_site.site(run)
    user, uid = site['OPERATOR_USER'], site['OPERATOR_UID']
    if not re.fullmatch(r'[a-z_][a-z0-9_-]{0,31}', user) or not uid.isascii() or not uid.isdecimal() or int(uid) == 0:
        raise ValueError('SSH verification needs a non-root operator account')
    port = str(site['SERVER_SSH_PORT'])
    host_key = ' '.join(Path(public_host_key).read_text().split()[:2])
    if not re.fullmatch(r'ssh-ed25519 [A-Za-z0-9+/]+={0,2}', host_key):
        raise ValueError('expected the trusted guest Ed25519 public host key')
    key_hash = hashlib.sha256(host_key.encode('ascii')).hexdigest()
    if retirement is None:
        expected = subprocess.check_output(['debugfs', '-R', f'cat /{user}/migration.txt', run / 'home.img'],
                                           stderr=subprocess.PIPE)
        if not expected:
            raise ValueError('frozen ext2 home has no nonempty operator migration.txt sentinel')
        expected_hash = hashlib.sha256(expected).hexdigest()
        userland = cycle['userland']
    else:
        if key_hash != retirement['hostKeySha256']:
            raise ValueError('trusted host key differs from the verified migration return')
        expected_hash = retirement['sentinelSha256']
        userland = retirement['pending']['generation']

    with tempfile.TemporaryDirectory(dir=boot) as directory:
        known = Path(directory) / 'known_hosts'
        known.write_text(f'[127.0.0.1]:{port} {host_key}\n')
        args = ['ssh', '-F', '/dev/null', '-o', 'StrictHostKeyChecking=yes',
                '-o', f'UserKnownHostsFile={known}', '-o', 'GlobalKnownHostsFile=/dev/null',
                '-o', 'HostKeyAlgorithms=ssh-ed25519', '-o', 'BatchMode=yes',
                '-o', 'IdentitiesOnly=yes', '-o', 'ConnectTimeout=5',
                '-i', identity, '-p', port, '-l', user, '127.0.0.1']

        def ssh(command):
            return subprocess.check_output([*args, command], timeout=10)

        if ssh('id -u').strip() != uid.encode('ascii'):
            raise ValueError('SSH login returned the wrong operator UID')
        generation = ssh('cat /usr/share/factory/generation').decode().strip()
        mounts = ssh('cat /proc/mounts').decode()
        sentinel = ssh(f'cat /home/{user}/migration.txt')
        sentinel_hash = hashlib.sha256(sentinel).hexdigest()
        if (sentinel != expected if retirement is None else sentinel_hash != expected_hash):
            raise ValueError('SSH home sentinel differs from the migration baseline')

    probe = dict(boot=boot.name, phase=phase, generation=generation, mounts=mounts,
                 sentinelSha256=sentinel_hash, hostKeySha256=key_hash)
    validate_cycle_probe(probe, boot.name, phase, userland)
    if current_boot(run) != boot or (boot / 'ended').exists():
        raise ValueError('boot changed during SSH verification')
    slots_module().write_json(boot / MIGRATION_PROBE, probe)
    print(f'{phase}: verified SSH generation, mounts and home sentinel for {boot.name}')


def finish_cycle(run, phase, exit_status):
    cycle = cycle_state(run)
    if cycle['phase'] != phase:
        raise ValueError('migration phase changed during boot')
    frozen_ext2(run, cycle)
    boot = current_boot(run)
    serial = (boot / 'serial.txt').read_text(errors='replace').splitlines()
    events = [json.loads(line) for line in (boot / 'events.jsonl').read_text().splitlines()]
    clean = (exit_status == 0 and (boot / 'ended').is_file()
             and not any('ROOT_MOUNT_FAILURE' in line or 'Kernel panic' in line for line in serial))
    if phase == 'migrate':
        exits = [line for line in serial if line.startswith('[migrate] exit code:')]
        verified = (clean and shutdown_verified(events) and exits == ['[migrate] exit code: 0']
                    and '[migrate] manifests equal' in serial
                    and serial.index('[migrate] manifests equal') < serial.index(exits[0]))
    else:
        filesystem = 'ext2' if phase == 'rescue' else 'breenfs'
        verified = (clean and shutdown_verified(events)
                    and f'[boot] {filesystem} root filesystem mounted' in serial
                    and 'bsshd: listening on 0.0.0.0:22' in serial
                    and has_cycle_probe(run, cycle, phase, boot.name))
    if phase == 'migrate' or phase == 'rescue':
        cycle['boots'].pop('return', None)
    cycle['boots'][phase] = boot.name if verified else None
    # This is the stopped image after this specific launch, under the run lock.
    cycle['disks'] = disk_digests(run, MIGRATION_DISKS)
    slots_module().write_json(run / CYCLE_RECORD, cycle)
    write_cycle_result(run, cycle)
    if not verified:
        if phase == 'cutover' and slots_module().read(run)['next'] is not None:
            rollback_cycle(run)
        raise ValueError(f'{phase} boot did not supply its verification receipts')


def compare_manifests(run, cycle, tools):
    frozen_ext2(run, cycle)
    for image in BREENFS_DISKS:
        subprocess.run([tools / 'breenfs-check', '--full', run / image], check=True, capture_output=True)

    reader = Path(__file__).with_name('metal') / 'migrate_manifest.py'

    def ext2(image, *args):
        return subprocess.check_output([sys.executable, reader, 'ext2', run / image, *args])

    def breenfs(image, *args):
        return subprocess.check_output([tools / 'breenfs-extract', run / image, 'migration-manifest', *args])

    userland = cycle['userland']
    for tree, source, destination in (
        ('generation', ext2('system.img', '--subtree', '/.breenix-generations/' + userland),
         breenfs('breenfs-system.img', '--root', 'gen-' + userland)),
        ('shared root', ext2('system.img', '--shared'), breenfs('breenfs-system.img')),
        ('home', ext2('home.img'), breenfs('breenfs-home.img')),
    ):
        if not source.startswith(b'2f\t') or source != destination:
            raise ValueError(f'{tree} migration manifests differ')
    frozen_ext2(run, cycle)


def write_cycle_result(run, cycle):
    boots = cycle['boots']
    outcomes = (
        (boots.get('migrate'), 'missing guest migration equality receipt'),
        (cycle.get('compared'), cycle.get('comparisonError', 'cold manifests have not been compared')),
        (has_cycle_probe(run, cycle, 'cutover', boots.get('cutover')), 'missing verified cutover boot'),
        (all(has_cycle_probe(run, cycle, phase, boots.get(phase)) for phase in ('rescue', 'return')),
         'missing verified rescue and return boots'),
    )
    build = json.loads((run / 'build.json').read_text())
    cases = [dict(id='persistent-root/' + case, result='PASS' if passed else 'FAIL',
                  ms=cycle.get('comparisonMs', 0) if case == 'host-manifests-equal' else 0,
                  reason='' if passed else reason)
             for (case, _), (passed, reason) in zip(PERSISTENT_ROOT_CASES, outcomes)]
    record = dict(milestone='filesystem/persistent-root', commit=build.get('commit'),
                  boot=next((boots[phase] for phase in ('return', 'rescue', 'cutover', 'migrate')
                             if boots.get(phase)), None),
                  cases=cases, cleanup=True, consoleGaps=[], boots=boots)
    slots_module().write_json(run / 'filesystem-persistent-root.json', record)


def check_manifests(run, tools):
    cycle = cycle_state(run)
    started = time.monotonic()
    try:
        compare_manifests(run, cycle, tools)
        if cycle['disks'] != disk_digests(run, MIGRATION_DISKS):
            raise ValueError('cold images differ from the stopped image seal')
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        cycle.pop('compared', None)
        cycle['comparisonError'] = str(error)
        raise
    else:
        cycle['compared'] = cycle['disks']
        cycle.pop('comparisonError', None)
    finally:
        cycle['comparisonMs'] = int((time.monotonic() - started) * 1000)
        slots_module().write_json(run / CYCLE_RECORD, cycle)
        write_cycle_result(run, cycle)
    if not cycle['boots'].get('migrate'):
        raise ValueError('missing guest migration equality receipt')


def retirement_state(run):
    return json.loads((run / RETIREMENT_RECORD).read_text())


def retirement_build(root, run):
    build = json.loads((run / 'build.json').read_text())
    if (build.get('sourceBuilt') is not True or build.get('dirty') is not False
            or not re.fullmatch(r'[0-9a-f]{40}', build.get('commit') or '')
            or build['commit'] != git(root, 'rev-parse', 'HEAD') or git(root, 'status', '--porcelain')):
        raise ValueError('retirement requires a disposable run built from the clean tools commit')
    return build


def clean_serial(boot):
    lines = (boot / 'serial.txt').read_text(errors='replace').splitlines()
    if (not (boot / 'ended').is_file()
            or any('ROOT_MOUNT_FAILURE' in line or 'Kernel panic' in line for line in lines)):
        raise ValueError('boot did not finish cleanly')
    return lines


def retirement_events(boot):
    return [json.loads(line) for line in (boot / 'events.jsonl').read_text().splitlines()]


def verify_retirement_boot(boot, phase, generation):
    lines = clean_serial(boot)
    probe = json.loads((boot / MIGRATION_PROBE).read_text()) if (boot / MIGRATION_PROBE).is_file() else None
    if (not probe or '[boot] breenfs root filesystem mounted' not in lines
            or 'bsshd: listening on 0.0.0.0:22' not in lines
            or not shutdown_verified(retirement_events(boot))):
        raise ValueError(f'{phase} boot did not supply its verification receipts')
    validate_cycle_probe(probe, boot.name, phase, generation)
    if not re.fullmatch(r'[0-9a-f]{64}', probe.get('hostKeySha256', '')):
        raise ValueError('SSH evidence has no verified host key')
    return probe


def begin_retirement(root, run):
    import vm_gpt

    build = retirement_build(root, run)
    cycle = cycle_state(run)
    slots = slots_module()
    record = slots.read(run)
    boot = current_boot(run)
    if ((run / RETIREMENT_RECORD).exists() or cycle['phase'] != 'return'
            or cycle['boots'].get('return') != boot.name or record['next'] is not None
            or not all(cycle['boots'].get(phase) for phase in ('migrate', 'cutover', 'rescue'))
            or not cycle.get('compared')):
        raise ValueError('retirement needs a completed, verified migration return with no staged slot')
    current, rescue = (slots.select(run, slot) for slot in ('current', 'previous'))
    if (current.get('layout') != 'breenfs' or rescue.get('layout') != 'ext2-rescue'
            or current['userland'] != cycle['userland'] or rescue['userland'] != cycle['userland']):
        raise ValueError('retirement needs the promoted BreenFS generation and its ext2 rescue')
    metadata = json.loads((boot / 'run.json').read_text())
    if (metadata['id'] != boot.name or metadata['target'] != 'qemu'
            or metadata['bootedCommit'] != build['commit'] or metadata['toolsCommit'] != build['commit']):
        raise ValueError('migration return belongs to another run or tools commit')
    frozen_ext2(run, cycle)
    if cycle['disks'] != disk_digests(run, MIGRATION_DISKS):
        raise ValueError('migration images differ from their stopped seal')
    probe = verify_retirement_boot(boot, 'return', current['userland'])
    layout = vm_gpt.create(run, build['volumeGuids'])
    state = dict(phase='ready', commit=build['commit'], migration=boot.name, lastBoot=boot.name,
                 base=current['userland'], rescue=rescue, layout=layout, upgrades=[],
                 sentinelSha256=probe['sentinelSha256'], hostKeySha256=probe['hostKeySha256'])
    slots.write_json(run / RETIREMENT_RECORD, state)


def retirement_intent(run, state, generation, install=False):
    guids = {role: part[0] for role, part in state['layout']['parts'].items()}
    guids['test-root'] = guids.pop('test')
    rendered = slots_module().migration_intent('upgrade' if install else 'return', generation, guids)
    (run / 'boot-intent').write_bytes(rendered)
    return rendered


def prepare_retirement(root, run, operation, slot):
    import vm_gpt

    state = retirement_state(run)
    build = retirement_build(root, run)
    slots = slots_module()
    record = slots.read(run)
    if (build['commit'] != state['commit'] or current_boot(run).name != state['lastBoot']):
        raise ValueError('retirement build or last boot changed')
    install = operation == 'install'
    phase = state['phase']
    if install and phase == 'ready' and len(state['upgrades']) < 2 and record['next'] is None:
        phase = 'install'
        current = slots.select(run, 'current')
        previous = slots.select(run, 'previous')
        if current.get('layout') != 'breenfs' or previous.get('layout') not in ('ext2-rescue', 'breenfs'):
            raise ValueError('upgrade needs the retained BreenFS slots')
        generation = 'vm-' + secrets.token_hex(16)
        refs = [current['userland'],
                previous['userland'] if previous.get('layout') == 'breenfs' else 'empty', generation]
        (run / 'install-references').write_text(' '.join(refs), encoding='ascii')
        selected = dict(kernel='kernel', kernelSha256=build['artifacts']['kernel'])
    elif operation == 'boot' and (phase, slot) in (
            ('next', 'next'), ('reclaimed', 'current'), ('done', 'current')):
        phase = 'verify' if phase == 'next' else 'final'
        selected = slots.select(run, slot)
        if selected.get('layout') != 'breenfs':
            raise ValueError('verification slot is not BreenFS')
        generation = selected['userland']
    else:
        raise ValueError('retirement requires the next ordered install, NEXT verification, reclaim or CURRENT boot')
    with vm_gpt.open_disk(run, state['layout']) as disk:
        vm_gpt.readback(disk, state['layout'], phase == 'final')
    receipt = verify(root, run, run / selected['kernel'],
                     slot_digest=None if install else selected['kernelSha256'])
    retirement_intent(run, state, generation, install)
    pending = dict(phase=phase, generation=generation, launch=secrets.token_hex(16))
    slots.write_json(run / 'verified-build.json', dict(receipt, retirement=pending))
    return list(vm_gpt.drive_args(run, state['layout'], install))


def start_retirement(run):
    # Commit the request only when the launcher is about to hand QEMU the disk.
    state = retirement_state(run)
    pending = json.loads((run / 'verified-build.json').read_text())['retirement']
    allowed = {'install': ('ready',), 'verify': ('next',), 'final': ('reclaimed', 'done')}
    if state['phase'] not in allowed[pending['phase']] or current_boot(run).name != state['lastBoot']:
        raise ValueError('retirement launch no longer follows the last boot')
    state.update(phase=pending['phase'], pending=pending)
    slots_module().write_json(run / RETIREMENT_RECORD, state)


def retirement_launch(boot, state):
    launch = json.loads((boot / 'run.json').read_text())
    intervening = [entry for entry in boot.parent.iterdir()
                   if state['lastBoot'] < entry.name <= boot.name and entry.is_dir()]
    if (launch['id'] != boot.name or launch['target'] != 'qemu'
            or launch['toolsCommit'] != state['commit'] or launch['bootedCommit'] != state['commit']
            or launch.get('retirement') != state['pending']
            or intervening != [boot]):
        raise ValueError('boot is not the requested VM retirement launch')


def verify_retirement_install(boot):
    lines = clean_serial(boot)
    expected = ('[boot] ext2 root filesystem mounted',
                "[persistent] install starting: the test root's userland onto BreenFS",
                '[persistent] recovered slot references needed', '[persistent] layout set-ID check passed')
    receipts = [line for line in lines if line in expected or line.startswith('[persistent] install complete:')]
    if (tuple(receipts[:-1]) != expected
            or not re.fullmatch(r'\[persistent\] install complete: files=[1-9][0-9]* dirs=[0-9]+ '
                                r'links=[0-9]+ hard-links=[0-9]+ bytes=[1-9][0-9]* left-out=[0-9]+',
                                receipts[-1])
            or any(line.startswith('[persistent] install failed:') for line in lines)
            or [event.get('data', {}).get('reason') for event in retirement_events(boot)
                if event.get('event') == 'SHUTDOWN'] != ['host-qmp-quit']):
        raise ValueError('BreenFS install did not supply its completion and QMP shutdown receipts')


def verify_retirement_probe(boot, state, phase, generation):
    probe = verify_retirement_boot(boot, phase, generation)
    if any(probe[key] != state[key] for key in ('hostKeySha256', 'sentinelSha256')):
        raise ValueError('SSH evidence differs from the migration baseline')


def retirement_slots(run, state):
    slots = slots_module()
    record = slots.read(run)
    for row, slot in zip(reversed(state['upgrades']), ('current', 'previous'), strict=True):
        selected = slots.select(run, slot)
        if selected['userland'] != row['generation'] or selected.get('layout') != 'breenfs':
            raise ValueError('slot does not name the verified BreenFS upgrade')
    if record['next'] is not None:
        raise ValueError('retirement requires both verified BreenFS slots with no pending NEXT')
    return record['current']


def retirement_proof(state, boot, case):
    return dict(milestone='filesystem/ext2-retired', commit=state['commit'],
                toolsCommit=state['commit'], migration=state['migration'], boot=boot,
                upgrades=state['upgrades'], cleanup=True, consoleGaps=[],
                cases=[dict(id='ext2-retired/' + case, result='PASS', ms=0)])


def finish_retirement(run, exit_status, monitor_status):
    import vm_gpt

    slots = slots_module()
    state = retirement_state(run)
    phase, generation = (state['pending'][key] for key in ('phase', 'generation'))
    boot = current_boot(run)
    retirement_launch(boot, state)
    if exit_status != 0 or monitor_status != 0:
        raise ValueError(f'{phase} boot did not stop successfully')
    with vm_gpt.open_disk(run, state['layout']) as disk:
        gpt = vm_gpt.readback(disk, state['layout'], phase == 'final')
    if phase == 'install':
        verify_retirement_install(boot)
        record = slots.read(run)
        if record['next'] is not None:
            raise ValueError('NEXT changed during installation')
        sha = digest(run / 'boot-kernel')
        if sha != json.loads((run / 'build.json').read_text())['artifacts']['kernel']:
            raise ValueError('install kernel differs from its seal')
        record['next'] = dict(kernel='kernel', kernelSha256=sha, userland=generation, layout='breenfs')
        slots.write(run, record)
        state.update(phase='next', install=boot.name)
    else:
        verify_retirement_probe(boot, state, phase, generation)
        if phase == 'verify':
            if slots.select(run, 'next')['userland'] != generation:
                raise ValueError('NEXT changed during verification')
            previous = slots.select(run, 'current')['userland']
            state['upgrades'].append(dict(install=state.pop('install'), verification=boot.name,
                                          generation=generation, previous=previous))
            slots.promote(run)
            state['phase'] = 'ready'
        else:
            current = retirement_slots(run, state)
            if gpt != state['reclaimed']['after'] or generation != current['userland'] or 'rescue' in state:
                raise ValueError('final boot does not match the reclaimed GPT and CURRENT')
            answer = retirement_intent(run, state, current['userland'])
            proof = retirement_proof(state, boot.name, 'reclaimed')
            proof.update(state['reclaimed'], nextAnswer=dict(slot='current', userland=current['userland'],
                         intentSha256=hashlib.sha256(answer).hexdigest()))
            slots.write_json(run / 'filesystem-reclaimed.json', proof)
            state['phase'] = 'done'
    state['lastBoot'] = boot.name
    del state['pending']
    slots.write_json(run / RETIREMENT_RECORD, state)
    if phase == 'verify' and len(state['upgrades']) == 2:
        slots.write_json(run / 'filesystem-two-upgrades.json', retirement_proof(state, boot.name, 'two-upgrades'))


def reclaim_retirement(root, run):
    import vm_gpt

    state = retirement_state(run)
    build = retirement_build(root, run)
    if (state['phase'] != 'ready' or len(state['upgrades']) != 2 or not state.get('rescue')
            or build['commit'] != state['commit'] or current_boot(run).name != state['lastBoot']):
        raise ValueError('reclaim needs two verified upgrades in the same stopped VM run')
    last, generation = state['migration'], state['base']
    for row in state['upgrades']:
        install, verification = (named_boot(run, row[key]) for key in ('install', 'verification'))
        if (not last < install.name < verification.name or generation != row['previous']
                or generation == row['generation']):
            raise ValueError('reclaim needs two successive verified upgrades')
        for boot, phase in ((install, 'install'), (verification, 'verify')):
            launch = json.loads((boot / 'run.json').read_text())['retirement']
            if launch['generation'] != row['generation'] or launch['phase'] != phase:
                raise ValueError('upgrade boot does not match its recorded phase and generation')
            retirement_launch(boot, dict(state, pending=launch, lastBoot=last))
            last = boot.name
        verify_retirement_install(install)
        verify_retirement_probe(verification, state, 'verify', row['generation'])
        last, generation = verification.name, row['generation']
    proof = json.loads((run / 'filesystem-two-upgrades.json').read_text())
    if last != state['lastBoot'] or proof != retirement_proof(state, last, 'two-upgrades'):
        raise ValueError('reclaim needs the matching two-upgrades receipt')
    retirement_slots(run, state)
    slots = slots_module()
    layout = state['layout']
    with vm_gpt.open_disk(run, layout, write=True) as disk:
        before = vm_gpt.readback(disk, layout, False)
        # Block the next launch before editing either GPT copy.
        state['phase'] = 'reclaiming'
        slots.write_json(run / RETIREMENT_RECORD, state)
        vm_gpt.write_copies(disk, vm_gpt.tables(layout, True))
        after = vm_gpt.readback(disk, layout, True)
    del state['rescue']
    state.update(phase='reclaimed', reclaimed=dict(before=before, after=after, clearedEntries=[3, 4]))
    slots.write_json(run / RETIREMENT_RECORD, state)


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--check-manifest':
        try:
            validate_manifest(sys.argv[2])
        except (OSError, ValueError, KeyError, TypeError) as error:
            sys.exit(f'VM artifacts: {error}')
        sys.exit(0)
    operation, root, run, value, *extra = sys.argv[1:]
    if operation in ('begin-retirement', 'reclaim-ext2', 'prepare-retirement', 'start-retirement', 'finish-retirement'):
        run = Path(run)
        try:
            if operation == 'begin-retirement':
                begin_retirement(root, run)
            elif operation == 'reclaim-ext2':
                reclaim_retirement(root, run)
            elif operation == 'prepare-retirement':
                action, slot = value.split(':')
                print(*prepare_retirement(root, run, action, slot), sep='\n')
            elif operation == 'start-retirement':
                start_retirement(run)
            else:
                status, monitor = map(int, value.split(':'))
                finish_retirement(run, status, monitor)
        except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
            sys.exit(f'VM retirement: {error}')
        sys.exit(0)
    if operation == 'probe-cycle':
        if len(extra) != 1:
            sys.exit('SSH verification needs an identity and a trusted guest public host key')
        try:
            probe_cycle(Path(run), value, extra[0])
        except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
            sys.exit(f'VM migration: {error}')
        sys.exit(0)
    if operation in ('migration-images', 'migrate', 'cutover', 'rescue', 'return',
                     'check-manifests', 'promote-cycle', 'rollback-cycle', 'verify-cycle', 'finish-cycle'):
        if extra:
            sys.exit('unexpected migration arguments')
        run = Path(run)
        try:
            if operation == 'migration-images':
                build_migration_images(run, value)
            elif operation == 'check-manifests':
                check_manifests(run, Path(value))
            elif operation == 'promote-cycle':
                promote_cycle(run)
            elif operation == 'rollback-cycle':
                rollback_cycle(run)
            elif operation == 'verify-cycle':
                verify_cycle(root, run)
            elif operation == 'finish-cycle':
                phase, status = value.split(':')
                finish_cycle(run, phase, int(status))
            else:
                stage_cycle(run, operation)
        except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
            sys.exit(f'VM migration: {error}')
        sys.exit(0)
    if operation == 'seal-candidate':
        if len(extra) != 2:
            raise SystemExit('invalid candidate seal arguments')
        seal_candidate(root, run, value, *extra)
        sys.exit(0)
    if extra:
        raise SystemExit('unexpected arguments')
    if operation == 'seal':
        seal(root, run, value)
    elif operation == 'seal-slot':
        seal_slot(root, run, value)
    elif operation == 'verify-slot':
        verify_slot(root, run, value)
    elif operation in ('verify', 'verify-fault'):
        verify(root, run, value, operation == 'verify-fault')
    elif operation == 'stage-intent':
        build = json.loads((Path(run) / 'build.json').read_text())
        seal = {'buildCommit': build.get('commit'), 'disks': {name: digest(Path(run) / name) for name in ('system.img', 'home.img')}}
        tmp = Path(run) / 'continuation.json.tmp'
        tmp.write_text(json.dumps(seal) + '\n')
        tmp.replace(Path(run) / 'continuation.json')
        receipt_path = Path(run) / 'verified-build.json'
        receipt = json.loads(receipt_path.read_text())
        receipt['artifacts'].update(seal['disks'])
        receipt_path.write_text(json.dumps(receipt) + '\n')
    elif operation == 'continuation':
        continuation(run)
    else:
        raise SystemExit('expected seal or verify')
