"""Host evidence for the migration of the recorded R240 persistent install."""
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess

import metal_export
import metal_image
import metal_lib as m
import metal_persist
from migrate_manifest import Ext2, require_e2fsprogs, shared_manifest

# E24 ledger 996 and 1007 distinguish the staged slots from the return tools.
RECORDED_RETURN = '20261121T172024Z-metal'
RECORDED_CUTOVER_COMMIT = '94109fc7b3d95137cb91289f6a9f1321409cbcf6'
RECORDED_RETURN_COMMIT = '250fb833de97abddbf4672dbfca1980cb370cfe3'
PERSISTENT_ROOT_CASES = ('migrate-manifests-equal', 'host-manifests-equal',
                         'cutover-verified', 'rescue-and-return')
COPY_OFF = '20261121T094229Z-metal'
BOOT_ID = re.compile(r'[0-9]{8}T[0-9]{6}Z-metal')
SHA256 = re.compile(r'[0-9a-f]{64}')
COMMIT = re.compile(r'[0-9a-f]{40}')
BLOCK = 4096
MAX_DATA = 32 << 30


def events(boot):
    return [json.loads(line) for line in (boot / 'events.jsonl').read_text().splitlines()]


def safe_failed_attempt(boot, record, log):
    phase = record.get('migrationPhase')
    status = (boot / 'exit-status').read_text().strip()
    payload = ('persistent' if phase in ('rescue', 'return')
               else metal_export.PAYLOAD if phase == 'readback' else 'image')
    if (phase not in PHASES or not status.isdecimal() or int(status) in (m.EXIT_DONE, m.EXIT_INCOMPLETE)
            or record.get('payload') != payload
            or log[-1].get('event') != 'closed' or log[-1].get('endedBy') != 'driver'
            or log[-1].get('status') != int(status) or record.get('bootManagerAbort')):
        return False
    names = [row['event'] for row in log]
    if 'migration-not-powered' in names:
        return (not {'power-on', 'power-on-error', 'manager-answer'} & set(names)
                and not any(row['event'] == 'baseline' and row.get('power') != 'Off' for row in log))

    commit = record.get('commit')
    if (not COMMIT.fullmatch(commit or '')
            or record.get('toolsCommit') != commit
            or not any(row['event'] == 'migration-admitted' and row.get('phase') == phase for row in log)
            or 'power-on' not in names or 'capture-incomplete' in names or 'power-on-error' in names):
        return False
    answers = [row.get('answer') for row in log if row['event'] == 'manager-answer']
    power_off = [row for row in log if row['event'] == 'power-off']
    return (answers == [manager_answer(phase, record['id'], record.get('slotAction'))]
            and bool(power_off) and power_off[-1].get('off') is True)


def prove_history(index, boots, read_events, *, allowed=None, active=None):
    """Prove every indexed boot after the copy-off and refuse evidence gaps."""
    base = next((i for i, row in enumerate(index) if row['id'] == COPY_OFF), None)
    if base is None or sum(row['id'] == COPY_OFF for row in index) != 1:
        raise ValueError('the 24.16 copy-off is missing or ambiguous in the finished index')
    tail = index[base + 1:]
    sequence = index[base]['seq']
    if any(row['seq'] != sequence + offset or not BOOT_ID.fullmatch(row['id'])
           for offset, row in enumerate(tail, 1)):
        raise ValueError('the finished index has a gap after the copy-off')
    names = list(dict.fromkeys(row['id'] for row in tail))
    if set(names) != {boot.name for boot in boots.iterdir() if boot.is_dir()
                      and boot.name > COPY_OFF and boot.name != active}:
        raise ValueError('the boot directory and finished index disagree after the copy-off')
    for name in names:
        boot = boots / name
        record = m.read_json(boot / 'run.json', {}) or {}
        log = read_events(boot)
        if (not (boot / 'ended').is_file() or record.get('id') != name
                or not any(row['event'] == 'closed' for row in log)):
            raise ValueError(f'incomplete boot record after copy-off: {name}')
        if record.get('inject') == 'fence-out-of-partition':
            # This probe attempts a raw write immediately beyond the test partition.
            raise ValueError(f'boot after copy-off could have reached the frozen ext2 partitions: {name}')
        if allowed and name in allowed:
            # Earlier phases have their own leases and are bound to the exact recorded boot.
            if (record.get('migrationPhase') != allowed[name] or (boot / 'exit-status').read_text().strip() != '0'
                    or not any(row['event'] == 'power-off' and row.get('off') is True for row in log)):
                raise ValueError(f'unverified migration phase after copy-off: {name}')
            continue
        if any(row['event'] in ('migration-admitted', 'migration-not-powered') for row in log):
            if not safe_failed_attempt(boot, record, log):
                raise ValueError(f'failed migration phase lacks safe shutdown or routing evidence: {name}')
            continue
        payload = record.get('payload')
        if (record.get('bootManagerAbort') or record.get('upgradeParent')
                or (record.get('inject') or '').startswith('watchdog-')
                or not (payload == 'image' and record.get('disk') == 'root'
                        and record.get('mode') in ('default', 'suite', 'tests', 'console', 'probe', 'shell')
                        or payload == 'disk-readback' and record.get('disk') is None)
                or record.get('kind') not in ('boot', 'fault-test')):
            raise ValueError(f'boot after copy-off could have reached the frozen ext2 partitions: {name}')
        answers = [e.get('answer') for e in log if e['event'] == 'manager-answer']
        if not answers or any(answer != f'test {name}' for answer in answers):
            raise ValueError(f'missing or incorrect boot-manager test answer after copy-off: {name}')
    return names


