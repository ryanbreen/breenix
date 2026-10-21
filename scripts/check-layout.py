#!/usr/bin/env python3
"""Compare a built ext2 image's directories and metadata with the shared layout table."""
import argparse
from pathlib import Path
import re
import shutil
import subprocess


def check(image, table):
    debugfs = shutil.which('debugfs')
    if debugfs is None:
        for directory in ('/usr/sbin', '/sbin', '/opt/homebrew/opt/e2fsprogs/sbin'):
            candidate = Path(directory) / 'debugfs'
            if candidate.is_file():
                debugfs = str(candidate)
                break
    if debugfs is None:
        raise ValueError('layout verification requires e2fsprogs debugfs')
    count = 0
    for line in table.read_text().splitlines():
        row = line.split()
        if len(row) != 5 or row[0] not in ('dir', 'file'):
            continue
        kind, path, mode, uid, gid = row
        result = subprocess.run([debugfs, '-R', 'stat /' + path, str(image)], text=True, capture_output=True, check=True)
        text = result.stdout
        attrs = re.search(r'Type:\s+(\w+)\s+Mode:\s+([0-7]+)', text)
        owner = re.search(r'User:\s+(\d+)\s+Group:\s+(\d+)', text)
        expected_kind = 'directory' if kind == 'dir' else 'regular'
        if not attrs or not owner or (attrs[1], int(attrs[2], 8), int(owner[1]), int(owner[2])) != (expected_kind, int(mode, 8), int(uid), int(gid)):
            raise ValueError('/' + path + ': built image differs from layout table: ' + text.strip())
        count += 1
    print(f'layout image metadata: {count} table rows passed')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image', type=Path)
    parser.add_argument('--table', type=Path, default=Path(__file__).resolve().parents[1] / 'libs/libbreenix/src/layout.tsv')
    args = parser.parse_args()
    check(args.image, args.table)
