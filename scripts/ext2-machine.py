#!/usr/bin/env python3
"""Refuse a supplied filesystem whose userspace belongs to another architecture."""
from pathlib import Path
import os
import shutil
import struct
import subprocess
import sys


def check(image, arch):
    candidates = [os.environ.get('BREENIX_DEBUGFS'),
                  '/opt/homebrew/opt/e2fsprogs/sbin/debugfs',
                  '/usr/local/opt/e2fsprogs/sbin/debugfs',
                  '/sbin/debugfs', '/usr/sbin/debugfs', shutil.which('debugfs')]
    tool = next((p for p in candidates if p and os.access(p, os.X_OK)), None)
    if tool is None:
        raise ValueError('debugfs is required to check the image architecture')
    result = subprocess.run([tool, '-R', 'cat /bin/busybox', str(image)], capture_output=True)
    errors = [line for line in result.stderr.splitlines() if not line.startswith(b'debugfs ')]
    data = result.stdout
    if result.returncode or errors or len(data) < 64 or data[:7] != b'\x7fELF\x02\x01\x01':
        raise ValueError('cannot read an ELF64 /bin/busybox from the image')
    machine = struct.unpack_from('<H', data, 18)[0]
    expected = 183 if arch in ('arm64', 'aarch64') else 62
    if machine != expected:
        raise ValueError('wrong architecture: image machine %d, expected %d (%s)' % (machine, expected, arch))


if __name__ == '__main__':
    try:
        check(Path(sys.argv[1]), sys.argv[2])
    except (OSError, ValueError) as error:
        sys.exit('--disk: ' + str(error))
