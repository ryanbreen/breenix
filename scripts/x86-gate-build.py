#!/usr/bin/env python3
"""Builds an x86 gate run's inputs in the x86 build container, off the gate VM's cores.

  x86-gate-build.py remote HOST_FILE ROOT TREE LANE SHA RUN MODE OUTPUT
      In the gate VM (scripts/gate-tree.py runs it from the lane's tree): runs the build in the container named by
      HOST_FILE over ssh, then installs its outputs in TREE and its logs and result in OUTPUT.
  x86-gate-build.py container ROOT LANE SHA RUN MODE REPO RUST_FORK
      In the build container, as the remote side of `remote`: checks out SHA at the same path as the VM's tree,
      runs docker/qemu/run-x86-gate.sh's build phase there, and writes a tar to its standard output.

The container's cpuset is the boot host's shared CPUs, outside those reserved for the gate VM, and its slot
directory is its own: the build phase takes the container's x86-build leases, which no gate VM boot excludes,
and builds userspace through scripts/gate-artifacts.py exactly as the gate VM did (a cache miss builds a
candidate and a clean verification and publishes only byte-identical results, in the container's own cache).
The tree path matches the VM's because the launcher has absolute paths compiled in. The tar holds logs/<path>
(the build's logs), tree/<path> (the files the boot half reads, on success) and a final status.json naming every
output with its size, the build's exit status and the CPUs it ran on. Closing the ssh session's input stops
the build, so a cancelled gate run does not leave one behind.

HOST_FILE is private to the gate VM (never in the repository):
  BUILD_SSH        ssh destination of the build container (an alias in the VM's ssh configuration)
  BUILD_REPO       the container's clone of the repository, which fetches each commit
  BUILD_RUST_FORK  the container's checkout of the pinned Rust fork
"""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tarfile
import threading
import time

HERE = Path(__file__).resolve().parent
# The gate's settings that shape what the build phase builds; the boot half reads the rest itself.
FORWARDED = ('BREENIX_GATE_FRESH', 'BREENIX_BOOT_SUITE', 'BREENIX_QEMU_PROFILE', 'BREENIX_QEMU_STORAGE',
             'BREENIX_GATE_KERNEL_FEATURES', 'BREENIX_TARGET_POLICY_CHECKED', 'BREENIX_CATCHUP',
             'BREENIX_SLOT_WORKTREE', 'BREENIX_SLOT_COMMIT')
# Outputs a previous build left in the VM's tree; removed first, so a file the build no longer
# produces cannot boot.
STALE = ('userspace/programs/*.elf', 'userspace/programs/libc-busybox.links', 'target/test_binaries.img',
         'testdata/ext2.img', 'target/ext2.img', 'target/ext2-boot-target.img', 'target/release/qemu-uefi')
CHUNK = 1 << 20


