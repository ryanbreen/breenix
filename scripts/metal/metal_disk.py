"""Suite fixture delivery and cold raw readback through the firmware, without mounting the readback disk.

Before the persistent install the root is the whole disk, named by its storage record in LBA 0 (request format 1);
on the persistent layout it is the test root partition, named by its unique GUID (format 2, docs/metal/README.md,
"Test boots on the persistent layout")."""
import hashlib
import json
import os
from pathlib import Path
import secrets
import struct
from urllib.parse import urlsplit, urlunsplit

import breenfs_replay
import metal_image
import metal_lib as m
import metal_persist
import metal_root
import gt710_upload
import uuid

MAGIC = b'BREENIX-SUITE-1\0'
HEADER = struct.Struct('<16sQQ512s512s')
MAGIC_2 = b'BREENIX-SUITE-2\0'
HEADER_2 = struct.Struct('<16sQQ16s512s512s')
UPLOAD_PREFIX = '/suite-disk/'


def record(facts):
    return metal_root.root_record(facts['installedBy'], facts['bytes'], facts['sha256AfterRecord'])


def config(action, size, expected, root, url=''):
    if action not in (1, 2) or size <= 512 or size % 512 or len(expected) != 512 or len(root) != 512:
        raise ValueError('invalid suite disk request')
    if not expected.startswith(metal_root.RECORD_MAGIC + b'root ') or not root.startswith(metal_root.RECORD_MAGIC + b'root '):
        raise ValueError('suite disk requests need recorded roots')
    return HEADER.pack(MAGIC, action, size, expected, root) + (url.encode('ascii') + b'\0' if url else b'')


def config_2(action, size, partition, expected, root, url=''):
    """Format 2: the test root partition by unique GUID. Staging needs no earlier record; a readback names the
    suite's root record, which must still be the partition's first sector."""
    if action not in (1, 2) or size <= 512 or size % 512 or len(expected) != 512 or len(root) != 512:
        raise ValueError('invalid suite disk request')
    if not root.startswith(metal_root.RECORD_MAGIC + b'root ') or (action == 2 and expected != root):
        raise ValueError('suite disk requests need recorded roots')
    guid = uuid.UUID(partition).bytes_le
    return HEADER_2.pack(MAGIC_2, action, size, guid, expected, root) + (url.encode('ascii') + b'\0' if url else b'')


def stage(directory, inputs, boot_id, commit, site, build, mode='console', suite=None, idle_minutes=None, replug=False,
          the_layout=None, operator=None, no_ssh=False, screen_proof=False, screen_known=None, case=None, root_fs='ext2', tools_dir=None):
    """Reset the writable root for each boot, including unchanged userspace builds. `the_layout` is the layout an
    install boot's request writes before the stub stages this root; otherwise the recorded one, if any."""
    the_layout = the_layout or metal_persist.layout()
    previous = metal_persist.test_root()
    if not the_layout and not previous:
        raise ValueError('disk delivery needs a previously verified install to identify its owned disk')
    writer = Path(__file__).with_name('write-boot-target.sh')
    if not writer.is_file():
        writer = Path(__file__).resolve().parents[1] / 'write-boot-target.sh'
    identity = dict(ext2=build['ext2']['sha256'], bootUrl=site.get('BOOT_URL'), console=metal_root.console_identity(site),
                    site={k: v for k, v in site.items() if k.startswith('GUEST_')},
                    writer=metal_image.sha256(writer))
    if root_fs != 'ext2':
        identity['rootFs'] = root_fs
    key = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    image = inputs / 'ext2.img'
    # A reused mutable root can preserve the exact bytes diskChecks must check
    # this boot wrote, as well as consume fixtures needed by other suites.
    native_suite = mode == 'suite' and bool({'console', 'gpu', 'breengel', 'window-manager', 'ui-testing', 'gui-apps'} & set((suite or '').split(',')))
    upload = gt710_upload.prepare(directory, boot_id, site, suite=suite) if mode == 'probe' or native_suite else None
    def remove_ssh():
        # Damage only the candidate's private userland before recording its digest.
        # Its kernel, init and console still boot; the previous generation is untouched.
        import subprocess
        debugfs, = metal_root.require_e2fsprogs('debugfs')
        subprocess.run([debugfs, '-w', '-R', 'rm /usr/sbin/bsshd', str(image)], check=True, capture_output=True)
        check = subprocess.run([debugfs, '-R', 'stat /usr/sbin/bsshd', str(image)], check=True, capture_output=True)
        if b'Inode:' in check.stdout or b'File not found' not in check.stderr:
            raise ValueError('SSH fault did not remove the candidate daemon')
    if no_ssh and root_fs == 'breenfs':
        remove_ssh()
    staged = metal_root.stage_root(image, mode, suite, site, hasher=inputs / 'sha512-crypt', writer=writer,
                                   idle_minutes=idle_minutes, replug=replug, operator=operator, power_token=m.power_token(),gt710_upload=upload,
                                   screen_proof=screen_proof, screen_known=screen_known, case=case,
                                   **({'root_fs': root_fs, 'tools_dir': tools_dir} if root_fs != 'ext2' else {}))
    if no_ssh and root_fs != 'breenfs':
        remove_ssh()
    facts = metal_root.write_install_record(image, boot_id)
    for field in ('extentCount', 'breenfsSha256'):
        if field in staged:
            facts[field] = staged[field]
    if 'consoleAccount' in staged:
        facts['consoleAccount'] = staged['consoleAccount']
    root = record(facts)
    with image.open('r+b') as handle:
        handle.write(root)
    request = inputs / 'suite-disk.cfg'
    if the_layout:
        partition = the_layout['partitions']['test']['guid']
        request.write_bytes(config_2(1, facts['bytes'], partition, bytes(512), root))
        facts['partition'] = partition
    else:
        request.write_bytes(config(1, facts['bytes'], record(previous), root))
    facts.update(commit=commit, buildDigest=key)
    files = {'suite-disk.cfg': request, 'suite-root.img': image}
    m.write_json(directory / 'suite-disk.json', facts)
    return facts, files


