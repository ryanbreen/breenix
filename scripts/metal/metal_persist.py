"""The persistent install's layout and the install requests the delivery stub carries out (docs/metal/README.md,
"Boot manager and persistent install").

The layout is a GUID partition table with four partitions: the EFI system partition (the boot manager and the
kernel slots), the test root, the system partition and the home partition. Their unique GUIDs, and the disk's, are
made here for the one-time install and kept in the service's `disk.json` (`layout`). A kernel slot is a boot disk in
the delivery stub's format whose `ramdisk` is the slot's format-2 boot intent. An install request (`persist.cfg`
and `persist.dat`) tells the stub what to write: for the one-time install the partition table and each partition's
contents as extents; for an upgrade the files to write into the EFI system partition.
"""
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from urllib.parse import urlsplit, urlunsplit
import uuid

import metal_image
import metal_lib as m

SECTOR = 512
FIRST_LBA = 2048
ROLES = ('esp', 'test', 'system', 'home')
TYPES = {'esp': 'c12a7328-f81f-11d2-ba4b-00a0c93ec93b', 'test': 'dac951c3-27df-4e3d-8b9e-487906ac2c81',
         'system': '2ef14a34-abbd-48f8-83d6-f2a8c7b4a9fe', 'home': '9bfeaef9-4f51-45de-8bcf-5cec1603b862'}
SIZES = {'esp': 512 << 20, 'test': 4 << 30, 'system': 8 << 30, 'home': 16 << 30}
NAMES = {'esp': 'EFI system partition', 'test': 'Breenix test root', 'system': 'Breenix system',
         'home': 'Breenix home'}
MANAGER_DIR = 'efi/breenix'
MANAGER_PATH = 'efi/breenix/bootmgr.efi'
LOAD_OPTION = 'Breenix boot manager'
REQUEST_MAGIC = 'BREENIX-PERSIST 1'
# ext2 for the system and home partitions: 4 KiB blocks, one inode per 64 KiB, no online-resize reservation.
EXT2_OPTIONS = ['-t', 'ext2', '-b', '4096', '-i', '65536', '-I', '256', '-O', '^resize_inode', '-E', 'nodiscard']
EXT2_BLOCK = 4096
FULL_CHUNK = 64 * 1024
GUID = re.compile(r'[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}')


def new_layout():
    """Random disk and partition GUIDs, the partitions in role order on 1 MiB boundaries from LBA 2048."""
    lba = FIRST_LBA
    partitions = {}
    for number, role in enumerate(ROLES, start=1):
        sectors = SIZES[role] // SECTOR
        partitions[role] = {'number': number, 'type': TYPES[role], 'guid': str(uuid.uuid4()), 'first': lba,
                            'last': lba + sectors - 1, 'name': NAMES[role]}
        lba += sectors
    return {'version': 1, 'diskGuid': str(uuid.uuid4()), 'partitions': partitions}