def backup(layout, *, allowed=None, active=None, verify_images=False):
    """Bind the 24.16 images to the recorded GPT entries and subsequent boot history."""
    boot = m.boot_dir(COPY_OFF)
    run = m.read_json(boot / 'run.json', {}) or {}
    if (run.get('id') != COPY_OFF or run.get('payload') != metal_export.PAYLOAD
            or (boot / 'exit-status').read_text().strip() != '0'
            or not any(e['event'] == 'power-off' and e.get('off') is True for e in events(boot))):
        raise ValueError('the 24.16 copy-off has no successful powered-off export record')
    exports = (run.get('persistentExport') or {}).get('partitions', [])
    if [row.get('role') for row in exports] != list(metal_export.ROLES):
        raise ValueError('the copy-off did not record the two ext2 images')
    paths = {}
    for row in exports:
        role = row['role']
        part = layout['partitions'][role]
        size = (part['last'] - part['first'] + 1) * metal_persist.SECTOR
        path = metal_export.pool() / COPY_OFF / f'{role}.img'
        if (row.get('guid') != part['guid'] or row.get('firstLba') != part['first']
                or row.get('lastLba') != part['last'] or path.stat().st_size != size
                or row.get('hostBytes') != size or row.get('firmwareBytes') != size
                or not SHA256.fullmatch(row.get('hostSha256', ''))
                or row['hostSha256'] != row.get('firmwareSha256')
                or verify_images and metal_image.sha256(path) != row['hostSha256']):
            raise ValueError(f'the copy-off {role} image differs from its recorded partition')
        paths[role] = path
    index = [json.loads(line) for line in (m.STATE / 'finished.jsonl').read_text().splitlines()]
    checked = prove_history(index, m.STATE / 'boots', events, allowed=allowed, active=active)
    return paths, checked