def load(path):
    spec = importlib.util.spec_from_file_location(path.stem.replace('-', '_'), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def cpu_list(cpus):
    """1,2,3,7 -> 1-3,7"""
    ranges, cpus = [], sorted(cpus)
    for cpu in cpus:
        if ranges and cpu == ranges[-1][1] + 1:
            ranges[-1][1] = cpu
        else:
            ranges.append([cpu, cpu])
    return ','.join(f'{a}-{b}' if a != b else str(a) for a, b in ranges)


def relative(name):
    path = PurePosixPath(name)
    if not name or path.is_absolute() or '..' in path.parts:
        raise RuntimeError(f'unsafe path from the build container: {name!r}')
    return path


def read_host_file(path):
    values = {}
    for line in Path(path).read_text().splitlines():
        line = line.strip()
        if line and not line.startswith('#'):
            name, _, value = line.partition('=')
            values[name.strip()] = value.strip()
    missing = [name for name in ('BUILD_SSH', 'BUILD_REPO', 'BUILD_RUST_FORK') if not values.get(name)]
    if missing:
        raise RuntimeError(f'{path} does not set {", ".join(missing)}')
    return values


def copy_sparse(source, output):
    """Holes stay holes: the gate's ext2 images are mostly unallocated, so zero chunks are skipped."""
    while chunk := source.read(CHUNK):
        if chunk.count(0) == len(chunk):
            output.seek(len(chunk), os.SEEK_CUR)
        else:
            output.write(chunk)


def write_sparse(source, path, size, mode):
    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_name(path.name + '.part')
    with partial.open('wb') as output:
        copy_sparse(source, output)
        output.truncate(size)
    os.chmod(partial, mode & 0o755)
    partial.replace(path)


def receive(stream, tree, logs):
    """Installs the container's tar; returns its status, or raises if the stream ended without one."""
    status = None
    with tarfile.open(fileobj=stream, mode='r|') as archive:
        for member in archive:
            if member.name == 'status.json':
                status = json.loads(archive.extractfile(member).read())
                continue
            path = relative(member.name)
            if not member.isfile() or path.parts[0] not in ('tree', 'logs') or len(path.parts) < 2:
                raise RuntimeError(f'unexpected member from the build container: {member.name!r}')
            base = tree if path.parts[0] == 'tree' else logs
            write_sparse(archive.extractfile(member), base.joinpath(*path.parts[1:]), member.size, member.mode)
    if status is None:
        raise RuntimeError('the build container ended without reporting a result')
    if status['status'] == 0:
        for name, size in status['files'].items():
            target = tree / relative(name)
            if not target.is_file() or target.stat().st_size != size:
                raise RuntimeError(f'{name} did not arrive from the build container')
    return status


def send(stream, logs, tree, files, result):
    """The container's reply: its logs, the outputs, and status.json last, which receive() requires."""
    with tarfile.open(fileobj=stream, mode='w|', format=tarfile.PAX_FORMAT) as archive:
        for path in sorted(logs.rglob('*')):
            if path.is_file():
                archive.add(path, 'logs/' + str(path.relative_to(logs)), recursive=False)
        for name in files:
            archive.add(tree / name, 'tree/' + name, recursive=False)
        data = json.dumps(result).encode()
        info = tarfile.TarInfo('status.json')
        info.size = len(data)
        archive.addfile(info, io.BytesIO(data))


def remote(host_file, root, tree, lane, sha, run_id, mode, output):
    host = read_host_file(host_file)
    ssh = ['ssh', '-o', 'BatchMode=yes', '-o', 'ServerAliveInterval=15', '-o', 'ServerAliveCountMax=4', host['BUILD_SSH']]
    tools = [HERE / 'x86-gate-build.py', HERE / 'gate-tree.py']
    digest = hashlib.sha256(b''.join(path.read_bytes() for path in tools)).hexdigest()[:16]
    directory = f'{root}/tools/{digest}'
    bundle = io.BytesIO()
    with tarfile.open(fileobj=bundle, mode='w') as archive:
        for path in tools:
            archive.add(path, path.name)
    started = time.monotonic()
    subprocess.run(ssh + [f'mkdir -p {shlex.quote(directory)} && tar -C {shlex.quote(directory)} -xf -'],
                   input=bundle.getvalue(), check=True)
    for pattern in STALE:
        for path in tree.glob(pattern):
            path.unlink()
    settings = [f'{name}={os.environ[name]}' for name in FORWARDED if name in os.environ]
    command = shlex.join(['env', *settings, 'python3', f'{directory}/x86-gate-build.py', 'container', str(root),
                          lane, sha, run_id, mode, host['BUILD_REPO'], host['BUILD_RUST_FORK']])
    logs = output / 'build-logs-container'
    process = subprocess.Popen(ssh + [command], stdin=subprocess.PIPE, stdout=subprocess.PIPE)

    def stop(number, frame):
        process.terminate()  # its closed session stops the build in the container
    for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, stop)
    try:
        status = receive(process.stdout, tree, logs)
    except (tarfile.TarError, RuntimeError, OSError, ValueError, KeyError) as error:
        process.terminate()
        exit_status = process.wait()
        print(f'GATE: FAIL (the x86 build container returned no build: {error}; ssh exit {exit_status})', flush=True)
        return 1
    finally:
        process.stdin.close()
    process.wait()
    status['receive_seconds'] = round(time.monotonic() - started - status.get('seconds', 0), 1)
    (output / 'x86-build.json').write_text(json.dumps(status, indent=1) + '\n')
    print(f'[x86-gate-build] container build exit={status["status"]} seconds={status.get("seconds")} '
          f'cpus={status["cpus"]} cpuset={status["cpuset"]} x86-build-slots={status["slots"]} '
          f'files={len(status["files"])} logs={logs}', flush=True)
    return status['status']


