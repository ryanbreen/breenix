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

COPY_OFF = '20261121T094229Z-metal'
BOOT_ID = re.compile(r'[0-9]{8}T[0-9]{6}Z-metal')
SHA256 = re.compile(r'[0-9a-f]{64}')
BLOCK = 4096
MAX_DATA = 32 << 30


def events(boot):
    return [json.loads(line) for line in (boot / 'events.jsonl').read_text().splitlines()]


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
        if allowed and name in allowed:
            # Earlier phases have their own leases and are bound to the exact recorded boot.
            if (record.get('migrationPhase') != allowed[name] or (boot / 'exit-status').read_text().strip() != '0'
                    or not any(row['event'] == 'power-off' and row.get('off') is True for row in log)):
                raise ValueError(f'unverified migration phase after copy-off: {name}')
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
    if commit != tools_commit or not re.fullmatch(r'[0-9a-f]{40}', tools_commit or ''):
        raise ValueError('migration needs matching source and deployed tools commits')
    layout = metal_persist.layout()
    cycle = state()
    if phase == 'add':
        current = metal_persist.read_disk().get('slots', {}).get('current') or {}
        generation = current.get('installedBy')
        if (cycle or not layout or layout['version'] != 1
                or not re.fullmatch(r'[A-Za-z0-9-]{1,64}', generation or '')
                or metal_persist.read_disk().get('slots', {}).get('next')):
            raise ValueError('add-breenfs needs the undisturbed v1 install and no pending candidate')
        cycle = dict(commit=commit, userland=generation, add=boot_id, boots={},
                     layout=metal_persist.add_breenfs_layout(layout))
        backup(layout, active=boot_id)
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
             for name, good in zip(('migrate-manifests-equal', 'host-manifests-equal',
                                    'cutover-verified', 'rescue-and-return'), passed, strict=True)]
    m.write_json(m.boot_dir(boot_id) / 'filesystem-persistent-root.json',
                 dict(milestone='filesystem/persistent-root', commit=cycle['commit'], boot=boot_id,
                      cases=cases, boots=boots, cleanup=True, consoleGaps=[]))
    save(cycle)
