#!/usr/bin/env python3
"""Host-wide flock leases for Breenix launchers; no VM is needed to test them."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import selectors
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

# Never checkout-relative or configurable by a launcher's environment.
SLOT_DIR = Path('/tmp/breenix-host-slots')
RESOURCES = {'x86-build': 2, 'x86-boot': 1, 'mac-boot': 1}
WAIT_MESSAGE_SECONDS = 60
VMRUN = '/Applications/VMware Fusion.app/Contents/Public/vmrun'


def command_output(argv):
    try:
        result = subprocess.run(argv, capture_output=True, text=True, errors='replace', timeout=10)
        return result.stdout.strip() if result.returncode == 0 else ''
    except (OSError, subprocess.TimeoutExpired):
        return ''


def mac_vms():
    """Include older/manual launches that do not yet participate in the queue."""
    running = []
    for binary in ('qemu-system-aarch64', 'qemu-system-x86_64'):
        for pid in command_output(['pgrep', '-x', binary]).splitlines():
            running.append({'vm': binary, 'pid': pid})
    if shutil.which('prlctl'):
        for row in command_output(['prlctl', 'list', '--all']).splitlines():
            if 'breenix' in row.lower() and any(state in row.lower().split() for state in ('running', 'paused', 'suspended', 'stopping')):
                running.append({'vm': 'parallels', 'detail': row})
    if Path(VMRUN).exists():
        for row in command_output([VMRUN, 'list']).splitlines():
            if 'breenix' in row.lower():
                running.append({'vm': 'vmware', 'detail': row})
    return running


class Slots:
    def __init__(self, directory=None):
        self.directory = Path(directory if directory is not None else SLOT_DIR)
        self.directory.mkdir(parents=True, exist_ok=True)
        self.held = {}
        self.tickets = {}

    def path(self, resource, number):
        return self.directory / f'{resource}-{number}.lock'

    def snapshot(self):
        with (self.directory / 'metadata.lock').open('a+') as state:
            fcntl.flock(state, fcntl.LOCK_SH)
            holders = []
            for resource, count in RESOURCES.items():
                for number in range(1, count + 1):
                    path = self.path(resource, number)
                    with path.open('a+') as handle:
                        try:
                            # Observers share their probe; only an exclusive lease
                            # blocks it. Two observers must not report each other.
                            fcntl.flock(handle, fcntl.LOCK_SH | fcntl.LOCK_NB)
                        except BlockingIOError:
                            try:
                                holder = json.loads(path.with_suffix('.json').read_text())
                            except (OSError, ValueError):
                                holder = {'resource': resource, 'worktree': 'publishing', 'commit': 'unknown'}
                            holder = dict({'resource': resource, 'worktree': 'unknown', 'commit': 'unknown'}, **holder) if isinstance(holder, dict) else {'resource': resource, 'worktree': 'unknown', 'commit': 'unknown'}
                            holder['resource'] = resource
                            started = holder.get('started')
                            holder['held_seconds'] = round(max(0, time.time() - started), 1) if isinstance(started, (int, float)) else 0.0
                            holders.append(holder)
            return holders

    def enqueue(self, resource):
        with (self.directory / 'metadata.lock').open('a+') as state:
            fcntl.flock(state, fcntl.LOCK_EX)
            path = self.directory / f'{resource}-wait-{time.monotonic_ns():020d}-{os.getpid()}'
            handle = path.open('x+')
            fcntl.flock(handle, fcntl.LOCK_EX)
            self.tickets[resource] = (path, handle)

    def dequeue(self, resource):
        ticket = self.tickets.pop(resource, None)
        if ticket:
            ticket[0].unlink(missing_ok=True)
            ticket[1].close()

    def try_acquire(self, resource, identity):
        with (self.directory / 'metadata.lock').open('a+') as state:
            fcntl.flock(state, fcntl.LOCK_EX)
            if resource in self.held:
                return {'resource': resource}  # Nested launchers share the lease.
            ticket = self.tickets.get(resource)
            for path in sorted(self.directory.glob(f'{resource}-wait-*')):
                if ticket and path == ticket[0]:
                    break
                with path.open('a+') as waiter:
                    try:
                        fcntl.flock(waiter, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    except BlockingIOError:
                        return None
                    path.unlink(missing_ok=True)

            for number in range(1, RESOURCES[resource] + 1):
                path = self.path(resource, number)
                handle = path.open('a+')
                try:
                    fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    handle.close()
                    continue
                holder = dict(identity, resource=resource, slot=number, started=time.time(), pid=os.getpid())
                # The lock inode is permanent. Metadata is replaced atomically under its lock.
                temporary = path.with_suffix(f'.{os.getpid()}.tmp')
                temporary.write_text(json.dumps(holder) + '\n')
                temporary.replace(path.with_suffix('.json'))
                self.held[resource] = handle
                self.dequeue(resource)
                return holder
            return None

    def release(self, resource):
        handle = self.held.pop(resource, None)
        if handle is not None:
            handle.close()  # Also happens automatically on process death, including SIGKILL.

    def close(self):
        for resource in list(self.tickets):
            self.dequeue(resource)
        for resource in list(self.held):
            self.release(resource)


def write_header(record, serial, context=None):
    path = Path(serial)
    if not path.exists() or (context is None and not Path(record).exists()):
        return
    # Queue metadata is a sidecar, outside guest serial and scorer inputs.
    context = Path(record).read_bytes() if context is None else context
    path.with_name(path.name + '.host-slots.jsonl').write_bytes(context)


def isolated_cargo_home(source, directory):
    """Use fresh Cargo tracking data; download dependencies instead of copying live caches."""
    source = Path(source)
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    destination = Path(tempfile.mkdtemp(prefix='gate-cargo-home.', dir=directory))
    try:
        for name in ('config', 'config.toml', 'credentials', 'credentials.toml'):
            entry = source / name
            if entry.is_file():
                shutil.copy2(entry, destination / name)
    except BaseException:
        shutil.rmtree(destination)
        raise
    return destination


def send_request(request):
    with socket.socket(socket.AF_UNIX) as client:
        client.connect(os.environ['BREENIX_SLOT_SESSION'])
        client.sendall(json.dumps(request).encode() + b'\n')
        response = b''
        while not response.endswith(b'\n'):
            chunk = client.recv(65536)
            if not chunk:
                raise RuntimeError('host-slot supervisor exited before replying')
            response += chunk
        result = json.loads(response)
        if 'error' in result:
            raise RuntimeError(result['error'])
        return result


def signal_process_group(group, number):
    try:
        os.killpg(group, number)
    except ProcessLookupError:
        return
    except PermissionError:
        # Darwin's killpg skips zombies and can return EPERM for a group
        # containing only zombies. Verify that no live member remains;
        # a real permission failure must still fail cleanup.
        table = subprocess.check_output(['ps', '-axo', 'pgid=,stat='], text=True, timeout=10)
        for row in table.splitlines():
            fields = row.split()
            if len(fields) != 2:
                raise RuntimeError(f'malformed process-group row: {row!r}')
            if int(fields[0]) == group and not fields[1].startswith('Z'):
                raise


def process_table():
    # Environment ownership survives double-fork and setsid on Darwin as well
    # as Linux; never print the process environment.
    rows = subprocess.check_output(['ps', 'axeww', '-o', 'pid=,ppid=,pgid=,stat=,command='],
                                   text=True, errors='replace', timeout=10)
    result = {}
    for row in rows.splitlines():
        fields = row.split(None, 4)
        if len(fields) == 5:
            result[int(fields[0])] = (int(fields[1]), int(fields[2]), fields[3], fields[4])
    return result


def supervise(argv):
    # The foreground handle owns no lease. Its worker survives SIGKILL long
    # enough to stop descendants and daemon VMs before closing the lease.
    parent = os.getpid()
    worker = os.fork()
    if worker == 0:
        os.setsid()
        try:
            status = supervise_worker(argv, parent)
        except BaseException:
            import traceback
            traceback.print_exc()
            status = 1
        os._exit(status)
    def forward(number, frame):
        try:
            os.kill(worker, number)
        except ProcessLookupError:
            pass
    for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, forward)
    _, status = os.waitpid(worker, 0)
    return os.waitstatus_to_exitcode(status)


def supervise_worker(argv, parent):
    slots = Slots()
    worktree = os.environ.get('BREENIX_SLOT_WORKTREE', str(Path(__file__).resolve().parents[1]))
    identity = {'worktree': worktree, 'commit': os.environ.get('BREENIX_SLOT_COMMIT') or command_output(['git', '-C', worktree, 'rev-parse', 'HEAD']) or 'unknown'}
    stop_commands = []
    serials = {}
    pending = None
    received_signal = None
    child = None
    known = set()
    cargo_homes = []

    def refresh():
        table = process_table()
        known.update(pid for pid, row in table.items() if f'BREENIX_SLOT_SESSION={session}' in row[3].split())
        if child is not None:
            known.add(child.pid)
        changed = True
        while changed:
            found = {pid for pid, row in table.items() if row[0] in known}
            changed = not found.issubset(known)
            known.update(found)
        known.intersection_update(table)
        return {pid: row for pid, row in table.items() if pid in known and not row[2].startswith('Z')}

    def signal_tree(number):
        for group in {row[1] for row in refresh().values()}:
            signal_process_group(group, number)

    def stop_qemu():
        targets = {pid for pid, row in refresh().items() if Path(row[3].split()[0]).name.startswith('qemu-system')}
        for number in (signal.SIGTERM, signal.SIGKILL):
            for pid in targets:
                try:
                    os.kill(pid, number)
                except ProcessLookupError:
                    pass
            deadline = time.monotonic() + 5
            while targets.intersection(refresh()) and time.monotonic() < deadline:
                time.sleep(0.05)
        if targets.intersection(refresh()):
            raise RuntimeError('QEMU did not stop; keeping boot lease until cleanup')

    def forward(number, frame):
        nonlocal received_signal
        received_signal = number
        # Defer process inspection and signal forwarding to the main loop.

    for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, forward)

    with tempfile.TemporaryDirectory(prefix='breenix-slots-') as temporary:
        session = str(Path(temporary) / 'control.sock')
        record = str(Path(temporary) / 'queue.jsonl')
        with socket.socket(socket.AF_UNIX) as server, selectors.DefaultSelector() as selector:
            server.bind(session)
            server.listen()
            selector.register(server, selectors.EVENT_READ)
            env = dict(os.environ, BREENIX_SLOT_SESSION=session, BREENIX_SLOT_RECORD=record)
            child = subprocess.Popen(argv, env=env, start_new_session=True)
            try:
                forwarded_signal = None
                stop_deadline = None
                while child.poll() is None:
                    refresh()
                    if os.getppid() != parent and received_signal is None:
                        received_signal = signal.SIGHUP
                    if received_signal is not None and received_signal != forwarded_signal:
                        signal_tree(received_signal)
                        forwarded_signal = received_signal
                        stop_deadline = time.monotonic() + 120
                    if stop_deadline is not None and time.monotonic() >= stop_deadline:
                        signal_tree(signal.SIGKILL)
                        break
                    for key, _ in selector.select(timeout=0.5):
                        client, _ = key.fileobj.accept()
                        client.settimeout(10)
                        data = b''
                        while not data.endswith(b'\n'):
                            chunk = client.recv(65536)
                            if not chunk:
                                raise RuntimeError('incomplete host-slot request')
                            data += chunk
                        request = json.loads(data)
                        operation = request['operation']
                        if operation == 'acquire':
                            if pending is not None:
                                raise RuntimeError('overlapping requests from one launcher')
                            resource = request['resource']
                            if resource not in RESOURCES:
                                raise ValueError(f'unknown resource {resource}')
                            slots.enqueue(resource)
                            pending = {'client': client, 'resource': resource,
                                       'since': time.monotonic(), 'next_message': 0,
                                       'load_at_enqueue': os.getloadavg(), 'observed': [],
                                       'bypass': resource == 'mac-boot' and os.environ.get('BREENIX_BOOT_NO_QUEUE') == '1'}
                        else:
                            if operation == 'release':
                                if request['resource'].endswith('boot'):
                                    stop_qemu()
                                slots.release(request['resource'])
                            elif operation == 'cargo-home':
                                home = isolated_cargo_home(request['source'], request['directory'])
                                cargo_homes.append(home)
                                client.sendall(json.dumps({'path': str(home)}).encode() + b'\n')
                                client.close()
                                continue
                            elif operation == 'quiesce':
                                stop_qemu()
                            elif operation == 'vm':
                                stop_commands.append(request['stop'])
                                serials[request['serial']] = Path(record).read_bytes()
                            elif operation == 'serial':
                                serials[request['serial']] = Path(record).read_bytes()
                            else:
                                raise ValueError(f'unknown operation {operation}')
                            client.sendall(b'{}\n')
                            client.close()
                    if pending is None:
                        continue
                    resource = pending['resource']
                    holders = slots.snapshot()
                    external = (mac_vms() if resource == 'mac-boot' else [{'vm': 'qemu-system-x86_64', 'pid': pid} for pid in command_output(['pgrep', '-f', '^([^ ]*/)?qemu-system-x86_64( |$)']).splitlines()] if resource == 'x86-boot' else []) if resource not in slots.held and not any(h.get('resource') == resource for h in holders) else []
                    bypass = pending['bypass']
                    holder = None if external and not bypass else slots.try_acquire(resource, identity) if not bypass else {}
                    if holder is None:
                        # A peer can acquire while VM discovery runs. Refresh the
                        # snapshot after contention, before naming the holder.
                        holders = slots.snapshot()
                        if any(h.get('resource') == resource for h in holders):
                            external = []
                    observed = {'holders': holders, 'vms': external}
                    # Keep each holder identity once, with the age when first observed.
                    for entry in holders + external:
                        if not any(all(old.get(k) == entry.get(k) for k in ('pid', 'resource', 'started', 'vm', 'detail'))
                                   for old in pending['observed']):
                            pending['observed'].append(entry)
                    waited = time.monotonic() - pending['since']
                    if holder is None:
                        if waited >= pending['next_message']:
                            blockers = [h for h in holders if h.get('resource') == resource]
                            description = '; '.join(f"worktree={h['worktree']} commit={h['commit']} held={h['held_seconds']:.1f}s" for h in blockers )
                            if external:
                                description = 'unqueued VM ' + json.dumps(external, sort_keys=True)
                            if not description:
                                description = 'earlier queued request or unavailable holder metadata'
                            print(f'[host-slot] waiting for {resource}: {description}; waited={waited:.1f}s', file=sys.stderr, flush=True)
                            pending['next_message'] = waited + WAIT_MESSAGE_SECONDS
                        continue
                    result = {'resource': resource, 'queue_wait_seconds': round(waited, 3),
                              'bypass': bypass, 'load_at_enqueue': pending['load_at_enqueue'],
                              'load_at_acquire': os.getloadavg(), 'observed_running': pending['observed'],
                              'running_at_acquire': observed, **identity}
                    with open(record, 'a') as output:
                        output.write(json.dumps(result, sort_keys=True) + '\n')
                    print(f'[host-slot] {"BYPASS" if bypass else "acquired"} {resource}; waited={waited:.1f}s load={os.getloadavg()}', file=sys.stderr, flush=True)
                    slots.dequeue(resource)
                    pending['client'].sendall(b'{}\n')
                    pending['client'].close()
                    pending = None
            finally:
                # Clean descendants before releasing any lease, even if the shell was killed.
                if child is not None:
                    signal_tree(signal.SIGTERM)
                for command in stop_commands:
                    try:
                        subprocess.run(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
                    except (OSError, subprocess.TimeoutExpired) as error:
                        print(f'[host-slot] VM cleanup failed: {error}', file=sys.stderr)
                if child is not None:
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        pass
                    signal_tree(signal.SIGKILL)
                    child.wait()
                    stop_qemu()
                for serial, context in serials.items():
                    write_header(record, serial, context)
                if pending is not None:
                    pending['client'].close()
                for home in cargo_homes:
                    shutil.rmtree(home, ignore_errors=True)
                slots.close()
    return child.returncode if child.returncode >= 0 else 128 - child.returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=('supervise', 'acquire', 'release', 'vm', 'serial', 'header', 'cargo-home', 'quiesce'))
    parser.add_argument('arguments', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    values = args.arguments
    if args.operation == 'cargo-home':
        print(send_request({'operation': 'cargo-home', 'source': values[0], 'directory': values[1]})['path'])
        return 0
    if args.operation == 'supervise':
        return supervise(values[1:] if values[0] == '--' else values)
    if args.operation in ('acquire', 'release'):
        send_request({'operation': args.operation, 'resource': values[0]})
    elif args.operation == 'vm':
        send_request({'operation': 'vm', 'serial': values[0], 'stop': values[1:]})
    elif args.operation == 'serial':
        send_request({'operation': 'serial', 'serial': values[0]})
    elif args.operation == 'quiesce':
        send_request({'operation': 'quiesce'})
    elif args.operation == 'header':
        write_header(os.environ['BREENIX_SLOT_RECORD'], values[0])
    return 0


if __name__ == '__main__':
    sys.exit(main())