def container(root, lane, sha, run_id, mode, repo, rust_fork):
    # Standard output carries the tar: everything this process and its children print goes to standard error.
    stream = os.fdopen(os.dup(1), 'wb')
    os.dup2(2, 1)
    sys.stdout = os.fdopen(1, 'w', buffering=1)
    os.environ['PATH'] = str(Path.home() / '.cargo/bin') + os.pathsep + os.environ.get('PATH', '')
    if not root.is_absolute() or not re.fullmatch('[0-9a-f]{64}', lane) or not re.fullmatch('[0-9a-f]{40}', sha) \
            or not re.fullmatch(r'[A-Za-z0-9._-]+', run_id) or mode not in ('full', 'kthread'):
        raise ValueError('invalid build request')
    trees = load(HERE / 'gate-tree.py')
    fresh = os.environ.get('BREENIX_GATE_FRESH') == '1'
    name = ('fresh-' + run_id) if fresh else ('trees-' + lane)
    tree = root / ('fresh' if fresh else 'trees') / (run_id if fresh else lane)
    run = root / 'builds' / run_id
    tmp = run / 'gate-tmp'
    outputs = run / 'outputs.json'
    child = None
    cancelled = threading.Event()

    def watch():
        while sys.stdin.buffer.read(65536):
            pass
        cancelled.set()
        if child is not None and child.poll() is None:
            child.send_signal(signal.SIGTERM)
    threading.Thread(target=watch, daemon=True).start()

    status, started, wall = 1, time.time(), time.monotonic()
    shutil.rmtree(run, ignore_errors=True)
    tmp.mkdir(parents=True)
    try:
        with trees.lease(root, name) as lane_lock:
            trees.phase('container-lane-wait', wall)
            trees.owner(root, name, run_id, tree, started)
            try:
                trees.prune(root, protected=(tree,))
                with trees.timing('container-clone-fetch'):
                    trees.checkout(repo, tree, sha, lane)
                trees.snapshot_fork(rust_fork, tree / 'gate-rust-fork')
                environment = dict(os.environ, BREENIX_REPO_DIR=str(tree), BREENIX_GATE_CACHE_DIR=str(root),
                                   CARGO_BUILD_JOBS='4', BREENIX_RUST_FORK=str(tree / 'gate-rust-fork'),
                                   BREENIX_GATE_PHASE='build', BREENIX_GATE_TMP=str(tmp),
                                   BREENIX_GATE_OUTPUTS=str(outputs))
                if not cancelled.is_set():
                    child = subprocess.Popen([str(tree / 'docker/qemu/run-x86-gate.sh'), '1', mode], env=environment,
                                             cwd=tree, stdin=subprocess.DEVNULL, pass_fds=(lane_lock.fileno(),))
                    while child.poll() is None:
                        trees.owner(root, name, run_id, tree, started)
                        try:
                            child.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            pass
                    status = child.returncode
            finally:
                trees.owner(root, name, run_id, tree, started, 'finished')
        listed = json.loads(outputs.read_text()) if status == 0 else {'files': [], 'build_seconds': 0}
        files = {name: (tree / relative(name)).stat().st_size for name in listed['files']}
    except (OSError, RuntimeError, ValueError, subprocess.CalledProcessError) as error:
        print(f'GATE: FAIL (x86 build container: {error})', flush=True)
        status, files, listed = status or 1, {}, {'build_seconds': 0}
    slots = load(tree / 'scripts/host-slots.py') if (tree / 'scripts/host-slots.py').exists() else None
    result = {'status': status, 'seconds': round(time.monotonic() - wall, 1), 'build_seconds': listed['build_seconds'],
              'files': files, 'cpus': cpu_list(os.sched_getaffinity(0)),
              'cpuset': Path('/sys/fs/cgroup/cpuset.cpus.effective').read_text().strip()
              if Path('/sys/fs/cgroup/cpuset.cpus.effective').exists() else 'unknown',
              'slots': slots.RESOURCES['x86-build'] if slots else 'unknown'}
    try:
        send(stream, tmp, tree, files, result)
        stream.close()
    except BrokenPipeError:
        pass  # the gate VM went away; nothing is waiting for the result
    finally:
        if fresh:
            shutil.rmtree(tree, ignore_errors=True)
        shutil.rmtree(run, ignore_errors=True)
        try:
            trees.prune(root, protected=() if fresh else (tree,), required=False)
        except OSError as error:
            print(f'[gate-cache] cleanup warning: {error}', flush=True)
    return status


def main():
    arguments = sys.argv[1:]
    if arguments[:1] == ['remote'] and len(arguments) == 9:
        host_file, root, tree, lane, sha, run_id, mode, output = arguments[1:]
        return remote(Path(host_file), Path(root), Path(tree), lane, sha, run_id, mode, Path(output))
    if arguments[:1] == ['container'] and len(arguments) == 8:
        root, lane, sha, run_id, mode, repo, rust_fork = arguments[1:]
        return container(Path(root), lane, sha, run_id, mode, repo, rust_fork)
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, RuntimeError, ValueError, subprocess.CalledProcessError) as error:
        print(f'GATE: FAIL ({error})', flush=True)
        sys.exit(1)
