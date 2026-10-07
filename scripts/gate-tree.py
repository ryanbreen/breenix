#!/usr/bin/env python3
"""Lease a bounded lane checkout for the whole remote gate, outside that checkout."""
import fcntl
from contextlib import contextmanager
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import time

GIB = 1024 ** 3


def phase(name, start, status=0):
    print(f'[gate-phase] phase={name} ended={time.time():.3f} seconds={time.monotonic()-start:.3f} status={status}', flush=True)


@contextmanager
def timing(name, detail=''):
    started = time.monotonic()
    print(f'[gate-phase] phase={name} started={time.time():.3f} {detail}'.rstrip(), flush=True)
    try:
        yield
    except BaseException:
        phase(name, started, 1)
        raise
    else:
        phase(name, started)


def size(path):
    # Allocated bytes: ext2/test disk copies can be sparse.
    total = 0
    for directory, _, files in os.walk(path):
        for name in files:
            try:
                info = (Path(directory) / name).lstat()
            except FileNotFoundError:
                continue  # an active lane may be cleaning temporary Cargo files
            if stat.S_ISREG(info.st_mode):
                total += info.st_blocks * 512
    return total


def lease(root, name, blocking=True):
    (root / 'locks').mkdir(parents=True, exist_ok=True)
    handle = (root / 'locks' / name).open('a+')
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | (0 if blocking else fcntl.LOCK_NB))
    except BlockingIOError:
        handle.close()
        return None
    return handle


def prune(root, protected=(), reserve=0):
    """Evict only idle entries; permanent lock inodes live outside deleted entries."""
    limit = int(os.environ.get('BREENIX_GATE_CACHE_GB', '12')) * GIB
    free_floor = int(os.environ.get('BREENIX_GATE_FREE_GB', '2')) * GIB + reserve
    with lease(root, 'eviction'):
        entries = []
        for kind in ('trees', 'artifacts'):
            parent = root / kind
            if parent.exists():
                entries.extend(parent.iterdir())
        sizes = {p: size(p) for p in entries}
        total = sum(sizes.values())
        for entry in sorted(entries, key=lambda p: p.stat().st_mtime):
            if total <= limit and shutil.disk_usage(root).free >= free_floor:
                break
            if entry in protected:
                continue
            handle = lease(root, entry.parent.name + '-' + entry.name, blocking=False)
            if handle is None:
                continue
            with handle:
                total -= sizes[entry]
                print(f'[gate-cache] evict {entry.parent.name}/{entry.name} bytes={sizes[entry]}', flush=True)
                shutil.rmtree(entry)
        if total > limit or shutil.disk_usage(root).free < free_floor:
            raise RuntimeError('gate cache budget/free-space reserve cannot be met without evicting an active entry')


def run(command, **kwargs):
    subprocess.run(command, check=True, **kwargs)


def main():
    canonical, lane, sha, output, boots, mode = sys.argv[1:]
    if not re.fullmatch('[0-9a-f]{64}', lane) or not re.fullmatch('[0-9a-f]{40}', sha):
        raise ValueError('invalid lane key or commit')
    root = Path(os.environ.get('BREENIX_GATE_CACHE_DIR', str(Path(canonical).parent / 'breenix-gate-cache')))
    if not root.is_absolute():
        raise ValueError('BREENIX_GATE_CACHE_DIR must be an absolute path')
    root.mkdir(parents=True, exist_ok=True)
    tree = root / 'trees' / lane
    start = time.monotonic()
    with lease(root, 'trees-' + lane) as lane_lock:
        phase('lane-wait', start)
        # Budget for checkout, kernel targets and first-key clean verification.
        prune(root, protected=(tree,), reserve=2 * GIB)
        fresh = os.environ.get('BREENIX_GATE_FRESH') == '1'
        if fresh:
            tree = Path(output).parent / 'clean-tree'
        with timing('clone-fetch'):
            run(['git', '-C', canonical, 'fetch', '--no-tags', '--no-write-fetch-head', '--no-auto-gc', 'origin', sha])
            if not (tree / '.git').exists():
                tree.parent.mkdir(parents=True, exist_ok=True)
                run(['git', 'clone', '--shared', canonical, str(tree)])
            # Reset tracked files, including the host-only rust-fork symlink.
            run(['git', '-C', str(tree), 'reset', '--hard'])
            run(['git', '-C', str(tree), 'checkout', '--detach', '--force', sha])
            # Remove obsolete ignored ELFs; targets remain incremental. Untracked
            # source files must never survive checkout of an older requested commit.
            run(['git', '-C', str(tree), 'clean', '-fd', '-e', 'rust-fork-real'])
        environment = dict(os.environ, BREENIX_REPO_DIR=str(tree), BREENIX_GATE_CACHE_DIR=str(root), CARGO_BUILD_JOBS='6')
        if not (tree / 'scripts/host-slots.py').exists():
            helper = str(Path(output) / 'host-slots.py')
            run(['python3', helper, 'acquire', 'x86-build'])
            run(['python3', helper, 'acquire', 'x86-boot'])
        status = subprocess.call([str(tree / 'docker/qemu/run-x86-gate.sh'), boots, mode], env=environment, cwd=tree, pass_fds=(lane_lock.fileno(),))
        if not fresh:
            os.utime(tree, None)
        prune(root, protected=(tree,) if not fresh else ())
        return status


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (RuntimeError, ValueError, subprocess.CalledProcessError) as error:
        print(f'GATE: FAIL ({error})', flush=True)
        sys.exit(1)
