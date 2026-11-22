"""The disposable QEMU disk's 512-byte GPT (UEFI 2.10 section 5.3)."""
from contextlib import contextmanager
import hashlib
import json
import os
import stat
import struct
import sys
import tempfile
import uuid
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).with_name('metal')))
from metal_persist import BREENFS_ROLES, EXT2_ROLES, FIRST_LBA, NAMES, ROLES, TYPES

SECTOR = 512
ENTRY = 128
ARRAY = 128 * ENTRY
TABLE_SECTORS = ARRAY // SECTOR
IMAGE = 'retirement.img'


def tables(layout, retired):
    roles = ROLES + BREENFS_ROLES
    if set(layout['parts']) != set(roles):
        raise ValueError('GPT does not describe the six migrated entries')
    entries = bytearray(ARRAY)
    for index, role in enumerate(roles):
        if retired and role in EXT2_ROLES:
            continue
        guid, first, count = layout['parts'][role]
        struct.pack_into('<16s16sQQQ72s', entries, index * ENTRY,
                         uuid.UUID(TYPES[role]).bytes_le, uuid.UUID(guid).bytes_le,
                         first, first + count - 1, 0, NAMES[role].encode('utf-16le'))
    last = layout['sectors'] - 1

    def copy(at, other, table):
        header = bytearray(SECTOR)
        struct.pack_into('<8sIIIIQQQQ16sQIII', header, 0, b'EFI PART', 0x10000, 92, 0, 0,
                         at, other, TABLE_SECTORS + 2, last - TABLE_SECTORS - 1,
                         uuid.UUID(layout['diskGuid']).bytes_le, table, 128, ENTRY, zlib.crc32(entries))
        struct.pack_into('<I', header, 16, zlib.crc32(header[:92]))
        return (table * SECTOR, entries), (at * SECTOR, header)

    # Flush the new backup before changing the primary.
    return copy(last, 1, last - TABLE_SECTORS), copy(1, last, 2)


def protective_mbr(sectors):
    mbr = bytearray(SECTOR)
    struct.pack_into('<B3sB3sII', mbr, 446, 0, b'\x00\x02\x00', 0xee, b'\xff' * 3,
                     1, min(sectors - 1, 0xffffffff))
    mbr[510:] = b'\x55\xaa'
    return mbr


def write_copies(disk, copies):
    for copy in copies:
        for offset, data in copy:
            disk.seek(offset)
            if disk.write(data) != len(data):
                raise ValueError('short GPT write')
        os.fsync(disk.fileno())


def readback(disk, layout, retired):
    copies = tables(layout, retired)
    checksum = hashlib.sha256()
    for offset, data in [(0, protective_mbr(layout['sectors'])),
                         *(piece for copy in copies for piece in copy)]:
        disk.seek(offset)
        actual = disk.read(len(data))
        if actual != data:
            raise ValueError('GPT copies differ from the sealed layout (headers, CRCs or entries)')
        checksum.update(actual)
    return checksum.hexdigest()


def create(run, guids):
    roles = ROLES + BREENFS_ROLES
    paths = {role: run / ('test-root.img' if role == 'test' else role + '.img') for role in roles[1:]}
    ids = {**guids, 'test': guids['test-root'], 'esp': str(uuid.uuid4())}
    if len(set(ids[role] for role in roles)) != len(roles):
        raise ValueError('duplicate GPT partition GUID')
    parts = {}
    first = FIRST_LBA
    for role in roles:
        # This VM uses fw_cfg and host slots. Keep position 1 for the ESP.
        size = paths[role].stat().st_size if role in paths else FIRST_LBA * SECTOR
        if size <= 0 or size % SECTOR:
            raise ValueError('GPT volume is not a positive whole number of sectors')
        count = size // SECTOR
        parts[role] = [ids[role], first, count]
        first = ((first + count + FIRST_LBA - 1) // FIRST_LBA) * FIRST_LBA
    layout = dict(parts=parts, diskGuid=str(uuid.uuid4()), sectors=first + TABLE_SECTORS + 1)
    with tempfile.NamedTemporaryFile(dir=run, prefix='retirement-', buffering=0) as disk:
        disk.truncate(layout['sectors'] * SECTOR)
        for role, path in paths.items():
            _, start, count = parts[role]
            position = start * SECTOR
            with path.open('rb') as source:
                for chunk in iter(lambda: source.read(1 << 20), b''):
                    if position + len(chunk) > (start + count) * SECTOR:
                        raise ValueError('GPT volume changed size while packing')
                    if chunk.count(0) != len(chunk):
                        disk.seek(position)
                        if disk.write(chunk) != len(chunk):
                            raise ValueError('short GPT volume write')
                    position += len(chunk)
            if position != (start + count) * SECTOR:
                raise ValueError('GPT volume changed size while packing')
        disk.seek(0)
        disk.write(protective_mbr(layout['sectors']))
        write_copies(disk, tables(layout, False))
        readback(disk, layout, False)
        info = os.fstat(disk.fileno())
        layout['identity'] = [info.st_dev, info.st_ino]
        try:
            os.link(disk.name, run / IMAGE)
        except FileExistsError:
            raise ValueError('disposable GPT disk already exists') from None
    return layout


@contextmanager
def open_disk(run, layout, write=False):
    flags = os.O_RDWR if write else os.O_RDONLY
    fd = os.open(run / IMAGE, flags | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'r+b' if write else 'rb', buffering=0) as disk:
        info = os.fstat(disk.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1
                or [info.st_dev, info.st_ino] != layout['identity']
                or info.st_size != layout['sectors'] * SECTOR):
            raise ValueError('GPT disk is not the disposable image built for this run')
        yield disk


def drive_args(run, layout, install):
    # The raw nodes expose only the sealed partition ranges. The run lock
    # excludes other launches or GPT edits sharing this file.
    roles = ['breenfs-home', 'breenfs-system'] + (['test'] if install else [])
    for role in roles:
        _, first, count = layout['parts'][role]
        read_only = role == 'test'
        node = dict(driver='raw', offset=first * SECTOR, size=count * SECTOR,
                    file=dict(driver='file', filename=str(run / IMAGE), locking='off'),
                    **{'node-name': role, 'read-only': read_only})
        yield from ('-blockdev', json.dumps(node), '-device', f'virtio-blk-device,drive={role}')
