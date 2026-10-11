#!/usr/bin/env python3
"""Reserve host CPU time for an x86 gate VM's vCPUs while its gate boots (#1368).

Runs on the VM's host, as root, around the command that runs the gate inside the VM:

    x86-host-reserve.py [--remove-self] --instance NAME -- COMMAND...

COMMAND's output is copied through unchanged. A guest's vCPUs are host threads, and other
work on a busy host left them unscheduled for up to ~50 ms while the guest charged that time
as running. When the gate prints that its boot phase started, every vCPU thread of the
instance gets SCHED_DEADLINE with RUNTIME_NS of every PERIOD_NS: it preempts all ordinary
host work, a continuously busy vCPU still leaves its host CPU the rest of each period, and
it waits at most about that long in any period. The reservation ends when the boot phase
ends or COMMAND exits. Helpers of concurrent gates share one reservation: the last holder
to leave removes it, and a helper finding a reservation with no holder (one left by a killed
helper) removes it before starting.
"""
import argparse
import fcntl
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

RUNTIME_NS = 95_000_000
PERIOD_NS = 100_000_000
SCHED_DEADLINE = 6
KVM_RUN = '0xae80'
BOOT_STARTED = b'[gate-phase] phase=boot started='
BOOT_ENDED = b'[gate-phase] phase=boot ended='
LOCK_DIR = Path('/run/lock')


def say(text):
    os.write(sys.stdout.fileno(), f'[x86-reserve] {text}\n'.encode())


def instance_pid(instance):
    result = subprocess.run(['incus', 'info', instance], capture_output=True, text=True, timeout=30)
    for line in result.stdout.splitlines():
        if line.startswith('PID:'):
            return int(line.split()[1])
    return None


def threads(pid):
    try:
        return [int(tid) for tid in os.listdir(f'/proc/{pid}/task')]
    except OSError:
        return []


def read(path):
    try:
        return Path(path).read_text()
    except OSError:
        return ''


def vcpu_threads(pid):
    """The threads that QEMU runs vCPUs on: those found in the KVM_RUN ioctl."""
    name = read(f'/proc/{pid}/comm').strip()
    found = set()
    for _ in range(5):
        for tid in threads(pid):
            fields = read(f'/proc/{pid}/task/{tid}/syscall').split()
            if read(f'/proc/{pid}/task/{tid}/comm').strip() == name and fields[:1] == ['16'] and fields[2:3] == [KVM_RUN]:
                found.add(tid)
        time.sleep(0.02)
    return sorted(found)


def policy(tid):
    try:
        return os.sched_getscheduler(tid)
    except OSError:
        return None


def set_deadline(tid):
    return subprocess.run(['chrt', '--deadline', '--sched-runtime', str(RUNTIME_NS), '--sched-deadline', str(PERIOD_NS),
                           '--sched-period', str(PERIOD_NS), '--pid', '0', str(tid)], capture_output=True, text=True)


def set_other(tid):
    return subprocess.run(['chrt', '--other', '--pid', '0', str(tid)], capture_output=True, text=True)


def reserve(instance):
    pid = instance_pid(instance)
    vcpus = vcpu_threads(pid) if pid else []
    failed = []
    for tid in vcpus:
        if policy(tid) != SCHED_DEADLINE:
            result = set_deadline(tid)
            if result.returncode != 0:
                failed.append(f'{tid}: {result.stderr.strip()}')
    reserved = sum(1 for tid in vcpus if policy(tid) == SCHED_DEADLINE)
    if not vcpus or failed:
        say(f'NOT reserved for {instance}: {reserved} of {len(vcpus)} vCPU threads hold SCHED_DEADLINE; '
            + ('; '.join(failed) if failed else 'no vCPU threads found'))
    else:
        say(f'reserved {reserved} vCPU threads of {instance}: SCHED_DEADLINE runtime={RUNTIME_NS // 1_000_000}ms '
            f'period={PERIOD_NS // 1_000_000}ms at {time.time():.3f}')


def release(instance):
    pid = instance_pid(instance)
    released = 0
    for tid in threads(pid) if pid else []:
        if policy(tid) == SCHED_DEADLINE:
            result = set_other(tid)
            if result.returncode == 0:
                released += 1
            else:
                say(f'could not release thread {tid} of {instance}: {result.stderr.strip()}')
    if released:
        say(f'released {released} vCPU threads of {instance} at {time.time():.3f}')


class Reservation:
    """One helper's hold on the instance's shared reservation."""

    def __init__(self, instance, directory=None):
        self.instance = instance
        directory = Path(directory or LOCK_DIR)
        self.state_path = directory / f'breenix-x86-reserve-{instance}.state'
        self.holders_path = directory / f'breenix-x86-reserve-{instance}.holders'
        self.held = None

    def _state(self):
        state = self.state_path.open('a+')
        fcntl.flock(state, fcntl.LOCK_EX)
        return state

    def _unheld(self):
        probe = self.holders_path.open('a+')
        try:
            fcntl.flock(probe, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
        except BlockingIOError:
            return False
        finally:
            probe.close()

    def clear_stale(self):
        with self._state():
            if self._unheld():
                release(self.instance)

    def take(self):
        if self.held is not None:
            return
        with self._state():
            self.held = self.holders_path.open('a+')
            fcntl.flock(self.held, fcntl.LOCK_SH)
            reserve(self.instance)

    def leave(self):
        if self.held is None:
            return
        with self._state():
            self.held.close()
            self.held = None
            if self._unheld():
                release(self.instance)


def relay(command, reservation):
    """Run command, copying its output and taking the reservation for each boot phase."""
    child = subprocess.Popen(command, stdout=subprocess.PIPE)

    def forward(number, frame):
        try:
            child.send_signal(number)
        except ProcessLookupError:
            pass

    for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, forward)
    pending = b''
    try:
        while True:
            try:
                chunk = os.read(child.stdout.fileno(), 65536)
            except InterruptedError:
                continue
            if not chunk:
                break
            try:
                os.write(sys.stdout.fileno(), chunk)
            except BrokenPipeError:
                # The launcher went away: let COMMAND see the broken pipe it would have
                # seen without this helper, and keep the reservation until COMMAND exits.
                child.stdout.close()
                break
            pending += chunk
            *lines, pending = pending.split(b'\n')
            for line in lines:
                if BOOT_STARTED in line:
                    reservation.take()
                elif BOOT_ENDED in line:
                    reservation.leave()
        return child.wait()
    finally:
        reservation.leave()
        if child.poll() is None:
            child.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--instance', required=True)
    parser.add_argument('--remove-self', action='store_true', help='delete this file once loaded (a per-run copy)')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('a command to run is required')
    if args.remove_self:
        Path(__file__).unlink(missing_ok=True)
    reservation = Reservation(args.instance)
    reservation.clear_stale()
    status = relay(command, reservation)
    return status if status >= 0 else 128 - status


if __name__ == '__main__':
    sys.exit(main())
