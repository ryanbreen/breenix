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
        if (run / 'fault-kernel').exists():
            artifacts['fault-kernel'] = digest(run / 'fault-kernel')
    fault_kind = (run / 'fault-kind').read_text().strip() if (run / 'fault-kind').exists() else None
    if fault_kind not in (None, 'panic-reboot', 'watchdog') or bool(fault_kind) != ('fault-kernel' in artifacts):
        raise ValueError('invalid fault kernel kind')
    build = dict(commit=commit if source_commit else None, dirty=dirty, sourceBuilt=bool(source_commit), artifacts=artifacts, faultKind=fault_kind)
    (run / 'build.json').write_text(json.dumps(build) + '\n')


def verify(root, run, kernel, fault=False):
    run = Path(run)
    build = json.loads((run / 'build.json').read_text())
    # QEMU uses this private copy, rather than a mutable shared target output.
    shutil.copyfile(kernel, run / 'boot-kernel')
    if not (run / 'power-token').is_file() or 'power-token' not in build['artifacts']:
        raise ValueError('VM run predates power-token sealing; build a new run')
    if digest(run / 'power-token') != build['artifacts']['power-token']:
        raise ValueError('VM power token differs from the sealed token')
    if fault and build.get('sourceBuilt') is not True:
        raise ValueError('fault boot requires a source-built kernel')
    eligible = False
    expected = build['artifacts'].copy()
    continuation = run / 'continuation.json'
    if continuation.exists():
        seal = json.loads(continuation.read_text())
        if seal['buildCommit'] != build.get('commit'):
            raise ValueError('continuation belongs to a different build')
        expected.update(seal['disks'])
    if build.get('sourceBuilt') is True:
        for name in ('system.img', 'home.img'):
            if digest(run / name) != expected[name]:
                raise ValueError('VM artifacts differ from the latest seal (disk)')
    if build.get('sourceBuilt') is True:
        kernel_name = 'fault-kernel' if fault else 'kernel'
        if digest(run / 'boot-kernel') != expected.get(kernel_name):
            raise ValueError('VM artifacts differ from the source build (kernel)')
        if build['commit'] != git(root, 'rev-parse', 'HEAD') or git(root, 'status', '--porcelain'):
            raise ValueError('source build does not match the clean tools commit')
        eligible = build.get('dirty') is False
    actual = dict(expected)
    actual.update({name: digest(run / name) for name in ('system.img', 'home.img')})
    receipt = dict(build, artifacts=actual, evidenceEligible=eligible)
    (run / 'verified-build.json').write_text(json.dumps(receipt) + '\n')
    return receipt


def continuation(run):
    run = Path(run)
    boot = run / 'boots' / (run / 'current').read_text().strip()
    events = [json.loads(line) for line in (boot / 'events.jsonl').read_text().splitlines()]
    requested = False
    verified = False
    for event in events:
        if event.get('event') == 'requested-shutdown':
            requested = event.get('action') == 'poweroff'
        if event.get('event') == 'SHUTDOWN' and event.get('data', {}).get('reason') == 'guest-shutdown' and requested:
            verified = True
    if verified:
        build = json.loads((run / 'build.json').read_text())
        seal = {'buildCommit': build.get('commit'), 'disks': {name: digest(run / name) for name in ('system.img', 'home.img')}}
        tmp = run / 'continuation.json.tmp'
        tmp.write_text(json.dumps(seal) + '\n')
        tmp.replace(run / 'continuation.json')


if __name__ == '__main__':
    operation, root, run, value = sys.argv[1:]
    if operation == 'seal':
        seal(root, run, value)
    elif operation in ('verify', 'verify-fault'):
        verify(root, run, value, operation == 'verify-fault')
    elif operation == 'stage-intent':
        build = json.loads((Path(run) / 'build.json').read_text())
        seal = {'buildCommit': build.get('commit'), 'disks': {name: digest(Path(run) / name) for name in ('system.img', 'home.img')}}
        tmp = Path(run) / 'continuation.json.tmp'
        tmp.write_text(json.dumps(seal) + '\n')
        tmp.replace(Path(run) / 'continuation.json')
        receipt_path = Path(run) / 'verified-build.json'
        receipt = json.loads(receipt_path.read_text())
        receipt['artifacts'].update(seal['disks'])
        receipt_path.write_text(json.dumps(receipt) + '\n')
    elif operation == 'continuation':
        continuation(run)
    else:
        raise SystemExit('expected seal or verify')
