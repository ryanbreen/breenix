#!/usr/bin/env python3
"""Seal source-built VM artifacts and verify the bytes selected for launch."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()


def seal(root, run, source_commit):
    run = Path(run)
    commit = git(root, 'rev-parse', 'HEAD')
    dirty = bool(git(root, 'status', '--porcelain'))
    if source_commit and (source_commit != commit or dirty):
        raise ValueError('sources changed during build')
    artifacts = {name: digest(run / name) for name in ('system.img', 'home.img', 'power-token')}
    if source_commit:
        artifacts['kernel'] = digest(run / 'kernel')
    build = dict(commit=commit if source_commit else None, dirty=dirty, sourceBuilt=bool(source_commit), artifacts=artifacts)
    (run / 'build.json').write_text(json.dumps(build) + '\n')


def verify(root, run, kernel):
    run = Path(run)
    build = json.loads((run / 'build.json').read_text())
    # QEMU uses this private copy, rather than a mutable shared target output.
    shutil.copyfile(kernel, run / 'boot-kernel')
    if digest(run / 'power-token') != build['artifacts']['power-token']:
        raise ValueError('VM power token differs from the sealed token')
    eligible = False
    if build.get('sourceBuilt') is True:
        expected = build['artifacts']
        actual = {name: digest(run / ('boot-kernel' if name == 'kernel' else name))
                  for name in ('kernel', 'system.img', 'home.img', 'power-token')}
        if actual != expected:
            raise ValueError('VM artifacts differ from the source build')
        if build['commit'] != git(root, 'rev-parse', 'HEAD') or git(root, 'status', '--porcelain'):
            raise ValueError('source build does not match the clean tools commit')
        eligible = build.get('dirty') is False
    receipt = dict(build, evidenceEligible=eligible)
    (run / 'verified-build.json').write_text(json.dumps(receipt) + '\n')
    return receipt


if __name__ == '__main__':
    operation, root, run, value = sys.argv[1:]
    if operation == 'seal':
        seal(root, run, value)
    elif operation == 'verify':
        verify(root, run, value)
    else:
        raise SystemExit('expected seal or verify')
