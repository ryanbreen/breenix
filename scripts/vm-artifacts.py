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
import time
import uuid


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


def validate_manifest(path):
    manifest = json.loads(Path(path).read_text())
    actual = [(category['id'], case['id'], case['title'])
              for category in manifest['categories'] if category.get('hostRunner') is True
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


def current_boot(run):
    name = (run / 'current').read_text().strip()
    if not re.fullmatch(r'[0-9]{8}T[0-9]{6}Z-qemu', name):
        raise ValueError('invalid VM boot identity')
    return run / 'boots' / name


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
                    and 'bsshd: listening on 0.0.0.0:22' in serial)
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


def shared_manifest(body):
    prefixes = tuple(path.hex().encode('ascii') for path in (b'/.breenix-shared', b'/root'))
    selected = []
    first = {}
    counts = {}
    for line in body.splitlines():
        row = line.split(b'\t')
        path, group = row[0], row[5]
        if path != b'2f' and not any(path == prefix or path.startswith(prefix + b'2f') for prefix in prefixes):
            continue
        if group != b'-':
            first.setdefault(group, path)
            counts[group] = counts.get(group, 0) + 1
        selected.append(row)
    if not all(any(row[0] == prefix for row in selected) for prefix in prefixes):
        raise ValueError('ext2 system is missing shared state or root home')
    for row in selected:
        group = row[5]
        if group != b'-':
            row[5] = first[group] if counts[group] > 1 else b'-'
    return b''.join(b'\t'.join(row) + b'\n' for row in selected)


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
        ('shared root', shared_manifest(ext2('system.img')), breenfs('breenfs-system.img')),
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
        (boots.get('cutover'), 'missing verified cutover boot'),
        (boots.get('rescue') and boots.get('return'), 'missing verified rescue and return boots'),
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


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--check-manifest':
        try:
            validate_manifest(sys.argv[2])
        except (OSError, ValueError, KeyError, TypeError) as error:
            sys.exit(f'VM artifacts: {error}')
        sys.exit(0)
    operation, root, run, value, *extra = sys.argv[1:]
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
