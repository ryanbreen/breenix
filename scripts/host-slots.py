#!/usr/bin/env python3
"""Host-wide flock leases for Breenix launchers; no VM is needed to test them."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import runpy
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
# x86 gate builds and metal builds run in the boot host's build containers, outside the
# x86 gate VM: each container's cpuset is the host's shared CPUs, so its builds leave the
# gate VM's reserved cores alone, and its slot directory is its own, so no lease in the
# gate VM covers them. Each build runs BUILD_JOBS Cargo jobs and peaks near BUILD_MEMORY;
# the machine a helper runs on holds as many builds as both fit (build_slots), which in
# the gate VM itself is its 8 vCPUs at 4 jobs each.
BUILD_JOBS = 4
BUILD_MEMORY = 4 * 1024 ** 3
# scripts/metal/metal-build sets its Cargo jobs from this name.
METAL_BUILD_JOBS = BUILD_JOBS


def build_slots():
    try:
        cpus = len(os.sched_getaffinity(1))  # the container's cpuset, whatever this process is pinned to
    except (AttributeError, OSError):
        cpus = os.cpu_count() or 1
    try:
        memory = int(Path('/sys/fs/cgroup/memory.max').read_text())
    except (OSError, ValueError):  # 'max' or no cgroup v2: the whole machine
        try:
            memory = os.sysconf('SC_PAGE_SIZE') * os.sysconf('SC_PHYS_PAGES')
        except (AttributeError, OSError, ValueError):
            memory = BUILD_MEMORY
    return max(1, min(cpus // BUILD_JOBS, memory // BUILD_MEMORY))


RESOURCES = {'x86-build': build_slots(), 'x86-boot': 1, 'mac-boot': 1, 'metal-build': build_slots()}
# A build inside the gate VM shares the VM's vCPUs with its boots. Builds there left the
# gate guest's vCPU threads waiting on the VM's run queue for up to ~100 ms despite the
# boot's higher priority (#1368), so a boot first holds every build lease in the VM, then
# its boot lease, and keeps both until it ends. A boot request queues for both in one
# step, so every queue orders two boots the same way, and a session that holds a build
# lease gives it up while it waits for all of them, so two such sessions never wait on
# each other. Gate builds of commits whose gate has a build phase run in the x86 build
# container (scripts/x86-gate-build.py) and take its leases, which no boot excludes; only
# builds still inside the VM (a commit older than that phase, a launcher sourcing
# docker/qemu/lib/qemu-host-lock.sh there) take the VM's leases.
# claim-lint:ok: tests/host_slots_test.py test_boot_holds_every_build_lease_until_it_ends.
EXCLUDES = {'x86-boot': 'x86-build'}
WAIT_MESSAGE_SECONDS = 60
# A guest's vCPUs are host threads. A busy host leaves them unscheduled for tens of
# milliseconds while the guest charges that time as running, and timing cases fail
# (#1368). A boot of a timing-sensitive suite therefore waits, holding its lease, until
# the host is quiet: the 1-minute load average at or below half the CPUs and, on Linux,
# no CPU leaving a lowest-priority spinner unscheduled for longer than QUIET_MAX_GAP_MS
# during a short burst. Inside the x86 gate VM that burst is the only view of the
# physical host: halted vCPUs accrue no steal, and the VM's load average omits the
# host's work. Breenix boots wait for other projects; they never limit them. The
# launcher's environment can override the policy for one invocation (quiet_policy).
# No host waits by default. On the Mac the wait held the boot lease for up to 45 minutes
# behind other projects' work and measured no smaller guest stalls (#1376, #1383); in the
# x86 gate VM it held x86-boot for 35 minutes waiting on the vCPU-gap burst while every
# other lane's boot queued behind it. Boots record their load and, on Linux, the vCPU gap
# and steal at start and verdict; a boot holding x86-boot still excludes every build in
# the gate VM. BREENIX_QUIET=1 asks for the wait for one invocation.
QUIET_WAITS_ON_THIS_HOST = False
QUIET_LOAD_FRACTION = 0.5
QUIET_MAX_GAP_MS = 10.0
QUIET_BURST_SECONDS = 0.5
QUIET_SAMPLES = 3
QUIET_SAMPLE_SECONDS = 5
QUIET_MAX_WAIT_SECONDS = 2700
# A boot's verdict is the first of these lines its serial prints; the record's
# load_at_verdict is sampled then, not when a launcher showing the result lets go.
DONE_LINE = re.compile(rb'^(?:SUITE \S+ DONE passed=|PROBE DONE\b|USERSPACE TEST REPORT DONE\b|RUN PATH DONE\b)')
VMRUN = '/Applications/VMware Fusion.app/Contents/Public/vmrun'


def command_output(argv):
    try:
        result = subprocess.run(argv, capture_output=True, text=True, errors='replace', timeout=10)
        return result.stdout.strip() if result.returncode == 0 else ''
    except (OSError, subprocess.TimeoutExpired):
        return ''


def x86_vms():
    """x86-64 QEMU guests running on this host, queued or not."""
    return [{'vm': 'qemu-system-x86_64', 'pid': pid}
            for pid in command_output(['pgrep', '-f', '^([^ ]*/)?qemu-system-x86_64( |$)']).splitlines()]


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
        self.held_number = {}
        self.held_all = {}
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

    def enqueue(self, *resources):
        # One critical section for all of a request's tickets: two requests queue for
        # every resource they share in the same order.
        with (self.directory / 'metadata.lock').open('a+') as state:
            fcntl.flock(state, fcntl.LOCK_EX)
            stamp = f'{time.monotonic_ns():020d}-{os.getpid()}'
            for resource in resources:
                path = self.directory / f'{resource}-wait-{stamp}'
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
            if resource in self.held or resource in self.held_all:
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
                self.held_number[resource] = number
                self.dequeue(resource)
                return holder
            return None

    def try_acquire_all(self, resource, identity, reason):
        """Hold every slot of `resource`, after the requests queued before this one.

        Slots are taken as they come free and kept, so requests queued later wait.
        Returns True once this session holds them all.
        """
        with (self.directory / 'metadata.lock').open('a+') as state:
            fcntl.flock(state, fcntl.LOCK_EX)
            # Waiting while holding one slot deadlocks with a second session that holds
            # another: give this session's slot up and take it back in queue order.
            self.held_number.pop(resource, None)
            own = self.held.pop(resource, None)
            if own is not None:
                own.close()
            ticket = self.tickets.get(resource)
            for path in sorted(self.directory.glob(f'{resource}-wait-*')):
                if ticket and path == ticket[0]:
                    break
                with path.open('a+') as waiter:
                    try:
                        fcntl.flock(waiter, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    except BlockingIOError:
                        return False
                    path.unlink(missing_ok=True)
            handles = self.held_all.setdefault(resource, {})
            for number in range(1, RESOURCES[resource] + 1):
                if number in handles:
                    continue
                path = self.path(resource, number)
                handle = path.open('a+')
                try:
                    fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    handle.close()
                    continue
                holder = dict(identity, resource=resource, slot=number, started=time.time(), pid=os.getpid(), reason=reason)
                temporary = path.with_suffix(f'.{os.getpid()}.tmp')
                temporary.write_text(json.dumps(holder) + '\n')
                temporary.replace(path.with_suffix('.json'))
                handles[number] = handle
            if len(handles) < RESOURCES[resource]:
                return False
            self.dequeue(resource)
            return True

    def release_all(self, resource):
        for handle in self.held_all.pop(resource, {}).values():
            handle.close()

    def release(self, resource):
        self.held_number.pop(resource, None)
        handle = self.held.pop(resource, None)
        if handle is not None:
            handle.close()  # Also happens automatically on process death, including SIGKILL.
        if resource in EXCLUDES:
            self.release_all(EXCLUDES[resource])

    def close(self):
        for resource in list(self.tickets):
            self.dequeue(resource)
        for resource in list(self.held):
            self.release(resource)
        for resource in list(self.held_all):
            self.release_all(resource)


def quiet_threshold():
    return round((os.cpu_count() or 1) * QUIET_LOAD_FRACTION, 2)


def timing_sensitive(manifest):
    """timingSensitive marks clock reads, time measurements or concurrency cases."""
    try:
        return json.loads(Path(manifest).read_text()).get('timingSensitive') is True
    except (OSError, ValueError, AttributeError):
        return False


def quiet_policy(environ):
    """One invocation's overrides of the quiet-host policy, from the launcher's environment.

    BREENIX_QUIET=1 waits whatever the manifests say and BREENIX_QUIET=0 never waits;
    BREENIX_QUIET_MAX_LOAD, BREENIX_QUIET_MAX_GAP_MS and BREENIX_QUIET_MAX_WAIT replace
    the load threshold, the Linux vCPU gap threshold and the cap in seconds.
    """
    policy = {}
    mode = environ.get('BREENIX_QUIET', '')
    if mode not in ('', '0', '1'):
        raise ValueError(f'BREENIX_QUIET must be 0 or 1, not {mode!r}')
    if mode:
        policy['required'] = mode == '1'
    for name, key in (('BREENIX_QUIET_MAX_LOAD', 'max_load'), ('BREENIX_QUIET_MAX_GAP_MS', 'max_gap_ms'),
                      ('BREENIX_QUIET_MAX_WAIT', 'max_wait_seconds')):
        if environ.get(name):
            value = float(environ[name])
            if not value >= 0:
                raise ValueError(f'{name} must be a number of at least 0, not {environ[name]!r}')
            policy[key] = value
    return policy


def cpu_counters():
    """Linux's aggregate CPU ticks: (all, steal), or None where /proc/stat is absent."""
    try:
        fields = [int(v) for v in Path('/proc/stat').read_text().split('\n', 1)[0].split()[1:9]]
    except (OSError, ValueError):
        return None
    return sum(fields), fields[7]


