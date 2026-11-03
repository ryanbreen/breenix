#!/usr/bin/env python3
"""Set image ownership after unprivileged mke2fs -d; never chown host files."""
from pathlib import Path
import re
import struct
import subprocess
import sys


def set_owners(image):
    # ext2 on-disk inode layout (kernel/src/fs/ext2). mke2fs copied the builder's
    # uid/gid; all shipped files belong to root except the test account's home.
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
    paths = ['/etc/breenix/test-ssh/home', '/etc/breenix/test-ssh/home/.ssh',
             '/etc/breenix/test-ssh/home/.ssh/authorized_keys']
    commands = '\n'.join(f'set_inode_field {path} {field} 2000' for path in paths for field in ('uid', 'gid'))
    result = subprocess.run(['debugfs', '-w', '-f', '-', str(image)], input=commands + '\n',
                            capture_output=True, text=True, check=True)
    if re.search(r'not found|error|Usage:', result.stdout + result.stderr, re.I):
        raise ValueError('ext2 test-account ownership failed: ' + result.stderr)
    for path in paths:
        stat = subprocess.check_output(['debugfs', '-R', 'stat ' + path, str(image)], text=True)
        if not re.search(r'User:\s+2000\s+Group:\s+2000', stat):
            raise ValueError('ext2 ownership readback differs: ' + path)


if __name__ == '__main__':
    set_owners(sys.argv[1])