def readback_checks(directory, suite):
    return m.read_json(directory / 'definition/docs/suites' / (suite + '.json'), {}).get('diskChecks', [])


def verify_owned_root(source_id, facts):
    current = metal_persist.test_root() or {}
    if (current.get('lastBoot') != source_id or current.get('partition') != facts.get('partition')
            or record(current) != record(facts)):
        raise ValueError('the owned disk was replaced between the writer and the next boot')


def require_test_partition(facts):
    layout = metal_persist.layout()
    if not layout or facts.get('partition') != layout['partitions']['test']['guid']:
        raise ValueError('power-cut evidence needs the recorded disposable test partition')


def prepare_cut_recovery(source_id, readback_id, commit):
    source = m.boot_dir(source_id)
    facts = m.read_json(source / 'suite-disk.json')
    require_test_partition(facts)
    verify_owned_root(source_id, facts)
    result = m.read_json(source / 'cut-readback.json', {})
    if ((source / 'commit').read_text().strip() != commit
            or result.get('id') != readback_id or result.get('passed') is not True):
        raise ValueError('no-restage recovery needs a successful cold replay of this cut')
    return facts, result


def prepare_readback(directory, stage, source_id, commit, boot_id, site, stub):
    source = m.boot_dir(source_id)
    run = m.read_json(source / 'run.json', {})
    facts = m.read_json(source / 'suite-disk.json')
    events = [json.loads(line) for line in (source / 'events.jsonl').read_text().splitlines()]
    off = next((e for e in reversed(events) if e['event'] == 'power-off'), {})
    decided = next((e for e in events if e['event'] == 'decided'), {})
    pending = next((e for e in events if e['event'] == 'suite-awaiting-disk-readback'), {})
    # Older manual retry sources recorded an awaiting event only for a finished guest.
    provisional = pending.get('provisional', True if pending else None)
    names = run.get('suites') or [run.get('suite')]
    common = (facts and run.get('disk') == 'root' and run.get('mode') == 'suite'
              and (source / 'ended').exists() and off.get('off') is True
              and (source / 'commit').read_text().strip() == commit
              and not any(event['event'] == 'console-gap-start' for event in events)
              and any(event['event'] == 'disk-root-record' and event.get('matchesLastInstall') is True
                      for event in events))
    power_cut = bool(run.get('powerCut'))
    bit_flip = run.get('breenfsBitFlip') is True
    if not common:
        raise ValueError('raw readback needs a completed disk suite boot with confirmed power-off and root identity')
    status = int((source / 'exit-status').read_text().strip())
    ended_by = (source / 'ended-by').read_text().strip()
    if power_cut:
        require_test_partition(facts)
        serial = (source / 'serial.txt').read_bytes()
        nonce, _ = breenfs_replay.receipts(serial)
        cut = breenfs_replay.cut_evidence(source, nonce)
        if window := cut.get('window'):
            breenfs_replay.verify_aimed_window(serial, cut, window)
        provisional = False
        if (facts.get('installedBy') != source_id or status != m.EXIT_SHORT or ended_by != 'driver'
                or decided.get('status') != m.EXIT_SHORT or decided.get('reason') != 'deliberate power cut'):
            raise ValueError('raw readback needs the completed deliberate cut of this root')
    elif bit_flip:
        require_test_partition(facts)
        if (run.get('id') != source_id or facts.get('installedBy') != source_id
                or run.get('payload') != 'image' or names != ['filesystem']
                or run.get('rootFs') != 'breenfs' or run.get('onlyCase') != 'power-cut/writer'
                or not any(event['event'] == 'disk-root-record' and event.get('matchesLastInstall') is True
                           and event.get('installedBy') == source_id for event in events)
                or status != m.EXIT_SHORT or ended_by != 'driver' or provisional is not True
                or decided.get('status') != m.EXIT_DONE):
            raise ValueError('raw readback needs the completed bit-flip probe on this root')
    elif (run.get('onlyCase') or not names[-1] or not readback_checks(source, names[-1])
            or status != m.EXIT_SHORT or ended_by != 'driver' or type(provisional) is not bool
            or decided.get('status') != (m.EXIT_DONE if provisional else m.EXIT_SHORT)):
        raise ValueError('raw readback needs a provisional, failed or deadline-ended suite source')
    verify_owned_root(source_id, facts)
    url = urlsplit(site['BOOT_URL'])
    route = UPLOAD_PREFIX + secrets.token_hex(32)
    request = stage / 'suite-disk.cfg'
    target = urlunsplit((url.scheme, url.netloc, route, '', ''))
    if facts.get('partition'):
        request.write_bytes(config_2(2, facts['bytes'], facts['partition'], record(facts), record(facts), target))
    else:
        request.write_bytes(config(2, facts['bytes'], record(facts), record(facts), target))
    image = stage / 'readback-disk.img'
    metal_image.assemble(image, {'suite-disk.cfg': request})
    output = stage / 'boot.efi'
    payload = metal_image.stub_efi(stub, image, output)
    image.unlink()
    m.write_json(directory / 'readback.json', dict(source=source_id, bytes=facts['bytes'], rootRecord=record(facts).hex(),
        suitePoweredOffAt=off['t'], sourceStatus=status, sourceProvisional=provisional, powerCut=power_cut, bitFlip=bit_flip,
        sourceReason=decided['reason'], sourceEndedBy=ended_by))
    m.write_json(m.STATE / 'disk-upload.json', dict(id=boot_id, route=route, bytes=facts['bytes'],
                                                 rootRecord=record(facts).hex()))
    return payload, output


