#!/usr/bin/env python3
"""Lease a bounded lane checkout for the whole remote gate, outside that checkout."""
import hashlib
import fcntl
from contextlib import contextmanager
import os
import json
import signal
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


def prune(root, protected=(), reserve=0, required=True):
    """Evict only idle entries; permanent lock inodes live outside deleted entries."""
    limit = int(os.environ.get('BREENIX_GATE_CACHE_GB', '12')) * GIB
    free_floor = int(os.environ.get('BREENIX_GATE_FREE_GB', '15')) * GIB + reserve
    with lease(root, 'eviction'):
        entries = []
        for kind in ('trees', 'artifacts', 'fresh'):
            parent = root / kind
            if parent.exists():
                entries.extend(parent.iterdir())
        sizes = {p: size(p) for p in entries}
        total = sum(sizes.values())
        def last_used(entry):
            marker = root / 'owners' / (entry.parent.name + '-' + entry.name + '.json')
            if marker.exists():
                return json.loads(marker.read_text())['heartbeat']
            return entry.stat().st_mtime  # recency only; the lease decides liveness
        for entry in sorted(entries, key=last_used):
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
        if total > limit:
            print(f'[gate-cache] soft budget exceeded: bytes={total} limit={limit}; leased entries retained', flush=True)
        free = shutil.disk_usage(root).free
        if free < free_floor:
            message = f'free space {free / GIB:.1f} GiB is below safe floor {free_floor / GIB:.1f} GiB after idle eviction'
            if required:
                raise RuntimeError(message)
            print(f'[gate-cache] cleanup warning: {message}', flush=True)


def run(command, **kwargs):
    subprocess.run(command, check=True, **kwargs)


def owner(root, name, run_id, tree, started, state='running'):
    """External cleanup uses process identity and this marker, never copied mtimes."""
    directory = root / 'owners'
    directory.mkdir(exist_ok=True)
    boot = Path('/proc/sys/kernel/random/boot_id')
    process = Path(f'/proc/{os.getpid()}/stat')
    data = dict(run_id=run_id, pid=os.getpid(), boot_id=boot.read_text().strip() if boot.exists() else '',
                process_start=process.read_text().split(') ')[1].split()[19] if process.exists() else '',
                started=started, heartbeat=time.time(), state=state, tree=str(tree))
    temporary = directory / (name + '.tmp')
    temporary.write_text(json.dumps(data) + '\n')
    temporary.replace(directory / (name + '.json'))


def checkout(canonical, tree, sha, lane):
    # Retain every fetched SHA referenced by a shared checkout across canonical GC.
    run(['git', '-C', canonical, 'fetch', '--no-tags', '--no-write-fetch-head', '--no-auto-gc',
         'origin', f'{sha}:refs/breenix-gates/{lane}/{sha}'])
    def clone():
        tree.parent.mkdir(parents=True, exist_ok=True)
        run(['git', 'clone', '--shared', '--no-checkout', canonical, str(tree)])
    if not (tree / '.git').exists():
        clone()
    try:
        run(['git', '-C', str(tree), 'checkout', '--detach', '--force', sha])
    except subprocess.CalledProcessError:
        shutil.rmtree(tree)
        clone()
        run(['git', '-C', str(tree), 'checkout', '--detach', '--force', sha])
    run(['git', '-C', str(tree), 'clean', '-fd', '-e', 'rust-fork-real'])


def reclaim_legacy(canonical):
    """Reclaim finished legacy clones using run identity and open-process ownership."""
    if not Path('/proc').exists():
        return
    candidates = []
    for path in Path(canonical).parent.glob('breenix-*-gate-*'):
        match = re.fullmatch(r'breenix-(\d{8}T\d{6}Z)-x86_64-gate-[0-9a-f]+', path.name)
        if not match or not (path / '.git').exists():
            continue
        started = __import__('calendar').timegm(time.strptime(match[1], '%Y%m%dT%H%M%SZ'))
        if time.time() - started < 3600:
            continue
        logs = list((path / 'gate-tmp').glob('**/stdout.log'))
        # A completed suite/full boot alone is not a completed launch. A legacy
        # clone must have a finished DONE/report and no process referencing it.
        if not any(re.search(r'^SUITE [a-z0-9-]+ DONE |USERSPACE TEST COMPLETE', log.read_text(errors='replace'), re.M) for log in logs):
            continue
        candidates.append(path)
    referenced = set()
    for process in Path('/proc').glob('[0-9]*'):
        try:
            command = (process / 'cmdline').read_bytes().decode(errors='replace')
            links = [process / 'cwd'] + list((process / 'fd').iterdir())
            targets = [os.readlink(link) for link in links if link.is_symlink()]
        except (FileNotFoundError, ProcessLookupError):
            continue
        except PermissionError:
            return  # incomplete ownership scan cannot justify deletion
        for path in candidates:
            if str(path) in command or any(target == str(path) or target.startswith(str(path) + '/') for target in targets):
                referenced.add(path)
    for path in candidates:
        if path not in referenced:
            print(f'[gate-cache] reclaim finished legacy clone {path.name}', flush=True)
            shutil.rmtree(path)


