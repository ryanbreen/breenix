"""Suite fixture delivery and cold raw readback through the firmware, without mounting the readback disk."""
import hashlib
import json
import os
from pathlib import Path
import secrets
import struct
from urllib.parse import urlsplit, urlunsplit

import metal_image
import metal_lib as m
import metal_root

MAGIC = b'BREENIX-SUITE-1\0'
HEADER = struct.Struct('<16sQQ512s512s')
UPLOAD_PREFIX = '/suite-disk/'


def record(facts):
    return metal_root.root_record(facts['installedBy'], facts['bytes'], facts['sha256AfterRecord'])


def config(action, size, expected, root, url=''):
    if action not in (1, 2) or size <= 512 or size % 512 or len(expected) != 512 or len(root) != 512:
        raise ValueError('invalid suite disk request')
    if not expected.startswith(metal_root.RECORD_MAGIC + b'root ') or not root.startswith(metal_root.RECORD_MAGIC + b'root '):
        raise ValueError('suite disk requests need recorded roots')
    return HEADER.pack(MAGIC, action, size, expected, root) + (url.encode('ascii') + b'\0' if url else b'')


def stage(directory, inputs, boot_id, commit, site, build):
    """Install only when the userspace build or private site configuration changes."""
    previous = m.read_json(m.STATE / 'disk.json')
    if not previous:
        raise ValueError('disk delivery needs a previously verified install to identify its owned disk')
    writer = Path(__file__).with_name('write-boot-target.sh')
    if not writer.is_file():
        writer = Path(__file__).resolve().parents[1] / 'write-boot-target.sh'
    identity = dict(ext2=build['ext2']['sha256'], bootUrl=site.get('BOOT_URL'), console=metal_root.console_identity(site),
                    site={k: v for k, v in site.items() if k.startswith('GUEST_')},
                    writer=metal_image.sha256(writer))
    key = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    if previous.get('buildDigest') == key:
        facts = dict(previous)
        files = {}
    else:
        image = inputs / 'ext2.img'
        # Install one reusable root; per-boot mode selection travels separately.
        metal_root.stage_root(image, 'console', None, site, hasher=inputs / 'sha512-crypt', writer=writer)
        facts = metal_root.write_install_record(image, boot_id)
        root = record(facts)
        with image.open('r+b') as handle:
            handle.write(root)
        request = inputs / 'suite-disk.cfg'
        request.write_bytes(config(1, facts['bytes'], record(previous), root))
        facts.update(commit=commit, buildDigest=key)
        files = {'suite-disk.cfg': request, 'suite-root.img': image}
    m.write_json(directory / 'suite-disk.json', facts)
    return facts, files


def prepare_readback(directory, stage, source_id, commit, boot_id, site, stub):
    source = m.boot_dir(source_id)
    run = m.read_json(source / 'run.json', {})
    facts = m.read_json(source / 'suite-disk.json')
    events = [json.loads(line) for line in (source / 'events.jsonl').read_text().splitlines()]
    off = [event for event in events if event['event'] == 'power-off' and event.get('off')]
    if (not facts or run.get('disk') != 'root' or run.get('mode') != 'suite' or not (source / 'ended').exists() or not off
            or (source / 'commit').read_text().strip() != commit
            or any(event['event'] == 'console-gap-start' for event in events)
            or not any(event['event'] == 'decided' and event.get('status') == m.EXIT_DONE for event in events)
            or not any(event['event'] == 'disk-root-record' and event.get('matchesLastInstall') for event in events)):
        raise ValueError('raw readback needs a completed disk suite boot with confirmed power-off and root identity')
    # Another disk user may have acquired the lease between the boots. Refuse a changed disk association.
    current = m.read_json(m.STATE / 'disk.json', {})
    if record(current) != record(facts) or current.get('lastBoot') != source_id:
        raise ValueError('the owned disk was replaced between the suite and its cold readback')
    url = urlsplit(site['BOOT_URL'])
    route = UPLOAD_PREFIX + secrets.token_hex(32)
    request = stage / 'suite-disk.cfg'
    request.write_bytes(config(2, facts['bytes'], record(facts), record(facts),
                              urlunsplit((url.scheme, url.netloc, route, '', ''))))
    image = stage / 'readback-disk.img'
    metal_image.assemble(image, {'suite-disk.cfg': request})
    output = stage / 'boot.efi'
    payload = metal_image.stub_efi(stub, image, output)
    image.unlink()
    m.write_json(directory / 'readback.json', dict(source=source_id, bytes=facts['bytes'], rootRecord=record(facts).hex(),
                                                 suitePoweredOffAt=off[-1]['t']))
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