def receive(handler):
    """Bounded streaming upload, bound to the active readback lease and exact cold disk record."""
    upload = m.read_json(m.STATE / 'disk-upload.json', {})
    lease = m.read_lease() or {}
    if (not upload or urlsplit(handler.path).path != upload.get('route') or lease.get('id') != upload.get('id')
            or lease.get('state') != 'powered' or upload.get('complete')):
        handler.send_error(404)
        return
    try:
        if handler.headers.get('Transfer-Encoding') or int(handler.headers.get('Content-Length', '-1')) != upload['bytes']:
            raise ValueError('raw disk upload length differs')
        handler.connection.settimeout(60)
        directory = m.boot_dir(upload['id'])
        temporary = directory / 'disk.img.partial'
        digest = hashlib.sha256()
        remaining = upload['bytes']
        with temporary.open('wb') as out:
            first = handler.rfile.read(512)
            if first.hex() != upload['rootRecord']:
                raise ValueError('raw disk record differs')
            out.write(first)
            digest.update(first)
            remaining -= len(first)
            while remaining:
                chunk = handler.rfile.read(min(1 << 16, remaining))
                if not chunk:
                    raise ValueError('short raw disk upload')
                out.write(chunk)
                digest.update(chunk)
                remaining -= len(chunk)
            out.flush()
            os.fsync(out.fileno())
        temporary.replace(directory / 'disk.img')
        upload.update(complete=True, sha256=digest.hexdigest(), receivedAt=m.now())
        m.write_json(directory / 'disk-upload.json', upload)
        m.write_json(m.STATE / 'disk-upload.json', upload)
        handler.send_response(200)
        handler.send_header('Content-Length', '0')
        handler.end_headers()
    except (OSError, ValueError):
        handler.close_connection = True
        handler.send_error(400, 'raw disk upload failed')