def vcpu_gap_ms(seconds):
    """Longest time any CPU left a lowest-priority spinner pinned to it unscheduled.

    In a VM this is how long the physical host keeps the VM's vCPUs waiting, the stall a
    guest charges as running (#1368). Linux only: it needs sched_setaffinity.
    """
    readers = []
    for cpu in sorted(os.sched_getaffinity(0)):
        read, write = os.pipe()
        pid = os.fork()
        if pid == 0:
            worst = 0
            try:
                os.close(read)
                os.sched_setaffinity(0, {cpu})
                os.nice(19)
                now = time.monotonic_ns
                last = now()
                end = last + int(seconds * 1e9)
                while last < end:
                    sample = now()
                    worst = max(worst, sample - last)
                    last = sample
                os.write(write, str(worst).encode())
            finally:
                os._exit(0)
        os.close(write)
        readers.append((pid, read))
    worst = 0
    for pid, read in readers:
        with os.fdopen(read, 'rb') as stream:
            data = stream.read()
        os.waitpid(pid, 0)
        worst = max(worst, int(data or 0))
    return round(worst / 1e6, 1)


def host_sample(policy, burst=True):
    """The host state a boot is judged and recorded by: 1-minute load, plus the vCPU gap on Linux."""
    sample = {'load': os.getloadavg()[0]}
    if burst and sys.platform.startswith('linux'):
        sample['vcpu_gap_ms'] = vcpu_gap_ms(QUIET_BURST_SECONDS)
    sample['quiet'] = sample['load'] <= policy.get('max_load', quiet_threshold()) and \
        sample.get('vcpu_gap_ms', 0) <= policy.get('max_gap_ms', QUIET_MAX_GAP_MS)
    return sample