def snapshot_fork(source, destination):
    """Build from an immutable private copy, and reject a concurrent source edit."""
    source = Path(source)
    def fingerprint():
        result = hashlib.sha256()
        for path in sorted((source / 'library').rglob('*')):
            if path.is_file() and 'target' not in path.relative_to(source).parts:
                result.update(str(path.relative_to(source)).encode() + path.read_bytes())
        for name in ('Cargo.toml', 'Cargo.lock', '.cargo/config.toml'):
            if (source / name).exists():
                result.update(name.encode() + (source / name).read_bytes())
        return result.hexdigest()
    before = fingerprint()
    shutil.rmtree(destination, ignore_errors=True)
    shutil.copytree(source / 'library', destination / 'library', ignore=shutil.ignore_patterns('target', '.git'))
    for name in ('Cargo.toml', 'Cargo.lock', '.cargo/config.toml'):
        if (source / name).exists():
            (destination / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / name, destination / name)
    if fingerprint() != before:
        raise RuntimeError('external Rust library changed during snapshot; refusing mixed inputs')


def remove_evidence(path):
    marker = path / 'gate-tmp/run-owner.json'
    if marker.exists() and json.loads(marker.read_text()).get('state') == 'finished':
        shutil.rmtree(path)
    else:
        print('[gate-cache] evidence retained: remote owner has not finished', flush=True)


def main():
    canonical, lane, sha, output, boots, mode = sys.argv[1:]
    if not re.fullmatch('[0-9a-f]{64}', lane) or not re.fullmatch('[0-9a-f]{40}', sha):
        raise ValueError('invalid lane key or commit')
    root = Path(os.environ.get('BREENIX_GATE_CACHE_DIR', str(Path(canonical).parent / 'breenix-gate-cache')))
    if not root.is_absolute():
        raise ValueError('BREENIX_GATE_CACHE_DIR must be an absolute path')
    root.mkdir(parents=True, exist_ok=True)
    run_id = Path(output).parent.name
    fresh = os.environ.get('BREENIX_GATE_FRESH') == '1'
    name = ('fresh-' + run_id) if fresh else ('trees-' + lane)
    tree = root / ('fresh' if fresh else 'trees') / (run_id if fresh else lane)
    start = time.monotonic()
    started = time.time()
    with lease(root, name) as lane_lock:
        phase('lane-wait', start)
        owner(root, name, run_id, tree, started)
        run_marker = Path(output) / 'run-owner.json'
        run_marker.write_text(json.dumps(dict(run_id=run_id, pid=os.getpid(), started=started, state='running')) + '\n')
        try:
            # The hard floor measures the entire filesystem, including canonical
            # sources, old per-run clones and evidence outside the cache budget.
            reclaim_legacy(canonical)
            prune(root, protected=(tree,))
            with timing('clone-fetch'):
                checkout(canonical, tree, sha, lane)
            if os.environ.get('BREENIX_RUST_FORK'):
                snapshot_fork(os.environ['BREENIX_RUST_FORK'], tree / 'gate-rust-fork')
            print(f'[gate-tree] tree={tree} fresh={str(fresh).lower()}', flush=True)
            environment = dict(os.environ, BREENIX_REPO_DIR=str(tree), BREENIX_GATE_CACHE_DIR=str(root), CARGO_BUILD_JOBS='6')
            if os.environ.get('BREENIX_RUST_FORK'):
                environment.update(BREENIX_RUST_FORK=str(tree / 'gate-rust-fork'))
            if not (tree / 'scripts/host-slots.py').exists():
                helper = str(Path(output) / 'host-slots.py')
                run(['python3', helper, 'acquire', 'x86-build'])
                run(['python3', helper, 'acquire', 'x86-boot'])
            child = subprocess.Popen([str(tree / 'docker/qemu/run-x86-gate.sh'), boots, mode], env=environment, cwd=tree, pass_fds=(lane_lock.fileno(),))
            def stop(number, frame):
                # The host-slot supervisor also signals descendants. Keep this
                # owner alive until the gate finishes its own cleanup.
                if child.poll() is None:
                    child.send_signal(number)
            for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
                signal.signal(number, stop)
            while child.poll() is None:
                owner(root, name, run_id, tree, started)
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pass
            return child.returncode
        finally:
            if fresh and tree.exists():
                shutil.rmtree(tree)
            owner(root, name, run_id, tree, started, 'finished')
            run_marker.write_text(json.dumps(dict(run_id=run_id, pid=os.getpid(), started=started, ended=time.time(), state='finished')) + '\n')
            try:
                prune(root, protected=(tree,) if not fresh else (), required=False)
            except OSError as error:
                print(f'[gate-cache] cleanup warning: {error}', flush=True)


if __name__ == '__main__':
    try:
        if len(sys.argv) == 3 and sys.argv[1] == 'remove-evidence':
            remove_evidence(Path(sys.argv[2]))
        else:
            sys.exit(main())
    except (OSError, RuntimeError, ValueError, subprocess.CalledProcessError) as error:
        print(f'GATE: FAIL ({error})', flush=True)
        sys.exit(1)
