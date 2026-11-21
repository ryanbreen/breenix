"""Lease-bound reception of the persistent partition export (firmware action 3)."""
import contextlib
import fcntl
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
PART_BYTES = 1 << 30
RECEIPT = re.compile(r'metal-stub: persistent export (system|home)(?: part=([0-9]+))? '
                     r'bytes=([0-9]+) sha256=([0-9a-f]{64})')
DONE = 'metal-stub: persistent export complete'
SPARSE_DONE = 'metal-stub: sparse export complete'
SPARSE_RECEIPT = re.compile(r'metal-stub: sparse export (breenfs-system|breenfs-home) '
                            r'bytes=([0-9]+) sha256=([0-9a-f]{64})')
HEADER = struct.Struct('<16sQ')
RANGE = struct.Struct('<16sQQ')


def pool():
    return m.STATE / 'persistent-exports'


def pending(boot_id):
    return pool() / (boot_id + '.partial')


def active(boot_id, state):
    lease = m.read_lease() or {}
    return (lease.get('id') == boot_id and lease.get('state') == state and lease.get('payload') == PAYLOAD)


def prepare(stage, boot_id, site, stub, *, sparse=False):
    """Stage a firmware-only request from the last install's layout, under the driver lease."""
    if not active(boot_id, 'claimed'):
        raise ValueError('persistent export needs its own machine lease')
    layout = metal_persist.layout()
    if not layout:
        raise ValueError('persistent export needs a recorded persistent install')
    if sparse and layout['version'] != 2:
        raise ValueError('sparse readback requires the recorded BreenFS layout')
    if not sparse and layout['version'] == 3:
        raise ValueError('the ext2 partitions were retired from the recorded layout')
    roles = metal_persist.BREENFS_ROLES if sparse else ROLES
    parts = layout['partitions']
    guids = [uuid.UUID(part['guid']) for part in parts.values()]
    if len(set(guids)) != len(guids) or any(guid.int == 0 for guid in guids):
        raise ValueError('persistent export requires distinct nonzero partition GUIDs')
    url = urlsplit(site['BOOT_URL'])
    if url.scheme != 'http' or not url.hostname or url.username is not None:
        raise ValueError('persistent export needs the firmware HTTP endpoint')

    token = secrets.token_hex(32)
    rows = []
    for role in roles:
        part = parts[role]
        rows.append(dict(role=role, guid=part['guid'], firstLba=part['first'], lastLba=part['last'],
                         bytes=(part['last'] - part['first'] + 1) * metal_persist.SECTOR,
                         route=f"{PREFIX}{token}/{part['guid']}"))
    if sparse:
        for row in rows:
            row['sparse'] = True
    wire = HEADER.pack(b'BREENIX-EXPORT2\0', 4 if sparse else 3)
    for role in ('test', *roles):
        part = parts[role]
        wire += RANGE.pack(uuid.UUID(part['guid']).bytes_le, part['first'], part['last'])
    for row in rows:
        target = urlunsplit((url.scheme, url.netloc, row['route'] + ('/sparse' if sparse else ''), '', '')).encode('ascii')
        if len(target) + len('/0000000000000000') > 2046:
            raise ValueError('the persistent export URL is too long for firmware')
        wire += target + b'\0'

    root = pool()
    root.mkdir(mode=0o700, exist_ok=True)
    info = root.lstat()
    if info.st_uid != os.geteuid() or info.st_mode != (stat.S_IFDIR | 0o700):
        raise ValueError('persistent export needs a private boot-host pool directory')
    if not sparse and shutil.disk_usage(root).free < sum(row['bytes'] for row in rows):
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
    """Append the next part of a recorded partition while this lease is powered."""
    handler.connection.settimeout(60)
    lease = m.read_lease() or {}
    boot_id = lease.get('id', '')
    route, _, suffix = urlsplit(handler.path).path.rpartition('/')
    if suffix == 'sparse':
        return receive_sparse(handler, boot_id, route)
    with m.locked(PAYLOAD):
        rows = m.read_json(pending(boot_id) / 'request.json', []) if active(boot_id, 'powered') else []
        row = next((row for row in rows if not row.get('sparse') and route.isascii() and secrets.compare_digest(route, row['route'])), None)
        if row is None:
            handler.close_connection = True
            handler.send_error(404)
            return
        private = pending(boot_id)
        image = private / (row['role'] + '.img')
        receipts = private / (row['role'] + '.json')
        received = m.read_json(receipts, [])
        index = len(received)
        offset = index * PART_BYTES
        size = min(PART_BYTES, row['bytes'] - offset)
        try:
            if (suffix != f'{index:016x}' or size <= 0 or handler.headers.get('Transfer-Encoding')
                    or int(handler.headers.get('Content-Length', '-1')) != size):
                raise ValueError('persistent export part order or length differs')
            flags = os.O_WRONLY | (os.O_CREAT | os.O_EXCL if index == 0 else 0)
            out = os.fdopen(os.open(image, flags, 0o600), 'wb', buffering=0)
            try:
                # Protect this image across concurrent requests without holding the pool lock over a network read.
                fcntl.flock(out.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                if out.seek(0, os.SEEK_END) != offset:
                    raise ValueError('persistent export image length differs from received parts')
            except (OSError, ValueError):
                out.close()
                raise
        except (OSError, ValueError):
            handler.close_connection = True
            handler.send_error(400, 'persistent export upload refused')
            return

    digest = hashlib.sha256()
    count = 0
    with out:
        try:
            while count < size:
                chunk = handler.rfile.read(min(1 << 20, size - count))
                if not chunk:
                    raise ValueError('short persistent export upload')
                if out.write(chunk) != len(chunk):
                    raise OSError('short write to persistent export image')
                digest.update(chunk)
                count += len(chunk)
            os.fsync(out.fileno())
            with m.locked(PAYLOAD):
                if not active(boot_id, 'powered') or not (private / 'request.json').is_file():
                    raise ValueError('persistent export lease expired')
                received.append(dict(bytes=count, sha256=digest.hexdigest()))
                m.write_json(receipts, received)
        except (OSError, ValueError):
            with m.locked(PAYLOAD), contextlib.suppress(OSError):
                if index == 0:
                    image.unlink(missing_ok=True)
                else:
                    os.ftruncate(out.fileno(), offset)
            handler.close_connection = True
            handler.send_error(400, 'persistent export upload failed')
            return
    handler.send_response(200)
    handler.send_header('Content-Length', '0')
    handler.end_headers()


def receive_sparse(handler, boot_id, route):
    from metal_migration import unpack_sparse

    private = pending(boot_id)
    with m.locked(PAYLOAD):
        rows = m.read_json(private / 'request.json', []) if active(boot_id, 'powered') else []
        row = next((row for row in rows if row.get('sparse') and route.isascii()
                    and secrets.compare_digest(route, row['route'])), None)
        if row is None:
            handler.close_connection = True
            handler.send_error(404)
            return
        path = private / (row['role'] + '.img')
        try:
            if handler.headers.get('Transfer-Encoding'):
                raise ValueError('sparse export needs a content length')
            count = int(handler.headers.get('Content-Length', '-1'))
            if count > shutil.disk_usage(private).free:
                raise ValueError('no room for the sparse export')
            out = os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), 'wb')
        except (OSError, ValueError):
            handler.close_connection = True
            handler.send_error(400, 'sparse upload refused')
            return
    try:
        with out:
            digest = unpack_sparse(handler.rfile, out, row['bytes'], count)
            out.flush()
            os.fsync(out.fileno())
        with m.locked(PAYLOAD):
            if not active(boot_id, 'powered') or not (private / 'request.json').is_file():
                raise ValueError('sparse export lease expired')
            m.write_json(private / (row['role'] + '.json'), dict(bytes=count, sha256=digest))
    except (OSError, ValueError):
        path.unlink(missing_ok=True)
        handler.close_connection = True
        handler.send_error(400, 'sparse upload failed')
        return
    handler.send_response(200)
    handler.send_header('Content-Length', '0')
    handler.end_headers()