def describe(holders):
    """Name each distinct holder once, with why it holds the lease when it says."""
    names = {}
    for h in holders:
        reason = f" ({h['reason']})" if h.get('reason') else ''
        names.setdefault(f"worktree={h['worktree']} commit={h['commit']}{reason}", h['held_seconds'])
    return '; '.join(f'{name} held={held:.1f}s' for name, held in names.items())


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


def signal_process_group(group, number, timeouts):
    try:
        os.killpg(group, number)
    except ProcessLookupError:
        return
    except PermissionError:
        # Darwin's killpg skips zombies and can return EPERM for a group
        # containing only zombies. Verify that no live member remains;
        # a real permission failure must still fail cleanup.
        table = ps_output(['ps', '-axo', 'pgid=,stat='], timeouts)
        for row in table.splitlines():
            fields = row.split()
            if len(fields) != 2:
                raise RuntimeError(f'malformed process-group row: {row!r}')
            if int(fields[0]) == group and not fields[1].startswith('Z'):
                raise


# One ps normally takes a tenth of a second, but a loaded Mac has held one past
# 10 s. While the supervisor loop runs, every ps gets one 10 s bound and a timed-out
# read, forward or QEMU stop is retried on the next turn, so cancellation and the
# stall watchdog are never more than one bound late. Cleanup after the loop has
# nothing left to watch and must not abandon a running VM over one slow ps, so it
# retries with longer bounds.
LOOP_PS_TIMEOUTS = (10,)
CLEANUP_PS_TIMEOUTS = (10, 30, 120)


