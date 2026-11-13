#!/usr/bin/env python3
"""Run the headless ARM64 LAN fixtures together and retain one paired verdict.

Prepare the program-mode kernel and disk first. This runner always uses --no-build.
The outer QEMU lock and host-slots supervisor own the whole pair; child launchers
retain their normal locking, recording and cleanup, with private lock directories.
"""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import pwd
import re
import signal
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]


class PairSlots:
    """Aggregate nested boot leases while the outer lock owns both guests.

    host-slots.py release stops all QEMUs in its session. A child finishing must
    therefore release only its private QEMU lock, never the pair's boot lease.
    All other requests reach the unchanged supervisor, serialized because it
    permits only one pending request per session. The outer shell releases the
    real lease after both launchers have exited, including on cancellation.
    """

    def __init__(self, upstream):
        self.upstream = upstream
        self.errors = []

    def __enter__(self):
        self.directory = tempfile.TemporaryDirectory(prefix='collab-slots-', dir='/tmp')
        address = str(Path(self.directory.name) / 'control.sock')
        owner = self

        class Handler(socketserver.StreamRequestHandler):
            def handle(self):
                try:
                    self.request.settimeout(10)
                    data = self.rfile.readline(65536)
                    request = json.loads(data)
                    if (request.get('operation') in ('acquire', 'release')
                            and request.get('resource') == 'mac-boot'):
                        response = b'{}\n'
                    elif request.get('operation') == 'quiesce':
                        raise ValueError('a guest cannot quiesce the running pair')
                    else:
                        with socket.socket(socket.AF_UNIX) as client:
                            client.connect(owner.upstream)
                            client.sendall(data)
                            with client.makefile('rb') as stream:
                                response = stream.readline(65536)
                            result = json.loads(response)
                            if 'error' in result:
                                owner.errors.append(result['error'])
                    self.wfile.write(response)
                except (OSError, ValueError) as error:
                    owner.errors.append(str(error))
                    self.wfile.write(json.dumps({'error': str(error)}).encode() + b'\n')

        self.server = socketserver.UnixStreamServer(address, Handler)
        self.worker = threading.Thread(target=self.server.serve_forever,
                                       kwargs={'poll_interval': 0.05}, daemon=True)
        self.worker.start()
        return address

    def __exit__(self, *exception):
        self.server.shutdown()
        self.server.server_close()
        self.worker.join()
        self.directory.cleanup()