def publish_sparse(directory, boot_id, firmware):
    with m.locked(PAYLOAD):
        if not active(boot_id, 'powered'):
            raise ValueError('sparse export lost its lease')
        private = pending(boot_id)
        rows = m.read_json(private / 'request.json', [])
        roles = metal_persist.BREENFS_ROLES
        if tuple(row['role'] for row in rows if row.get('sparse')) != roles or set(firmware) != set(roles):
            raise ValueError('sparse export requires a receipt for both BreenFS partitions')
        records = []
        for row in rows:
            role = row['role']
            host = m.read_json(private / (role + '.json'), {})
            if (host.get('bytes'), host.get('sha256')) != firmware[role]:
                raise ValueError(f'sparse export {role} count or digest differs')
            records.append({key: row[key] for key in ('role', 'guid', 'firstLba', 'lastLba', 'bytes')}
                           | dict(streamBytes=host['bytes'], streamSha256=host['sha256']))
        final = pool() / boot_id
        private.rename(final)
        (final / 'request.json').unlink()
        m.write_json(directory / 'sparse-readback.json', dict(partitions=records))
        return final


def verify(boot_id, firmware):
    private = pending(boot_id)
    rows = m.read_json(private / 'request.json', [])
    if tuple(row['role'] for row in rows) != ROLES:
        raise ValueError('persistent export request is missing its partition layout')
    expected = {(row['role'], index) for row in rows
                for index in range((row['bytes'] + PART_BYTES - 1) // PART_BYTES)}
    if set(firmware) != expected | {(role, None) for role in ROLES}:
        raise ValueError('persistent export needs each part and both partition receipts')
    records = []
    for row in rows:
        role = row['role']
        image = private / (role + '.img')
        received = m.read_json(private / (role + '.json'), [])
        offsets = range(0, row['bytes'], PART_BYTES)
        if len(received) != len(offsets):
            raise ValueError(f'persistent export {role} is missing host part receipts')
        parts = []
        for index, offset in enumerate(offsets):
            host = received[index]
            part_bytes, part_digest = firmware[role, index]
            size = min(PART_BYTES, row['bytes'] - offset)
            if host['bytes'] != size or part_bytes != size:
                raise ValueError(f'persistent export {role} part {index} byte count differs')
            if host['sha256'] != part_digest:
                raise ValueError(f'persistent export {role} part {index} digest differs')
            parts.append(dict(index=index, offset=offset, hostBytes=host['bytes'], firmwareBytes=part_bytes,
                              hostSha256=host['sha256'], firmwareSha256=part_digest))
        firmware_bytes, firmware_digest = firmware[role, None]
        host_bytes = image.stat().st_size
        if host_bytes != row['bytes'] or firmware_bytes != row['bytes']:
            raise ValueError(f'persistent export {role} byte count differs')
        host_digest = metal_image.sha256(image)
        if firmware_digest != host_digest:
            raise ValueError(f'persistent export {role} digest differs')
        records.append(dict(role=role, guid=row['guid'], firstLba=row['firstLba'], lastLba=row['lastLba'],
                            hostBytes=host_bytes, firmwareBytes=firmware_bytes,
                            hostSha256=host_digest, firmwareSha256=firmware_digest, parts=parts))
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