def check_layout(layout):
    """A layout this module made: four partitions in order, each on a 1 MiB boundary, with lowercase GUIDs."""
    parts = layout.get('partitions', {})
    if layout.get('version') != 1 or set(parts) != set(ROLES) or not GUID.fullmatch(layout.get('diskGuid', '')):
        raise ValueError('the recorded layout is not a persistent-install layout')
    end = FIRST_LBA - 1
    for number, role in enumerate(ROLES, start=1):
        part = parts[role]
        if (part['number'] != number or part['type'] != TYPES[role] or not GUID.fullmatch(part['guid'])
                or part['first'] <= end or part['first'] % 2048 or part['last'] - part['first'] + 1 != SIZES[role] // SECTOR):
            raise ValueError(f'the recorded layout\'s {role} partition is malformed')
        end = part['last']
    return layout


# The service's record of the disk: `layout` (this layout), `testRoot` (the test root's last staged image, as the
# whole-disk record was), `slots` (the kernel slots' commits) and `definition` (the persistent kernel's definition
# files' folder in the store).

def read_disk():
    return m.read_json(m.STATE / 'disk.json', {}) or {}


def layout():
    """The disk's persistent layout, or none when the disk still has the whole-disk root."""
    record = read_disk()
    return check_layout(record['layout']) if record.get('layout') else None


def test_root():
    """The last staged root's facts: the test root's on the persistent layout, the whole disk's before it."""
    record = read_disk()
    return record.get('testRoot') if record.get('layout') else (record or None)


def update_disk(**changes):
    """Merges changes into disk.json under the lock, so the driver and the reaper never lose each other's fields."""
    with m.locked('disk'):
        record = read_disk()
        if not record.get('layout') and changes.get('layout'):
            # The whole-disk root facts become the history of the disk before its layout.
            record = {'wholeDisk': record} if record else {}
        record.update(changes)
        m.write_json(m.STATE / 'disk.json', record)
        return record


def record_test_root(facts):
    """Records the staged root's facts: beside the layout on the persistent layout, as the whole disk's before it."""
    if layout():
        update_disk(testRoot=facts)
    else:
        m.write_json(m.STATE / 'disk.json', facts)


def query_url(site):
    """The boot manager's question: `manager` beside the boot URL (scripts/metal/metal-http)."""
    url = urlsplit(site['BOOT_URL'])
    return urlunsplit((url.scheme, url.netloc, url.path.rsplit('/', 1)[0] + '/manager', '', ''))


def manager_config(site, countdown=5):
    """MANAGER.CFG's lines from the boot host; the stub adds the HTTP boot port and option it was loaded through."""
    return ['BREENIX-MANAGER 1', 'query ' + query_url(site), f'countdown {countdown}', 'test-path load-file']


def intent(target, root=None, home=None, tests=None):
    """A format-2 boot intent (kernel/src/boot/intent.rs)."""
    lines = ['BREENIX-BOOT 2', 'target ' + target]
    for item, guid in (('root', root), ('home', home)):
        if guid is not None:
            if not GUID.fullmatch(guid):
                raise ValueError(f'the {item} is not a GUID')
            lines.append(f'{item} {guid}')
    if tests is not None:
        lines.append(f'tests {tests[0]} {tests[1]}')
    return ('\n'.join(lines) + '\n').encode('ascii')


def tests_ramdisk(output, target, root, binaries):
    """A tests boot's RAM disk: its intent, padded to 4 KiB, then the BXTEST image."""
    size = Path(binaries).stat().st_size
    if size % SECTOR:
        raise ValueError('the test-binary disk is not whole sectors')
    head = intent(target, root=root, tests=(4096, size))
    if len(head) > 4096:
        raise ValueError('the tests intent does not fit its first 4 KiB')
    with open(output, 'wb') as out, open(binaries, 'rb') as source:
        out.write(head + bytes(4096 - len(head)))
        shutil.copyfileobj(source, out, 1 << 20)
    return output


def slot_image(output, esp_files, the_layout, scratch):
    """A kernel slot: the build's bootloader, kernel and boot.json, and the persistent boot's intent as `ramdisk`."""
    parts = the_layout['partitions']
    ramdisk = Path(scratch) / 'slot-intent'
    ramdisk.write_bytes(intent('persistent', root=parts['system']['guid'], home=parts['home']['guid']))
    files = {name: esp_files[name] for name in metal_image.ESP_FILES}
    files['ramdisk'] = ramdisk
    return metal_image.assemble(output, files)


def slot_note(commit, boot_id):
    return f'commit {commit}\ninstalled-by {boot_id}\n'


def sparse_file(path, size):
    with open(path, 'wb') as handle:
        handle.truncate(size)


def ext2_image(path, size, label):
    """An empty ext2 filesystem of `size` bytes in a sparse file."""
    mke2fs, = metal_root_tools('mke2fs')
    sparse_file(path, size)
    subprocess.run([mke2fs, *EXT2_OPTIONS, '-L', label, '-F', '-q', str(path)], check=True, capture_output=True,
                   env=dict(os.environ, E2FSPROGS_FAKE_TIME='946684800'))


def metal_root_tools(*names):
    import metal_root
    return metal_root.require_e2fsprogs(*names)


def ext2_metadata(path):
    """The block ranges an empty ext2 filesystem relies on, from dumpe2fs: superblocks, group descriptors,
    bitmaps and inode tables, in filesystem blocks."""
    dumpe2fs, = metal_root_tools('dumpe2fs')
    text = subprocess.run([dumpe2fs, str(path)], check=True, capture_output=True, text=True).stdout
    ranges = []
    for found in re.finditer(r'(?:superblock|descriptors|bitmap|table) at (\d+)(?:-(\d+))?', text):
        first = int(found.group(1))
        ranges.append((first, int(found.group(2) or first)))
    if 'Inode table at' not in text:
        raise ValueError('dumpe2fs listed no inode tables')
    return ranges


def chunks_with_data(path, chunk):
    """Offsets of the `chunk`-byte pieces of a file that hold a non-zero byte, scanning a mebibyte at a time."""
    window = 1 << 20
    zero = bytes(window)
    found = []
    with open(path, 'rb') as handle:
        offset = 0
        while piece := handle.read(window):
            if piece != zero[:len(piece)]:
                for at in range(0, len(piece), chunk):
                    if piece[at:at + chunk].count(0) != len(piece[at:at + chunk]):
                        found.append(offset + at)
            offset += len(piece)
    return found


def merge(ranges):
    """Sorted, coalesced (first, last) ranges."""
    merged = []
    for first, last in sorted(ranges):
        if merged and first <= merged[-1][1] + 1:
            merged[-1] = (merged[-1][0], max(merged[-1][1], last))
        else:
            merged.append((first, last))
    return merged


def ext2_extents(path):
    """What an empty ext2 image needs written, as sector extents: each non-zero block, and each metadata range
    (inode tables are zeros the filesystem relies on). Free data blocks are left as the disk has them."""
    per_block = EXT2_BLOCK // SECTOR
    blocks = [(offset // EXT2_BLOCK, offset // EXT2_BLOCK) for offset in chunks_with_data(path, EXT2_BLOCK)]
    return [(first * per_block, (last - first + 1) * per_block) for first, last in merge(blocks + ext2_metadata(path))]


def esp_image(path, size, files):
    """A FAT32 EFI system partition of `size` bytes holding `files` ({path in the ESP: source})."""
    sparse_file(path, size)
    metal_image.run('mkfs.fat', '-F', '32', '-s', '8', '-S', str(SECTOR), '-n', 'BREENIXESP', str(path))
    made = set()
    for name in sorted(files):
        parent = ''
        for part in Path(name).parent.parts:
            parent = f'{parent}/{part}'
            if parent not in made:
                metal_image.run('mmd', '-i', str(path), '::' + parent)
                made.add(parent)
        metal_image.run('mcopy', '-i', str(path), str(files[name]), '::/' + name)


class Request:
    """persist.cfg (lines the stub reads in order) and persist.dat (the data its extents and files name).

    Extents are kept as (partition, first sector, sectors, data offset or none) and adjacent ones of one kind are
    merged, so a mostly empty image costs a few lines."""

    def __init__(self, directory, action):
        self.directory = Path(directory)
        self.lines = [REQUEST_MAGIC, 'action ' + action]
        self.items = []
        self.data = self.directory / 'persist.dat'
        self.data.unlink(missing_ok=True)
        self.data.touch()
        self.written = 0

    @property
    def extents(self):
        return len(self.items)

    def append(self, source, offset=0, length=None):
        """Appends bytes of `source` to persist.dat; their offset there."""
        at = self.data.stat().st_size
        with open(source, 'rb') as handle, self.data.open('ab') as out:
            handle.seek(offset)
            remaining = length
            while remaining is None or remaining > 0:
                piece = handle.read(1 << 20 if remaining is None else min(1 << 20, remaining))
                if not piece:
                    break
                out.write(piece)
                if remaining is not None:
                    remaining -= len(piece)
        if length is not None and self.data.stat().st_size - at != length:
            raise ValueError('an extent ran past its image')
        return at

    def add(self, number, first, count, data):
        last = self.items[-1] if self.items else None
        if (last and last[0] == number and last[1] + last[2] == first and (last[3] is None) == (data is None)
                and (data is None or last[3] + last[2] * SECTOR == data)):
            self.items[-1] = (number, last[1], last[2] + count, last[3])
        else:
            self.items.append((number, first, count, data))
        self.written += count * SECTOR

    def extents_of(self, number, image, extents, run=EXT2_BLOCK):
        """Extents of partition `number` from `image`, in `run`-byte pieces: all-zero pieces as zero extents (no
        payload), the rest as data in persist.dat."""
        per_run = run // SECTOR
        with open(image, 'rb') as handle:
            for first, count in extents:
                at = 0
                while at < count:
                    size = min(per_run, count - at)
                    handle.seek((first + at) * SECTOR)
                    piece = handle.read(size * SECTOR)
                    if len(piece) != size * SECTOR:
                        raise ValueError('an extent ran past its image')
                    if piece.count(0) == len(piece):
                        self.add(number, first + at, size, None)
                    else:
                        self.add(number, first + at, size, self.append(image, (first + at) * SECTOR, size * SECTOR))
                    at += size

    def esp_file(self, path, source):
        at = self.append(source)
        self.lines.append(f'esp-file \\{path.replace("/", chr(92)).upper()} {at} {Path(source).stat().st_size}')

    def finish(self):
        extent_lines = [f'extent {n} {first} {count} ' + ('zero' if data is None else f'data {data}')
                        for n, first, count, data in self.items]
        tail = [line for line in self.lines if not line.startswith(('manager-config ', 'load-option '))]
        later = [line for line in self.lines if line.startswith(('manager-config ', 'load-option '))]
        config = self.directory / 'persist.cfg'
        config.write_text('\n'.join(tail + extent_lines + later) + '\n')
        return config


def install_request(directory, the_layout, expect, files, slot, note, site, scratch):
    """The one-time install: the layout, the four partitions' contents and the load option.

    `expect` is the 512-byte LBA 0 the owned disk holds now (its whole-disk root record); `files` the ESP files
    besides the slot (the manager)."""
    request = Request(directory, 'install')
    request.lines.append('expect-record ' + expect.hex())
    request.lines.append('disk ' + the_layout['diskGuid'])
    for role in ROLES:
        part = the_layout['partitions'][role]
        request.lines.append(f"partition {part['number']} {part['type']} {part['guid']} {part['first']} {part['last']} {part['name']}")
    scratch = Path(scratch)
    esp = scratch / 'esp.img'
    esp_image(esp, SIZES['esp'], dict(files, **{'efi/breenix/next.img': slot, 'efi/breenix/next.txt': note}))
    request.extents_of(1, esp, [(0, SIZES['esp'] // SECTOR)], run=FULL_CHUNK)
    esp.unlink()
    for role, label in (('system', 'breenix-system'), ('home', 'breenix-home')):
        image = scratch / f'{role}.img'
        ext2_image(image, SIZES[role], label)
        request.extents_of(the_layout['partitions'][role]['number'], image, ext2_extents(image))
        image.unlink()
    # The test root's first sectors: no stale whole-disk ext2 superblock remains where a test root goes.
    blank = scratch / 'blank.img'
    blank.write_bytes(bytes(FULL_CHUNK))
    request.extents_of(the_layout['partitions']['test']['number'], blank, [(0, FULL_CHUNK // SECTOR)])
    blank.unlink()
    for line in manager_config(site):
        request.lines.append('manager-config ' + line)
    request.lines.append('load-option ' + LOAD_OPTION)
    return request.finish(), request


def upgrade_request(directory, the_layout, files, slot, note, site):
    """An upgrade: the staged slot, the manager and its configuration into the EFI system partition."""
    request = Request(directory, 'upgrade')
    request.lines.append('expect-disk ' + the_layout['diskGuid'])
    request.lines.append('disk ' + the_layout['diskGuid'])
    esp = the_layout['partitions']['esp']
    request.lines.append(f"esp {esp['guid']}")
    for path, source in dict(files, **{'efi/breenix/next.img': slot, 'efi/breenix/next.txt': note}).items():
        request.esp_file(path, source)
    for line in manager_config(site):
        request.lines.append('manager-config ' + line)
    request.lines.append('load-option ' + LOAD_OPTION)
    return request.finish(), request


def file_facts(path):
    return {'size': Path(path).stat().st_size, 'sha256': metal_image.sha256(path)}


def store_definition(build_definition, commit):
    """Keeps the persistent kernel's definition files in the store, so each persistent boot is scored with them."""
    target = m.STATE / 'persistent' / commit / 'definition'
    if not target.is_dir():
        with tempfile.TemporaryDirectory(dir=m.STATE) as scratch:
            staged = Path(scratch) / 'definition'
            shutil.copytree(build_definition, staged)
            target.parent.mkdir(parents=True, exist_ok=True)
            os.replace(staged, target)
    return target

