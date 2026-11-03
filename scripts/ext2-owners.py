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


if __name__ == '__main__':
    set_owners(*sys.argv[1:])
