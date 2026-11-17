#!/usr/bin/env python3
"""Set image ownership after unprivileged mke2fs -d; never chown host files."""
from pathlib import Path
import re
import struct
import subprocess
import sys


def set_owners(image, ownership, root):
    # ext2 on-disk inode layout (kernel/src/fs/ext2). mke2fs copied the builder's
    # uid/gid; initialize root ownership, then apply the guest account/group map.
    with Path(image).open('r+b') as disk:
        disk.seek(1024)
        sb = disk.read(1024)
        if struct.unpack_from('<H', sb, 56)[0] != 0xef53:
            raise ValueError('not an ext2 image')
        u32 = lambda offset: struct.unpack_from('<I', sb, offset)[0]
        block = 1024 << u32(24)
        groups = (u32(4) - u32(20) + u32(32) - 1) // u32(32)
        inode_size = struct.unpack_from('<H', sb, 88)[0]
        disk.seek((u32(20) + 1) * block)
        descriptors = disk.read(groups * 32)
        for group in range(groups):
            table = struct.unpack_from('<I', descriptors, group * 32 + 8)[0] * block
            for index in range(u32(40)):
                position = table + index * inode_size
                for offset in (2, 24, 120, 122):  # uid, gid, uid_high, gid_high
                    disk.seek(position + offset)
                    disk.write(bytes(2))
    entries, links = [], []
    for line in Path(ownership).read_text(errors='surrogateescape').splitlines():
        fields = line.split('\t')
        if fields[0] == 'link':
            links.append(tuple('/' + str(Path(path).relative_to(root)) for path in fields[1:]))
            continue
        owner, path = fields
        uid, gid = map(int, owner.split(':'))
        relative = Path(path).relative_to(root)
        entries.append(('/' + str(relative), uid, gid))
    commands = '\n'.join(f'set_inode_field {path} {field} {value}' for path, uid, gid in entries
                         for field, value in (('uid', uid), ('gid', gid)))
    for source, target in links:
        stat = subprocess.check_output(['debugfs', '-R', 'stat ' + source, str(image)], text=True)
        count = int(re.search(r'Links:\s+(\d+)', stat)[1])
        commands += f'\nln {source} {target}\nset_inode_field {source} links_count {count + 1}'
    result = subprocess.run(['debugfs', '-w', '-f', '-', str(image)],
                            input=(commands + '\n').encode(errors='surrogateescape'),
                            capture_output=True, check=True)
    if re.search(rb'not found|error|Usage:', result.stdout + result.stderr, re.I):
        raise ValueError('ext2 metadata failed: ' + result.stderr.decode(errors='replace'))
    for source, target in links:
        before, after = [subprocess.check_output(['debugfs', '-R', 'stat ' + path, str(image)], text=True)
                         for path in (source, target)]
        if re.search(r'Inode:\s+(\d+)', before)[1] != re.search(r'Inode:\s+(\d+)', after)[1]:
            raise ValueError('ext2 fixture hard-link readback differs')
    for path, uid, gid in entries:
        if uid == gid == 0:
            continue  # The shared layout verifier checks the root-owned table rows.
        stat = subprocess.check_output(['debugfs', '-R', 'stat ' + path, str(image)], text=True)
        if not re.search(rf'User:\s+{uid}\s+Group:\s+{gid}\b', stat):
            raise ValueError('ext2 ownership readback differs: ' + path)


def list_entries(image):
    # debugfs is a read-only binary oracle; its parsable directory listing includes
    # inode identity, mode and numeric ownership even when rdump cannot chown.
    pending = ['/']
    seen_dirs = set()
    entries = []
    while pending:
        directory = pending.pop()
        quoted = '"' + directory.replace('\\', '\\\\').replace('"', '\\"') + '"'
        result = subprocess.run(['debugfs', '-R', 'ls -p -l ' + quoted, str(image)],
                                capture_output=True, check=True)
        if not result.stdout.startswith(b'/'):
            raise ValueError('debugfs could not list ' + directory + ': ' + result.stderr.decode(errors='replace'))
        for line in result.stdout.decode(errors='surrogateescape').splitlines():
            fields = line.split('/')
            if len(fields) < 8 or fields[0] or fields[5] in ('.', '..'):
                continue
            ino, mode, uid, gid, name = fields[1:6]
            if ino == '0':
                continue
            if not ino or not name or '\t' in name or '\n' in name:
                raise ValueError('invalid ext2 directory entry')
            path = directory.rstrip('/') + '/' + name
            entries.append((int(ino), int(mode, 8), int(uid), int(gid), path))
            if int(mode, 8) & 0o170000 == 0o040000:
                if int(ino) in seen_dirs:
                    raise ValueError('directory cycle in image')
                seen_dirs.add(int(ino))
                pending.append(path)
    # The root is not returned by ls as a normal entry.
    stat = subprocess.check_output(['debugfs', '-R', 'stat /', str(image)], text=True)
    owner = re.search(r'User:\s+(\d+)\s+Group:\s+(\d+)', stat)
    if not owner:
        raise ValueError('cannot read root ownership')
    root_mode = re.search(r'Mode:\s+(0?[0-7]+)', stat)
    if not root_mode:
        raise ValueError('cannot read root mode')
    entries.append((2, 0o040000 | int(root_mode[1], 8), int(owner[1]), int(owner[2]), '/'))
    return sorted(entries, key=lambda item: item[4])


def list_owners(image):
    for _, mode, uid, gid, path in list_entries(image):
        if mode & 0o170000 == 0o120000:
            # Linux cannot chmod a symlink; preserve its image mode in mkfs.
            print(f'{uid}:{gid}:{mode & 0o7777:o}\t{path}')
        elif uid or gid:
            print(f'{uid}:{gid}\t{path}')


def restore_links(image, tree):
    import os
    first = {}
    entries = list_entries(image)
    for ino, mode, _, _, path in entries:
        if mode & 0o170000 == 0o040000:
            continue
        target = Path(tree) / path.lstrip('/')
        if ino in first:
            target.unlink()
            os.link(first[ino], target, follow_symlinks=False)
        else:
            first[ino] = target
    for _, mode, _, _, path in reversed(entries):
        if mode & 0o170000 != 0o120000:
            os.chmod(Path(tree) / path.lstrip('/'), mode & 0o7777)


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--list':
        list_owners(sys.argv[2])
    elif len(sys.argv) == 4 and sys.argv[1] == '--restore-links':
        restore_links(sys.argv[2], sys.argv[3])
    else:
        set_owners(*sys.argv[1:])
