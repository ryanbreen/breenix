#!/usr/bin/env python3
"""List desktop boot inputs; destinations are relative to the deployment root.

Bundle SHA-256 hashes sorted entries as type, relative path and content digest (or
symlink target), each NUL-delimited. This covers bundle contents and symlinks.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def file_digest(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def bundle_digest(bundle):
    digest = hashlib.sha256()
    for path in sorted(bundle.rglob('*')):
        relative = path.relative_to(bundle).as_posix()
        if path.is_symlink():
            kind, value = 'link', str(path.readlink())
        elif path.is_file():
            kind, value = 'file', file_digest(path)
        elif path.is_dir():
            kind, value = 'dir', ''
        else:
            raise ValueError(f'unsupported bundle entry: {path}')
        for field in (kind, relative, value):
            digest.update(field.encode('utf-8') + b'\0')
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', required=True, choices=['desktop'])
    parser.add_argument('--gpu-gl', required=True, action='store_true')
    parser.add_argument('--json', required=True, action='store_true')
    parser.parse_args()
    paths = ['target/aarch64-breenix-kernel/release/kernel-aarch64', 'target/ext2-aarch64.img']
    artifacts = [{'path': str(ROOT / name), 'sha256': file_digest(ROOT / name), 'destination': name}
                 for name in paths]
    binary = Path(subprocess.check_output([str(ROOT / 'scripts/build-qemu-gl.sh'), '--print-path'], text=True).strip())
    bundle = binary.parent.parent.parent
    if bundle.name != 'QEMU-GL.app' or not binary.is_file():
        raise ValueError(f'GL QEMU is not in QEMU-GL.app: {binary}')
    artifacts.append({'path': str(bundle), 'sha256': bundle_digest(bundle), 'destination': 'QEMU-GL.app'})
    print(json.dumps(artifacts, indent=2))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f'boot-artifacts: {error}')