def stop_children(children):
    # Signal the process groups established by start_new_session below.
    for number in (signal.SIGTERM, signal.SIGKILL):
        for child in children.values():
            if child.poll() is None:
                try:
                    os.killpg(child.pid, number)
                except ProcessLookupError:
                    pass
        deadline = time.monotonic() + 5
        for child in children.values():
            try:
                child.wait(timeout=max(0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                pass
    for child in children.values():
        child.wait()


def guest_result(serial, child, records):
    result = {'serial': str(serial), 'vigil_id': None,
              'exit_status': child.returncode if child else None}
    errors = []
    matches = []
    for record in records:
        try:
            run = json.loads(record.read_text())
            recorded_serial = record.parent / 'serial.txt'
            by_link = recorded_serial.is_symlink() and recorded_serial.resolve() == serial
            by_pid = child and (record.parent / 'pid').read_text().strip() == str(child.pid)
            if by_link or by_pid:
                matches.append(run['id'])
        except (OSError, ValueError, KeyError):
            continue
    if len(matches) == 1:
        result['vigil_id'] = matches[0]
    else:
        errors.append('missing or ambiguous Vigil registration')
    if child is None or child.returncode != 0:
        errors.append('launcher did not exit successfully')
    try:
        data = serial.read_text(errors='replace')
    except OSError:
        data = ''
    role = serial.parent.name
    lines = [line.rstrip('\r') for line in data.splitlines(keepends=True)
             if line.startswith('COLLAB_LAN ')]
    expected = re.compile(rf'COLLAB_LAN role={role} ops=8 digest=([0-9a-f]{{64}})\r?\n')
    if len(lines) == 1 and (match := expected.fullmatch(lines[0])):
        result.update(ops=8, digest=match[1])
    else:
        errors.append('expected one complete COLLAB_LAN ops=8 digest record without FAIL')
    result['errors'] = errors
    return result


def run_pair(args):
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix='collab-pair-', dir=output.parent))
    home = Path(os.environ.get('VIGIL_TEST_HOME') or pwd.getpwuid(os.getuid()).pw_dir)
    boots = home / 'Library/Application Support/Vigil/breenix/boots'
    before = set(boots.glob('*/run.json'))
    children = {}
    errors = []
    verdict = {'verdict': 'FAIL', 'errors': errors}

    def interrupted(number, frame):
        raise InterruptedError(f'pair interrupted by signal {number}')

    previous = {number: signal.signal(number, interrupted)
                for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    slots = PairSlots(os.environ['BREENIX_SLOT_SESSION'])
    try:
        with ExitStack() as stack:
            session = stack.enter_context(slots)
            lock_root = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix='collab-locks-', dir='/tmp')))
            try:
                for role in ('host', 'join'):
                    folder = evidence / role
                    folder.mkdir()
                    serial = folder / 'serial.txt'
                    command = [str(args.launcher.resolve()), '--mode', 'program', '--program',
                               f'/usr/libexec/breenix/collab-lan-{role}', '--no-display',
                               '--no-build', '--accel', 'hvf', '--serial-log', str(serial)]
                    if role == 'host':
                        command += ['--collab-host', '7890']
                    env = dict(os.environ, BREENIX_SLOT_SESSION=session,
                               BREENIX_QEMU_LOCK=str(lock_root / role),
                               BREENIX_COLLAB_HOST_BIND='127.0.0.1')
                    # Each launcher registers its own PID, even if this runner was
                    # itself entered from a launcher with a recorder PID in its env.
                    env.pop('VIGIL_BOOT_ID', None)
                    log = stack.enter_context((folder / 'launcher.log').open('w'))
                    children[role] = subprocess.Popen(
                        ['bash', '-c', 'export BREENIX_LAUNCHER_PID=$$; exec "$@"',
                         'collab-guest', *command], env=env, stdin=subprocess.DEVNULL,
                        stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
                deadline = time.monotonic() + args.timeout
                for child in children.values():
                    child.wait(timeout=max(0, deadline - time.monotonic()))
            except (OSError, subprocess.TimeoutExpired) as error:
                errors.append(str(error))
            finally:
                # Leave slot proxy and lock directories available through cleanup.
                for number in previous:
                    signal.signal(number, signal.SIG_IGN)
                stop_children(children)
    finally:
        records = set(boots.glob('*/run.json')) - before
        for role in ('host', 'join'):
            verdict[role] = guest_result(evidence / role / 'serial.txt', children.get(role), records)
        errors.extend(slots.errors)
        if (not errors and all(not verdict[role]['errors'] for role in ('host', 'join'))):
            if verdict['host']['digest'] == verdict['join']['digest']:
                verdict['verdict'] = 'PASS'
            else:
                errors.append('guest digests differ')
        with tempfile.NamedTemporaryFile(mode='w', dir=output.parent, delete=False) as stream:
            json.dump(verdict, stream, indent=2)
            stream.write('\n')
            temporary = Path(stream.name)
        temporary.replace(output)
        for number, handler in previous.items():
            signal.signal(number, handler)
    print(f"COLLAB_PAIR {verdict['verdict']} {output}")
    return 0 if verdict['verdict'] == 'PASS' else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--launcher', type=Path, default=ROOT / 'scripts/boot-interactive.sh')
    parser.add_argument('--lock-library', type=Path, default=ROOT / 'docker/qemu/lib/qemu-host-lock.sh')
    parser.add_argument('--timeout', type=float, default=300, help='pair watchdog, after lock acquisition (seconds)')
    parser.add_argument('--_locked', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not 0 < args.timeout < float('inf'):
        parser.error('--timeout must be positive and finite')
    if not os.environ.get('BREENIX_SLOT_SESSION'):
        os.execv(sys.executable, [sys.executable, str(ROOT / 'scripts/host-slots.py'),
                                 'supervise', '--', sys.executable, str(Path(__file__).resolve()), *sys.argv[1:]])
    if not args._locked:
        # The pair must acquire the host-wide domain, even if its caller used a
        # private lock. The two children get private lock directories.
        env = dict(os.environ)
        env.pop('BREENIX_QEMU_LOCK', None)
        shell = ('set -e; source "$1"; shift; trap qemu_host_lock_release EXIT; '
                 'qemu_host_lock_acquire qemu-system-aarch64; "$@"')
        os.execve('/bin/bash', ['bash', '-c', shell, 'collab-pair-lock', str(args.lock_library.resolve()),
                               sys.executable, str(Path(__file__).resolve()), '--_locked', *sys.argv[1:]], env)
    return run_pair(args)


if __name__ == '__main__':
    sys.exit(main())
