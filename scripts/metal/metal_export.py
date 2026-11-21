"""Lease-bound reception of the persistent partition export (firmware action 3)."""
import hashlib
import os
import re
import secrets
import shutil
import stat
import struct
from urllib.parse import urlsplit, urlunsplit
import uuid

import metal_image
import metal_lib as m
import metal_persist

PAYLOAD = 'persistent-export'
PREFIX = '/persistent-export/'
ROLES = ('system', 'home')
RECEIPT = re.compile(r'metal-stub: persistent export (system|home) bytes=([0-9]+) sha256=([0-9a-f]{64})')
DONE = 'metal-stub: persistent export complete'
HEADER = struct.Struct('<16sQ')
RANGE = struct.Struct('<16sQQ')


def pool():
    return m.STATE / 'persistent-exports'


def pending(boot_id):
    return pool() / (boot_id + '.partial')


def active(boot_id, state):
    lease = m.read_lease() or {}
    return (lease.get('id') == boot_id and lease.get('state') == state and lease.get('payload') == PAYLOAD)


def prepare(stage, boot_id, site, stub):
    """Stage a firmware-only request from the last install's layout, under the driver lease."""
    if not active(boot_id, 'claimed'):
        raise ValueError('persistent export needs its own machine lease')
    layout = metal_persist.layout()
    if not layout:
        raise ValueError('persistent export needs a recorded persistent install')
    parts = layout['partitions']
    guids = [uuid.UUID(part['guid']) for part in parts.values()]
    if len(set(guids)) != len(guids) or any(guid.int == 0 for guid in guids):
        raise ValueError('persistent export requires distinct nonzero partition GUIDs')
    url = urlsplit(site['BOOT_URL'])
    if url.scheme != 'http' or not url.hostname or url.username is not None:
        raise ValueError('persistent export needs the firmware HTTP endpoint')

    token = secrets.token_hex(32)
    rows = []
    for role in ROLES:
        part = parts[role]
        rows.append(dict(role=role, guid=part['guid'], firstLba=part['first'], lastLba=part['last'],
                         bytes=(part['last'] - part['first'] + 1) * metal_persist.SECTOR,
                         route=f"{PREFIX}{token}/{part['guid']}"))
    wire = HEADER.pack(b'BREENIX-EXPORT1\0', 3)
    for role in ('test', *ROLES):
        part = parts[role]
        wire += RANGE.pack(uuid.UUID(part['guid']).bytes_le, part['first'], part['last'])
    for row in rows:
        target = urlunsplit((url.scheme, url.netloc, row['route'], '', '')).encode('ascii')
        if len(target) > 2046:
            raise ValueError('the persistent export URL is too long for firmware')
        wire += target + b'\0'

    root = pool()
    root.mkdir(mode=0o700, exist_ok=True)
    info = root.lstat()
    if info.st_uid != os.geteuid() or info.st_mode != (stat.S_IFDIR | 0o700):
        raise ValueError('persistent export needs a private boot-host pool directory')
    if shutil.disk_usage(root).free < sum(row['bytes'] for row in rows):
        raise ValueError('the boot-host pool has insufficient space for the persistent export')
    if (root / boot_id).exists():
        raise ValueError('this persistent export was already published')
    private = pending(boot_id)
    private.mkdir(mode=0o700)
    m.write_json(private / 'request.json', rows)

    request = stage / 'export.cfg'
    request.write_bytes(wire)
    image = stage / 'export-disk.img'
    metal_image.assemble(image, {'export.cfg': request})
    output = stage / 'boot.efi'
    facts = metal_image.stub_efi(stub, image, output)
    image.unlink()
    request.unlink()
    return facts, output


def receive(handler):
    """Accept one bounded HTTP PUT per recorded GUID while this lease is powered."""
    handler.connection.settimeout(60)
    lease = m.read_lease() or {}
    boot_id = lease.get('id', '')
    route = urlsplit(handler.path).path
    with m.locked(PAYLOAD):
        rows = m.read_json(pending(boot_id) / 'request.json', []) if active(boot_id, 'powered') else []
        row = next((row for row in rows if route.isascii() and secrets.compare_digest(route, row['route'])), None)
        if row is None:
            handler.close_connection = True
            handler.send_error(404)
            return
        try:
            if handler.headers.get('Transfer-Encoding') or int(handler.headers.get('Content-Length', '-1')) != row['bytes']:
                raise ValueError('persistent export upload length differs')
            private = pending(boot_id)
            image = private / (row['role'] + '.img')
            fd = os.open(image, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        except (OSError, ValueError):
            handler.close_connection = True
            handler.send_error(400, 'persistent export upload refused')
            return

    digest = hashlib.sha256()
    count = 0
    try:
        with os.fdopen(fd, 'wb') as out:
            while count < row['bytes']:
                chunk = handler.rfile.read(min(1 << 20, row['bytes'] - count))
                if not chunk:
                    raise ValueError('short persistent export upload')
                out.write(chunk)
                digest.update(chunk)
                count += len(chunk)
            out.flush()
            os.fsync(out.fileno())
        with m.locked(PAYLOAD):
            if not active(boot_id, 'powered') or not (private / 'request.json').is_file():
                raise ValueError('persistent export lease expired')
            m.write_json(private / (row['role'] + '.json'), dict(bytes=count, sha256=digest.hexdigest()))
    except (OSError, ValueError):
        with m.locked(PAYLOAD):
            image.unlink(missing_ok=True)
        handler.close_connection = True
        handler.send_error(400, 'persistent export upload failed')
        return
    handler.send_response(200)
    handler.send_header('Content-Length', '0')
    handler.end_headers()


def verify(boot_id, firmware):
    private = pending(boot_id)
    rows = m.read_json(private / 'request.json', [])
    if tuple(row['role'] for row in rows) != ROLES or set(firmware) != set(ROLES):
        raise ValueError('persistent export needs both partition receipts')
    records = []
    for row in rows:
        role = row['role']
        received = m.read_json(private / (role + '.json'), {})
        firmware_bytes, firmware_digest = firmware[role]
        if (received.get('bytes') != row['bytes'] or firmware_bytes != row['bytes']
                or (private / (role + '.img')).stat().st_size != row['bytes']):
            raise ValueError(f'persistent export {role} byte count differs')
        if firmware_digest != received['sha256']:
            raise ValueError(f'persistent export {role} digest differs')
        records.append(dict(role=role, guid=row['guid'], firstLba=row['firstLba'], lastLba=row['lastLba'],
                            hostBytes=received['bytes'], firmwareBytes=firmware_bytes,
                            hostSha256=received['sha256'], firmwareSha256=firmware_digest))
    return records


def publish(directory, boot_id, firmware):
    """Publish the pair after confirmed power-off, then record the verified copy."""
    with m.locked(PAYLOAD):
        if not active(boot_id, 'powered'):
            raise ValueError('the persistent export lease was lost before publication')
        rows = verify(boot_id, firmware)
        run = m.read_json(directory / 'run.json', {})
        if run.get('id') != boot_id:
            raise ValueError('the persistent export boot record is missing')
        private = pending(boot_id)
        final = pool() / boot_id
        (private / 'request.json').unlink()
        for role in ROLES:
            (private / (role + '.json')).unlink()
        private.rename(final)
        try:
            run['persistentExport'] = dict(partitions=rows)
            m.write_json(directory / 'run.json', run)
        except (OSError, ValueError):
            shutil.rmtree(final)
            raise


def discard(boot_id):
    with m.locked(PAYLOAD):
        directory = pending(boot_id)
        if directory.is_dir():
            shutil.rmtree(directory)
