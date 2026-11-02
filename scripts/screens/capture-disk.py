#!/usr/bin/env python3
"""Create a new marked raw capture disk and enable capture on a disposable root."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def prepare(disk, root):
    debugfs = shutil.which('debugfs') or next((p for p in (
        '/opt/homebrew/opt/e2fsprogs/sbin/debugfs', '/usr/local/opt/e2fsprogs/sbin/debugfs')
        if Path(p).is_file()), None)
    if not debugfs:
        raise ValueError('screen capture requires host debugfs')
    if ',' in str(disk):
        raise ValueError('capture disk path cannot contain a comma')
    with tempfile.TemporaryDirectory(prefix='screen-capture-') as temporary:
        setting = Path(temporary) / 'setting'
        setting.write_text('on\n')
        for command in ('rm /etc/breenix/screen-capture',
                        f'write {setting} /etc/breenix/screen-capture'):
            subprocess.run([debugfs, '-w', '-R', command, str(root)], check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        value = subprocess.check_output([debugfs, '-R', 'cat /etc/breenix/screen-capture', str(root)],
                                        stderr=subprocess.DEVNULL)
        if value != b'on\n':
            raise ValueError('capture setting readback differs')
    # O_EXCL refuses an existing disk, including a symlink, without truncating it.
    fd = os.open(disk, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'wb') as output:
        output.truncate(8 * 1024 * 1024 * 1024)
        output.write(b'SCRDISK1' + bytes(504))
        output.flush()
        os.fsync(output.fileno())


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('disk', type=Path)
    parser.add_argument('root', type=Path)
    args = parser.parse_args()
    try:
        prepare(args.disk, args.root)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f'screen capture: {error}\n')