def ps_output(argv, timeouts):
    for timeout in timeouts[:-1]:
        try:
            return subprocess.check_output(argv, text=True, errors='replace', timeout=timeout)
        except subprocess.TimeoutExpired:
            pass
    return subprocess.check_output(argv, text=True, errors='replace', timeout=timeouts[-1])


def process_table(timeouts):
    # Environment ownership survives double-fork and setsid on Darwin as well
    # as Linux; never print the process environment.
    rows = ps_output(['ps', 'axeww', '-o', 'pid=,ppid=,pgid=,stat=,command='], timeouts)
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
    legacy_progress = {}
    stalled_record = None
    pending = None
    received_signal = None
    child = None
    known = set()
    cargo_homes = []

    def refresh(timeouts=LOOP_PS_TIMEOUTS):
        table = process_table(timeouts)
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

    def signal_tree(number, timeouts=LOOP_PS_TIMEOUTS, done=None):
        done = set() if done is None else done
        for group in {row[1] for row in refresh(timeouts).values()} - done:
            signal_process_group(group, number, timeouts)
            done.add(group)

    def stop_qemu(timeouts=LOOP_PS_TIMEOUTS):
        targets = {pid for pid, row in refresh(timeouts).items() if Path(row[3].split()[0]).name.startswith('qemu-system')}
        for number in (signal.SIGTERM, signal.SIGKILL):
            for pid in targets:
                try:
                    os.kill(pid, number)
                except ProcessLookupError:
                    pass
            deadline = time.monotonic() + 5
            while targets.intersection(refresh(timeouts)) and time.monotonic() < deadline:
                time.sleep(0.05)
        if targets.intersection(refresh(timeouts)):
            raise RuntimeError('QEMU did not stop; keeping boot lease until cleanup')

    records = []
    bypassed = set()

    def context(count):
        return ''.join(json.dumps(entry, sort_keys=True) + '\n' for entry in records[:count]).encode()

    def save_records():
        Path(record).write_bytes(context(len(records)))

    def admit(request):
        records.append(dict(request['result']))
        save_records()
        request['client'].sendall(b'{}\n')
        request['client'].close()

    def open_boot(before=None):
        """The newest boot record without a release, among the first `before` records."""
        for index in range(min(len(records), len(records) if before is None else before) - 1, -1, -1):
            entry = records[index]
            if entry['resource'].endswith('boot') and 'load_at_release' not in entry:
                return index
        return None

    def publish(index):
        save_records()
        for serial, (count, _, _) in serials.items():
            if index < count:
                write_header(record, serial, context(count))

    counters = {}

    def start_boot(request):
        # Every launcher asks just before it starts its VM: record the host it starts
        # on, and whether a quiet host was required and found, in the open boot record.
        index = open_boot()
        if index is None:
            print('[host-slot] boot start: no boot lease is held; nothing recorded', file=sys.stderr, flush=True)
        else:
            entry = records[index]
            sample = request.get('sample') or host_sample(request['policy'])
            entry['load_at_start'] = os.getloadavg()
            if 'vcpu_gap_ms' in sample:
                entry['vcpu_gap_ms_at_start'] = sample['vcpu_gap_ms']
            entry['quiet'] = request.get('quiet_record', {'required': False})
            if cpu_counters() is not None:
                counters[index] = cpu_counters()
            publish(index)
        request['client'].sendall(b'{}\n')
        request['client'].close()

    def boot_verdict(index):
        # The serial printed its DONE line: the host the boot ran on until its verdict.
        entry = records[index]
        entry['load_at_verdict'] = os.getloadavg()
        if index in counters and cpu_counters() is not None:
            (total, steal), (total_now, steal_now) = counters[index], cpu_counters()
            entry['steal_percent_during_boot'] = round(100 * (steal_now - steal) / max(1, total_now - total), 2)
        publish(index)

    def watch_serials():
        for serial, (count, offset, partial) in list(serials.items()):
            try:
                with open(serial, 'rb') as stream:
                    size = os.fstat(stream.fileno()).st_size
                    if size < offset:
                        offset, partial = 0, b''  # The launcher truncated it for a new boot.
                    stream.seek(offset)
                    data = stream.read(1 << 22)
            except OSError:
                continue
            lines = (partial + data).split(b'\n')
            serials[serial] = (count, offset + len(data), lines.pop()[-4096:])
            if any(DONE_LINE.match(line.strip(b'\r')) for line in lines):
                index = open_boot(count)
                if index is not None:
                    boot_verdict(index)

    def register(serial):
        try:
            offset = Path(serial).stat().st_size  # Earlier boots' lines are not this boot's.
        except OSError:
            offset = 0
        serials[serial] = (len(records), offset, b'')

    def release_boot(resource):
        legacy_progress.clear()
        # The lease ends, which for Parallels and VMware can be long after the verdict.
        for index in range(len(records) - 1, -1, -1):
            entry = records[index]
            if entry['resource'] == resource and 'load_at_release' not in entry:
                entry['load_at_release'] = os.getloadavg()
                publish(index)
                return

    def quiet_host(request):
        # Holding the lease, wait until QUIET_SAMPLES samples QUIET_SAMPLE_SECONDS apart
        # all show a quiet host (host_sample), or until the cap.
        now = time.monotonic()
        if now < request['next_sample']:
            return False
        policy = request['policy']
        sample = host_sample(policy)
        request['next_sample'] = time.monotonic() + QUIET_SAMPLE_SECONDS
        request['quiet_samples'] = request['quiet_samples'] + 1 if sample['quiet'] else 0
        waited = now - request['quiet_since']
        met = request['quiet_samples'] >= QUIET_SAMPLES
        threshold = policy.get('max_load', quiet_threshold())
        cap = policy.get('max_wait_seconds', QUIET_MAX_WAIT_SECONDS)
        shown = f'load={sample["load"]:.2f} (needs <= {threshold})' + (
            f' vcpu_gap={sample["vcpu_gap_ms"]}ms (needs <= {policy.get("max_gap_ms", QUIET_MAX_GAP_MS)}ms)'
            if 'vcpu_gap_ms' in sample else '')
        if met or waited >= cap:
            request['sample'] = sample
            request['quiet_record'] = {'required': True, 'met': met, 'threshold': threshold,
                                       'wait_seconds': round(waited, 3), 'policy': policy}
            if 'vcpu_gap_ms' in sample:
                request['quiet_record']['max_gap_ms'] = policy.get('max_gap_ms', QUIET_MAX_GAP_MS)
            if met:
                print(f'[host-slot] quiet host: {shown}; waited={waited:.1f}s', file=sys.stderr, flush=True)
            else:
                print(f'[host-slot] host NOT quiet after {waited:.0f}s ({shown}); booting anyway, recorded as not quiet', file=sys.stderr, flush=True)
            return True
        if waited >= request['next_message']:
            print(f'[host-slot] waiting for a quiet host before booting: {shown} '
                  f'for {QUIET_SAMPLES} samples {QUIET_SAMPLE_SECONDS}s apart; waited={waited:.1f}s of at most {cap:.0f}s',
                  file=sys.stderr, flush=True)
            request['next_message'] = waited + WAIT_MESSAGE_SECONDS
        return False

    def forward(number, frame):
        nonlocal received_signal
        received_signal = number
        # Defer process inspection and signal forwarding to the main loop.

    for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, forward)

    # Unix socket paths are bounded (104 bytes on macOS). Nested test/build
    # TMPDIRs can exceed that limit; keep this private IPC endpoint short.
    with tempfile.TemporaryDirectory(prefix='breenix-slots-', dir='/tmp') as temporary:
        session = str(Path(temporary) / 'control.sock')
        record = str(Path(temporary) / 'queue.jsonl')
        with socket.socket(socket.AF_UNIX) as server, selectors.DefaultSelector() as selector:
            server.bind(session)
            server.listen()
            selector.register(server, selectors.EVENT_READ)
            # Run Inspector supervises a gate with its own helper, which can predate the
            # tested checkout's: name the operations newer than the first supervisor.
            env = dict(os.environ, BREENIX_SLOT_SESSION=session, BREENIX_SLOT_RECORD=record,
                       BREENIX_SLOT_OPERATIONS='quiet,number')
            child = subprocess.Popen(argv, env=env, start_new_session=True)
            deferred = []
            try:
                forwarding = forwarded_signal = None
                signaled = set()
                stop_deadline = None
                owned = {}

                def requests():
                    # A release or quiesce whose QEMU stop met a slow ps waits here for the next turn.
                    retry, deferred[:] = list(deferred), []
                    yield from retry
                    for key, _ in selector.select(timeout=0.5):
                        client, _ = key.fileobj.accept()
                        client.settimeout(10)
                        data = b''
                        while not data.endswith(b'\n'):
                            chunk = client.recv(65536)
                            if not chunk:
                                raise RuntimeError('incomplete host-slot request')
                            data += chunk
                        yield client, json.loads(data)

                while child.poll() is None:
                    try:
                        owned = refresh()
                    except subprocess.TimeoutExpired:
                        pass
                    # Older QEMU launchers use the shared lease but do not send
                    # boot-start or serial requests. Enroll their owned VM's
                    # file-backed serials at launch when the caller opts in.
                    if (os.environ.get('BREENIX_WATCH_HISTORICAL') == '1' and open_boot() is not None
                            and not any(re.search(r'/(?:watch-boot|watch-qemu|x86-gate-boot|x86-suite-boot)\.py\b', row[3])
                                        for row in owned.values())):
                        for row in owned.values():
                            command = row[3]
                            if not Path(command.split()[0]).name.startswith('qemu-system') or '-S' in command.split():
                                continue
                            paths = re.findall(r'-serial\s+file:([^\s]+)', command)
                            paths += re.findall(r'-chardev\s+file,[^\s]*path=([^,\s]+)', command)
                            helper = Path(os.environ.get('BREENIX_SLOT_PROGRESS_HELPER')
                                          or str(Path(worktree) / 'scripts/boot_progress.py'))
                            if paths and helper.exists() and row[1] not in legacy_progress:
                                Progress = runpy.run_path(str(helper))["BootProgress"]
                                for serial in paths:
                                    serial = str(Path(serial).resolve())
                                    serials[serial] = (len(records), 0, b'')
                                primary = next((path for path in paths if Path(path).name.startswith('serial_user')), paths[0])
                                legacy_progress[row[1]] = (Progress(primary, track_completion=True), paths)
                        for group, (watcher, paths) in list(legacy_progress.items()):
                            if not any(row[1] == group for row in owned.values()):
                                del legacy_progress[group]
                            else:
                                watcher.poll([(path, Path(path).name.startswith('serial_user')) for path in paths])
                    if os.getppid() != parent and received_signal is None:
                        received_signal = signal.SIGHUP
                    if received_signal is not None and received_signal != forwarded_signal:
                        if received_signal != forwarding:
                            forwarding, signaled = received_signal, set()
                            stop_deadline = time.monotonic() + 120
                        try:
                            signal_tree(received_signal, done=signaled)
                            forwarded_signal = received_signal
                        except subprocess.TimeoutExpired:
                            # The launcher's own group needs no ps: signal it now, and the
                            # rest of the tree, once each, on a later turn.
                            if child.pid not in signaled:
                                try:
                                    os.killpg(child.pid, received_signal)
                                except (ProcessLookupError, PermissionError):
                                    pass
                                signaled.add(child.pid)
                    if stop_deadline is not None and time.monotonic() >= stop_deadline:
                        try:
                            signal_tree(signal.SIGKILL)
                        except subprocess.TimeoutExpired:
                            pass  # Cleanup below kills the tree with its longer bounds.
                        break
                    for client, request in requests():
                        operation = request['operation']
                        if operation == 'acquire':
                            if pending is not None:
                                raise RuntimeError('overlapping requests from one launcher')
                            resource = request['resource']
                            if resource not in RESOURCES:
                                raise ValueError(f'unknown resource {resource}')
                            if resource in slots.held or resource in slots.held_all or resource in bypassed:
                                # A nested launcher shares the lease: the same boot, so no second record.
                                client.sendall(b'{}\n')
                                client.close()
                                continue
                            slots.enqueue(*filter(None, (EXCLUDES.get(resource), resource)))
                            pending = {'client': client, 'resource': resource,
                                       'since': time.monotonic(), 'next_message': 0,
                                       'load_at_enqueue': os.getloadavg(), 'observed': [],
                                       'exclusive': EXCLUDES.get(resource),
                                       'bypass': resource == 'mac-boot' and os.environ.get('BREENIX_BOOT_NO_QUEUE') == '1'}
                        elif operation == 'quiet':
                            if pending is not None:
                                raise RuntimeError('overlapping requests from one launcher')
                            policy = request.get('policy', {})
                            required = policy.get('required', QUIET_WAITS_ON_THIS_HOST
                                                    and any(timing_sensitive(m) for m in request['manifests']))
                            pending = {'client': client, 'resource': 'quiet', 'next_message': 0,
                                       'required': required, 'policy': policy,
                                       'quiet_since': time.monotonic(), 'quiet_samples': 0, 'next_sample': 0}
                        else:
                            if operation == 'number':
                                number = slots.held_number.get(request['resource'])
                                response = ({'slot': number} if number is not None else
                                            {'error': 'resource lease is not held'})
                                client.sendall(json.dumps(response).encode() + b'\n')
                                client.close()
                                continue
                            elif operation == 'release':
                                if request['resource'].endswith('boot'):
                                    try:
                                        stop_qemu()
                                    except subprocess.TimeoutExpired:
                                        deferred.append((client, request))
                                        continue
                                    release_boot(request['resource'])
                                bypassed.discard(request['resource'])
                                slots.release(request['resource'])
                            elif operation == 'cargo-home':
                                home = isolated_cargo_home(request['source'], request['directory'])
                                cargo_homes.append(home)
                                client.sendall(json.dumps({'path': str(home)}).encode() + b'\n')
                                client.close()
                                continue
                            elif operation == 'quiesce':
                                try:
                                    stop_qemu()
                                except subprocess.TimeoutExpired:
                                    deferred.append((client, request))
                                    continue
                            elif operation == 'vm':
                                stop_commands.append(request['stop'])
                                register(request['serial'])
                            elif operation == 'serial':
                                register(request['serial'])
                            else:
                                raise ValueError(f'unknown operation {operation}')
                            client.sendall(b'{}\n')
                            client.close()
                    watch_serials()
                    stalled = next((watcher for watcher, _ in legacy_progress.values() if watcher.stalled()), None)
                    if stalled is not None and stalled_record is None:
                        stalled_record = stalled.record()
                        index = open_boot()
                        if index is not None:
                            records[index]["stall"] = stalled_record
                            publish(index)
                        received_signal = signal.SIGTERM
                    if pending is None:
                        continue
                    if pending['resource'] == 'quiet':
                        if pending['required'] and not quiet_host(pending):
                            continue
                        start_boot(pending)
                        pending = None
                        continue
                    resource = pending['resource']
                    if pending['exclusive'] and 'exclusive_wait_seconds' not in pending:
                        waited = time.monotonic() - pending['since']
                        if not slots.try_acquire_all(pending['exclusive'], identity, f'{resource} running'):
                            for entry in slots.snapshot():
                                if entry.get('resource') == pending['exclusive'] and not any(
                                        all(old.get(k) == entry.get(k) for k in ('pid', 'resource', 'started', 'slot'))
                                        for old in pending['observed']):
                                    pending['observed'].append(entry)
                            if waited >= pending['next_message']:
                                description = describe(h for h in slots.snapshot() if h.get('resource') == pending['exclusive']) or 'earlier queued request'
                                print(f'[host-slot] waiting for every {pending["exclusive"]} lease before {resource}: {description}; waited={waited:.1f}s', file=sys.stderr, flush=True)
                                pending['next_message'] = waited + WAIT_MESSAGE_SECONDS
                            continue
                        pending['exclusive_wait_seconds'] = round(waited, 3)
                        pending['next_message'] = 0
                    holders = slots.snapshot()
                    external = (mac_vms() if resource == 'mac-boot' else x86_vms() if resource == 'x86-boot' else []) if resource not in slots.held and not any(h.get('resource') == resource for h in holders) else []
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
                            description = describe(h for h in holders if h.get('resource') == resource)
                            if external:
                                description = 'unqueued VM ' + json.dumps(external, sort_keys=True)
                            if not description:
                                description = 'earlier queued request or unavailable holder metadata'
                            print(f'[host-slot] waiting for {resource}: {description}; waited={waited:.1f}s', file=sys.stderr, flush=True)
                            pending['next_message'] = waited + WAIT_MESSAGE_SECONDS
                        continue
                    pending['result'] = {'resource': resource, 'queue_wait_seconds': round(waited, 3),
                                         **({'exclusive': pending['exclusive'], 'exclusive_wait_seconds': pending['exclusive_wait_seconds']} if pending['exclusive'] else {}),
                                         'bypass': bypass, 'load_at_enqueue': pending['load_at_enqueue'],
                                         'load_at_acquire': os.getloadavg(), 'observed_running': pending['observed'],
                                         'running_at_acquire': observed, **identity}
                    print(f'[host-slot] {"BYPASS" if bypass else "acquired"} {resource}; waited={waited:.1f}s load={os.getloadavg()}', file=sys.stderr, flush=True)
                    slots.dequeue(resource)
                    if bypass:
                        bypassed.add(resource)
                    admit(pending)
                    pending = None
            finally:
                # Clean descendants before releasing any lease, even if the shell was killed.
                if child is not None:
                    signal_tree(signal.SIGTERM, CLEANUP_PS_TIMEOUTS)
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
                    signal_tree(signal.SIGKILL, CLEANUP_PS_TIMEOUTS)
                    child.wait()
                    stop_qemu(CLEANUP_PS_TIMEOUTS)
                watch_serials()
                for resource in RESOURCES:
                    if resource.endswith('boot'):
                        release_boot(resource)
                for serial, (count, _, _) in serials.items():
                    write_header(record, serial, context(count))
                if pending is not None:
                    pending['client'].close()
                for client, _ in deferred:
                    client.close()
                for home in cargo_homes:
                    shutil.rmtree(home, ignore_errors=True)
                slots.close()
    return 124 if stalled_record else child.returncode if child.returncode >= 0 else 128 - child.returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=('supervise', 'acquire', 'release', 'number', 'quiet', 'vm', 'serial', 'header', 'cargo-home', 'quiesce'))
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
    elif args.operation == 'number':
        print(send_request({'operation': 'number', 'resource': values[0]})['slot'])
    elif args.operation == 'quiet':
        if 'quiet' not in os.environ.get('BREENIX_SLOT_OPERATIONS', '').split(','):
            print('[host-slot] the supervising helper predates boot-start records: no quiet-host wait, start not recorded',
                  file=sys.stderr, flush=True)
            return 0
        send_request({'operation': 'quiet', 'manifests': values, 'policy': quiet_policy(os.environ),
                      'progress_helper': str(Path(__file__).resolve().with_name('boot_progress.py'))})
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