def unpack_sparse(stream, output, size, content_length):
    """Reconstruct a partition from its allocation bitmap and ordered block stream."""
    total = size // BLOCK
    digest = hashlib.sha256()

    def read(count):
        data = stream.read(count)
        if len(data) != count:
            raise ValueError('short sparse readback')
        digest.update(data)
        return data

    map_size = (total + 7) // 8
    if size % BLOCK or total < 24 or not 8 + map_size < content_length <= 8 + map_size + MAX_DATA:
        raise ValueError('sparse readback length exceeds its bound')
    count, = struct.unpack('<Q', read(8))
    bitmap = read(map_size)
    if (count != sum(byte.bit_count() for byte in bitmap)
            or count * BLOCK + map_size + 8 != content_length
            or bitmap[-1] >> ((total - 1) % 8 + 1)
            or bitmap[0] or bitmap[1]):
        raise ValueError('invalid sparse readback bitmap or byte count')

    def present(block):
        return bool(bitmap[block // 8] & (1 << (block % 8)))

    if any(not present(block) for block in (*range(16, 20), *range(total - 4, total))):
        raise ValueError('sparse readback is missing a superblock slot')
    output.truncate(size)
    current = 0
    for index, byte in enumerate(bitmap):
        if not byte:
            continue
        for bit in range(8):
            if not byte & (1 << bit):
                continue
            block = index * 8 + bit
            if current != block:
                output.seek(block * BLOCK)
            if output.write(read(BLOCK)) != BLOCK:
                raise OSError('short sparse readback image write')
            current = block + 1
    return digest.hexdigest()


def join_manifests(shared, generation, home, userland):
    """Use the guest's logical paths and bytewise path order for the host evidence."""
    rows = {}
    for tree, prefix in ((shared, b''), (generation, (f'/gen-{userland}').encode()), (home, b'/home')):
        if not tree.startswith(b'2f\t'):
            raise ValueError('migration tree manifest is missing its root')
        for line in tree.splitlines():
            row = line.split(b'\t')
            for column in (0, 5):
                if row[column] != b'-' and prefix:
                    path = bytes.fromhex(row[column].decode('ascii'))
                    row[column] = (prefix + (b'' if path == b'/' else path)).hex().encode()
            if row[0] in rows:
                raise ValueError('duplicate logical path in migration manifest')
            rows[row[0]] = b'\t'.join(row) + b'\n'
    return b''.join(rows[path] for path in sorted(rows))


def verify_mounts(text, phase):
    mounts = {}
    for line in text.splitlines():
        _, path, fstype, flags, *_ = line.split()
        if path in mounts:
            raise ValueError('duplicate migration mount')
        mounts[path] = fstype, set(flags.split(','))
    required = (
        {'/': ('ext2', {'ro'}), '/home': ('ext2', {'ro'}),
         '/tmp': ('breenfs', {'rw'}), '/run': ('breenfs', {'rw'})}
        if phase == 'rescue' else
        {'/': ('breenfs', {'ro'}), '/home': ('breenfs', {'rw', 'nosuid', 'nodev'}),
         **{path: ('breenfs', {'rw'}) for path in ('/etc', '/var', '/opt', '/srv', '/tmp', '/run', '/root')}}
    )
    for path, (kind, flags) in required.items():
        actual, options = mounts.get(path, ('', set()))
        if actual != kind or not flags <= options or options & {'ro', 'rw'} != flags & {'ro', 'rw'}:
            raise ValueError(f'incorrect {phase} mount at {path}')


def compare_guest_manifests(source, destination, guest):
    digests = dict(source=hashlib.sha256(source).hexdigest(),
                   destination=hashlib.sha256(destination).hexdigest())
    if source != destination or digests != guest:
        raise ValueError('guest and host migration manifests differ')
    return digests


def score_manifests(paths, cold, userland, guest, tools):
    """Read the proven ext2 images and validate both independent BreenFS images."""
    debugfs, = require_e2fsprogs('debugfs')

    def ext2(role, path=b'/'):
        return ''.join(Ext2(debugfs, paths[role]).rows(path)).encode('ascii')

    def breenfs(role, *args):
        return subprocess.check_output([tools / 'breenfs-extract', cold / (role + '.img'),
                                        'migration-manifest', *args])

    for role in metal_persist.BREENFS_ROLES:
        subprocess.run([tools / 'breenfs-check', '--full', cold / (role + '.img')], check=True)
    source = join_manifests(shared_manifest(ext2('system')),
                            ext2('system', os.fsencode('/.breenix-generations/' + userland)),
                            ext2('home'), userland)
    destination = join_manifests(breenfs('breenfs-system'),
                                 breenfs('breenfs-system', '--root', 'gen-' + userland),
                                 breenfs('breenfs-home'), userland)
    for name, manifest in (('source.tsv', source), ('destination.tsv', destination)):
        (cold / name).write_bytes(manifest)
    return compare_guest_manifests(source, destination, guest)


PHASES = ('add', 'migrate', 'readback', 'cutover', 'rescue', 'return')
VERIFY_PHASES = ('cutover', 'rescue', 'return')


def state():
    return m.read_json(m.STATE / 'persistent-migration.json', {}) or {}


def in_progress():
    cycle = state()
    return bool(cycle) and set(cycle.get('boots', {})) != set(PHASES)


def require_closed(layout):
    cycle = state()
    if (cycle.get('closed') != RECORDED_RETURN or cycle.get('layout') != layout
            or set(cycle.get('boots', {})) != set(PHASES)
            or cycle['boots']['return'] != RECORDED_RETURN):
        raise ValueError('BreenFS upgrades require a BreenFS install intent from the closed, completed 24.18 migration')
    return cycle


def close(return_boot):
    """Close the recorded cycle without changing slots, routing or disk contents."""
    with m.locked('lease'):
        if m.read_lease() or m.read_queue():
            raise ValueError('closing the migration requires an idle machine lease and queue')
        cycle = state()
        boots = cycle.get('boots', {})
        layout = metal_persist.layout()
        if (return_boot != RECORDED_RETURN or boots.get('return') != return_boot
                or set(boots) != set(PHASES) or len(set(boots.values())) != len(PHASES)
                or any(not BOOT_ID.fullmatch(boot) for boot in boots.values())
                or cycle.get('add') != boots['add'] or not layout or layout['version'] != 2
                or cycle.get('layout') != layout):
            raise ValueError('closing the migration requires the recorded completed 24.18 cycle')
        if cycle.get('closed') == return_boot:
            return cycle
        if (m.read_json(m.STATE / 'machine.json', {}).get('sample') or {}).get('power') != 'Off':
            raise ValueError('closing the migration requires the recorded machine state Off')
        boot = m.boot_dir(return_boot)
        run = m.read_json(boot / 'run.json', {})
        proof = m.read_json(boot / 'filesystem-persistent-root.json', {})
        log = events(boot)
        answers = [row.get('answer') for row in log if row['event'] == 'manager-answer']
        off = [row.get('off') for row in log if row['event'] == 'power-off']
        slots = metal_persist.read_disk().get('slots', {})
        current, previous = slots.get('current') or {}, slots.get('previous') or {}
        if (not (boot / 'ended').is_file() or (boot / 'exit-status').read_text().strip() != '0'
                or not log or log[-1].get('event') != 'closed' or log[-1].get('endedBy') != 'driver'
                or log[-1].get('status') != 0 or not off or off[-1] is not True
                or answers != [manager_answer('return', return_boot)]
                or run.get('id') != return_boot or run.get('migrationPhase') != 'return'
                or run.get('payload') != 'persistent' or run.get('entry') != 'current'
                or run.get('commit') != RECORDED_RETURN_COMMIT or run.get('toolsCommit') != RECORDED_RETURN_COMMIT
                or run.get('bootedCommit') != RECORDED_CUTOVER_COMMIT
                or run.get('bootManagerAbort') or run.get('session') or run.get('resting')
                or proof.get('milestone') != 'filesystem/persistent-root'
                or proof.get('boot') != return_boot or proof.get('boots') != boots
                or proof.get('commit') != cycle.get('commit') or cycle.get('commit') != CARRY_FORWARD_CYCLE
                or proof.get('sourceCommit') != RECORDED_RETURN_COMMIT
                or proof.get('toolsCommit') != RECORDED_RETURN_COMMIT
                or proof.get('bootedCommit') != RECORDED_CUTOVER_COMMIT
                or proof.get('cleanup') is not True or proof.get('consoleGaps') != []
                or [(case.get('id'), case.get('result')) for case in proof.get('cases', ())]
                   != [('persistent-root/' + name, 'PASS') for name in PERSISTENT_ROOT_CASES]
                or slots.get('next') or current.get('migration') != 'cutover'
                or previous.get('migration') != 'rescue'
                or any(slot.get('commit') != RECORDED_CUTOVER_COMMIT
                       or slot.get('installedBy') != cycle.get('userland')
                       for slot in (current, previous))):
            raise ValueError('closing the migration requires a passed return to the recorded BreenFS slot')
        cycle['closed'] = return_boot
        save(cycle)
        return cycle


def manager_answer(phase, boot_id, slot_action=None):
    if phase == 'rescue':
        return f'persistent {boot_id} previous' + (' commit' if slot_action == 'commit' else '')
    if phase == 'return':
        return f'persistent {boot_id} current'
    return f'test {boot_id}'


def save(value):
    m.write_json(m.STATE / 'persistent-migration.json', value)


CARRY_FORWARD_CYCLE = '2aa932f2f61542f111c23be70dd824f7ba7fe411'
# E24 ledger line 985, as corrected by line 986.
CARRY_FORWARD_FILES = ('scripts/metal/server-suite', 'scripts/metal/metal_migration.py',
                       'tests/remote_upgrade_test.py', 'tests/metal_migration_test.py')
CARRY_FORWARD_RULES = {
    985: CARRY_FORWARD_FILES,
    996: CARRY_FORWARD_FILES + ('scripts/metal/metal-boot', 'tests/metal_persistent_test.py'),
}


def require_carry_forward(phase, cycle_commit, commit):
    if phase not in VERIFY_PHASES or cycle_commit != CARRY_FORWARD_CYCLE:
        raise ValueError('migration phase needs its cycle commit')
    try:
        records = [json.loads(line) for line in
                   (m.STATE / 'migration-carry-forward.jsonl').read_text().splitlines()]
    except (OSError, ValueError) as error:
        raise ValueError('migration carry-forward record is missing or unreadable') from error

    matching_lines = set()
    for record in records:
        if (not isinstance(record, dict)
                or record.get('cycleCommit') != cycle_commit or record.get('headCommit') != commit):
            continue
        ledger_line = record.get('ledgerLine')
        allowed = CARRY_FORWARD_RULES.get(ledger_line) if isinstance(ledger_line, int) else None
        if allowed is None:
            continue
        files = record.get('allowedFiles')
        if isinstance(files, list) and len(files) == len(allowed) and all(path in files for path in allowed):
            matching_lines.add(ledger_line)
    if not matching_lines:
        raise ValueError('migration needs a matching carry-forward record under E24 ledger line 985 or 996')

    # If both records match, line 996 includes the paths already allowed by line 985.
    ledger_line = max(matching_lines)
    allowed = CARRY_FORWARD_RULES[ledger_line]

    # Acceptance supplies the two commits in a local Git repository in the service state directory.
    repo = m.STATE / 'migration-source.git'
    try:
        diff = subprocess.run(['git', '--no-replace-objects', f'--git-dir={repo}',
                               'diff', '--name-only', '-z', '--no-relative', '--no-renames',
                               '--ignore-submodules=none',
                               f'{cycle_commit}..{commit}', '--'], check=True, capture_output=True, timeout=15)
    except (OSError, subprocess.SubprocessError) as error:
        raise ValueError('cannot verify the migration carry-forward Git range') from error
    outside = set(os.fsdecode(diff.stdout).split('\0')) - {''} - set(allowed)
    if outside:
        raise ValueError(f'migration carry-forward changes files outside E24 ledger line {ledger_line}: '
                         f'{sorted(outside)!r}')


def admit(phase, boot_id, commit, tools_commit):
    """Under the machine lease, authorize just the next phase at the pinned head."""
    if commit != tools_commit or not COMMIT.fullmatch(tools_commit or ''):
        raise ValueError('migration needs matching source and deployed tools commits')
    layout = metal_persist.layout()
    cycle = state()
    if phase == 'add':
        slots = metal_persist.read_disk().get('slots', {})
        current = slots.get('current') or {}
        generation = current.get('installedBy')
        if (not layout or cycle.get('boots')
                or not re.fullmatch(r'[A-Za-z0-9-]{1,64}', generation or '')
                or slots.get('next')):
            raise ValueError('add-breenfs needs an unmigrated install and no pending candidate')
        if cycle:
            expected = cycle['layout']
            original = dict(expected, version=1, partitions={
                role: expected['partitions'][role] for role in metal_persist.ROLES})
            if (cycle['commit'] != commit or cycle['userland'] != generation
                    or layout not in (original, expected)):
                raise ValueError('add-breenfs retry differs from the original migration request')
        else:
            cycle = dict(commit=commit, userland=generation, boots={},
                         layout=metal_persist.add_breenfs_layout(layout))
        backup(layout, active=boot_id)
        cycle['add'] = boot_id
        save(cycle)  # From here an idle manager request must select the test loop.
    else:
        previous = PHASES[:PHASES.index(phase)]
        if set(cycle.get('boots', {})) != set(previous) or layout != cycle.get('layout'):
            raise ValueError('migration phase needs the completed preceding phases and their layout')
        if cycle.get('commit') != commit:
            require_carry_forward(phase, cycle.get('commit'), commit)
        allowed = {ident: step for step, ident in cycle['boots'].items()}
        backup(layout, allowed=allowed, active=boot_id)
    return cycle


def record_phase(cycle, phase, boot_id):
    cycle['boots'][phase] = boot_id
    boots = cycle['boots']
    passed = ('migrate' in boots, 'readback' in boots, 'cutover' in boots,
              'rescue' in boots and 'return' in boots)
    cases = [dict(id='persistent-root/' + name, result='PASS' if good else 'FAIL', ms=0)
             for name, good in zip(PERSISTENT_ROOT_CASES, passed, strict=True)]
    directory = m.boot_dir(boot_id)
    run = m.read_json(directory / 'run.json')
    provenance = dict(sourceCommit=run['commit'], toolsCommit=run['toolsCommit'])
    if run.get('bootedCommit'):
        provenance['bootedCommit'] = run['bootedCommit']
    m.write_json(directory / 'filesystem-persistent-root.json',
                 dict(milestone='filesystem/persistent-root', commit=cycle['commit'], boot=boot_id,
                      cases=cases, boots=boots, cleanup=True, consoleGaps=[], **provenance))
    save(cycle)


def record_upgrades(first, second, commit):
    """Read the existing upgrade verifier's receipts from the service boot store."""
    from upgrade_cases import breenfs_pair

    cycle = require_closed(metal_persist.layout())
    if not COMMIT.fullmatch(commit):
        raise ValueError('two-upgrades requires a pinned source and tools commit')
    results = []
    for install in (first, second):
        if not BOOT_ID.fullmatch(install):
            raise ValueError('invalid install boot id')
        directory = m.boot_dir(install)
        result = m.read_json(directory / 'upgrade-result.json', {})
        candidate = result.get('verification') or {}
        verification = candidate.get('boot') or ''
        if result.get('install') != install or not BOOT_ID.fullmatch(verification):
            raise ValueError('upgrade result does not name its install and verification boots')
        child = m.boot_dir(verification)
        for boot in (directory, child):
            log = events(boot)
            off = [row.get('off') for row in log if row['event'] == 'power-off']
            closed = [(row.get('endedBy'), row.get('status')) for row in log if row['event'] == 'closed']
            run = m.read_json(boot / 'run.json', {})
            if (not (boot / 'ended').is_file() or (boot / 'exit-status').read_text().strip() != '0'
                    or closed != [('driver', 0)] or not off or off[-1] is not True
                    or run.get('id') != boot.name or run.get('toolsCommit') != commit
                    or run.get('breenfsUpgrade') != cycle['closed'] or run.get('bootManagerAbort')
                    or run.get('inject') or any(row['event'] == 'capture-incomplete' for row in log)):
                raise ValueError(f'upgrade boot has no successful clean shutdown record: {boot.name}')
            if boot == directory:
                links = [row.get('verification') for row in log
                         if row['event'] == 'persistent-verification-next']
                if (run.get('mode') != 'persistent-install' or run.get('commit') != commit
                        or links != [verification]):
                    raise ValueError('install boot does not identify its verification child and source commit')
            else:
                powers = [row['t'] for row in log if row['event'] == 'power-on']
                answers = [row['answer'] for row in log if row['event'] == 'manager-answer']
                recorded = m.read_json(child / 'ssh-verification.json', {})
                if (run.get('upgradeParent') != install or run.get('bootedCommit') != commit
                        or run.get('entry') != 'current' or powers != [candidate.get('powerOn')]
                        or answers != [f'persistent {verification} current commit']
                        or not recorded or dict(recorded, exit=0, boot=verification) != candidate):
                    raise ValueError('verification is not the install child that booted and passed S21')
        results.append(result)
    proof = breenfs_pair(cycle, commit, *results)
    expected = [ident for row in proof['upgrades'] for ident in (row['install'], row['verification'])]
    index = [json.loads(line) for line in (m.STATE / 'finished.jsonl').read_text().splitlines()]
    actual = []
    for row in index:
        ident = row['id']
        if not cycle['closed'] < ident <= proof['boot'] or not BOOT_ID.fullmatch(ident):
            continue
        run = m.read_json(m.boot_dir(ident) / 'run.json', {})
        if ident in expected or run.get('mode') == 'persistent-install' or run.get('upgradeParent'):
            actual.append(ident)
    if actual != expected:
        raise ValueError('two-upgrades needs the paired install and verification boots in order after the 24.18 return')
    m.write_json(m.boot_dir(proof['boot']) / 'filesystem-two-upgrades.json', proof)
    return proof


def main():
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('close').add_argument('return_boot')
    record = commands.add_parser('record-upgrades')
    record.add_argument('first')
    record.add_argument('second')
    record.add_argument('--sha', required=True)
    args = parser.parse_args()
    try:
        if args.command == 'close':
            print(close(args.return_boot)['closed'])
        else:
            print(json.dumps(record_upgrades(args.first, args.second, args.sha)))
    except (OSError, ValueError) as error:
        parser.exit(1, f'{error}\n')


if __name__ == '__main__':
    main()
