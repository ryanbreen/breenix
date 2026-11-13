#!/usr/bin/env python3
"""Answer GUI SMB stop/start requests from a QEMU serial capture."""
import argparse
import json
import os
from pathlib import Path
import re
import runpy
import select
import shlex
import signal
import sys
import subprocess
import time

ROOT = Path(__file__).resolve().parent
GUI_REQUEST = runpy.run_path(str(ROOT / 'metal/smb-restart'))['GUI_REQUEST']
# This is the same admission lock claim_lease takes before inspecting lease.json.
# Hold it until the QEMU process and SMB recovery have both finished. Do not
# create a machine lease: QEMU is not a powered metal boot or a reaper client.
LEASE_CHECK = '''import fcntl, os, sys
from pathlib import Path
state = Path(os.environ.get('BREENIX_METAL_STATE', '/var/lib/breenix-metal'))
if not state.is_dir():
    raise SystemExit('boot state unavailable')
with (state / 'lease.lock').open('a') as lock:
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    if (state / 'lease.json').exists():
        raise SystemExit('metal boot lease is active')
    print('clear', flush=True)
    sys.stdin.read()
'''

# Run on the boot host. Service actions and final recovery belong to this
# connection, so an EOF restores smbd before releasing machine admission.
REMOTE = '''import os, select, signal, subprocess, sys
if EXPECTED_INSTANCE is not None and os.environ['SMB_TEST_INSTANCE'] != EXPECTED_INSTANCE:
    raise SystemExit('SMB site differs from boot host')
lock = subprocess.Popen(['sudo', '-n', 'incus', 'exec', os.environ['METAL_INSTANCE'],
                         '--', 'python3', '-u', '-c', LOCK_CODE],
                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
stopped = False
def execute(action):
    return subprocess.run(['sudo', '-n', 'incus', 'exec', os.environ['SMB_TEST_INSTANCE'],
                           '--', 'systemctl', action, 'smbd'],
                          stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                          stderr=subprocess.DEVNULL, timeout=30).returncode
def finish(*args):
    raise SystemExit(143)
signal.signal(signal.SIGTERM, finish)
try:
    if lock.stdout.readline().strip() != 'clear':
        raise SystemExit('metal admission unavailable')
    print('clear', flush=True)
    while lock.poll() is None:
        if not select.select([sys.stdin], [], [], .2)[0]:
            continue
        line = sys.stdin.readline()
        if not line:
            break
        action = line.strip()
        if action not in ('start', 'stop') or lock.poll() is not None:
            raise SystemExit('invalid request or lost admission')
        if action == 'stop': stopped = True
        status = execute(action)
        if action == 'start' and status == 0: stopped = False
        print(status, flush=True)
finally:
    if stopped and lock.poll() is None:
        execute('start')
    lock.stdin.close()
    lock.wait()
'''


class Admission:
    def __init__(self, instance=None):
        # The SSH alias lives with the QEMU site's other private SMB settings;
        # the container names come from the boot host's authoritative host.env.
        sys.path.insert(0, str(ROOT))
        from vm_site import load_site
        host = load_site()['SMB_TEST_SSH_HOST']
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]{0,252}', host):
            raise ValueError('invalid SMB SSH host')
        program = ('LOCK_CODE = ' + repr(LEASE_CHECK) + '\n'
                   + 'EXPECTED_INSTANCE = ' + repr(instance) + '\n' + REMOTE)
        command = ('set -a; . ~/.config/breenix-metal/host.env; set +a; '
                   'exec python3 -u -c ' + shlex.quote(program))
        self.process = subprocess.Popen(['ssh', '-S', 'none', '-o', 'BatchMode=yes',
                                         '-o', 'ServerAliveInterval=5', '-o', 'ServerAliveCountMax=3',
                                         host, command], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        try:
            if self.reply() != 'clear':
                raise RuntimeError('metal boot lease or admission lock is active or unavailable')
        except BaseException:
            self.close()
            raise

    def reply(self):
        if not select.select([self.process.stdout], [], [], 35)[0]:
            raise RuntimeError('SMB admission response timed out')
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError('SMB admission connection closed')
        return line.strip()

    def execute(self, action):
        self.process.stdin.write(action + '\n')
        self.process.stdin.flush()
        return int(self.reply())

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=40)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            self.process.wait(timeout=5)
        finally:
            self.process.stdout.close()


def check():
    admission = Admission()
    admission.close()


def watch(serial, instance, admission, process=None):
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]{0,62}', instance):
        raise ValueError('invalid SMB instance')
    records = {'requests': []}
    path = Path(str(serial) + '.smb-restart.json')
    stopped = False
    done = False
    answered = set()

    def finish(_signum, _frame):
        nonlocal done
        done = True

    def save():
        temporary = path.with_name(path.name + '.tmp')
        temporary.write_text(json.dumps(records, indent=2) + '\n')
        temporary.replace(path)

    def execute(action):
        try:
            return admission.execute(action)
        except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
            return -1

    signal.signal(signal.SIGTERM, finish)
    signal.signal(signal.SIGINT, finish)
    signal.signal(signal.SIGHUP, finish)
    save()
    try:
        with serial.open('rb') as source:
            pending = b''
            while not done:
                pending += source.read()
                lines = pending.split(b'\n')
                pending = lines.pop()
                for raw in lines:
                    match = GUI_REQUEST.fullmatch(raw.decode('utf-8', errors='replace'))
                    if not match:
                        continue
                    action, seq = match.group(1).lower(), int(match.group(2))
                    if seq in answered:
                        continue
                    answered.add(seq)
                    record = dict(case='guskit/smb-share-gone', seq=seq, action=action, ok=False)
                    records['requests'].append(record)
                    save()
                    if action == 'stop':
                        stopped = True
                    record['exit'] = execute(action)
                    record['ok'] = record['exit'] == 0
                    if action == 'start' and record['ok']:
                        stopped = False
                    save()
                if admission.process.poll() is not None:
                    raise RuntimeError('SMB admission lost during QEMU boot')
                if process is not None and process.poll() is not None:
                    done = True
                if not done:
                    time.sleep(.2)
    finally:
        # Keep admission through the launcher's traps and QEMU shutdown.
        if process is not None and process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=40)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
        if stopped:
            records['cleanupStartExit'] = execute('start')
        save()
    return int(records.get('cleanupStartExit', 0) != 0 or any(not r['ok'] for r in records['requests']))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('check')
    for name in ('watch', 'run'):
        watcher = sub.add_parser(name)
        watcher.add_argument('--serial', type=Path, required=True)
        watcher.add_argument('--instance', required=True)
        if name == 'run':
            watcher.add_argument('argv', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    try:
        if args.command == 'check':
            check()
            return 0
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]{0,62}', args.instance):
            raise ValueError('invalid SMB instance')
        admission = Admission(args.instance)
        process = None
        try:
            if args.command == 'run':
                command = args.argv[1:] if args.argv[:1] == ['--'] else args.argv
                if not command:
                    raise ValueError('run needs a launcher command')
                args.serial.parent.mkdir(parents=True, exist_ok=True)
                args.serial.touch()
                process = subprocess.Popen(command, start_new_session=True,
                    env=dict(os.environ, BREENIX_QEMU_SMB_GUARDED='1'))
            status = watch(args.serial, args.instance, admission, process)
            return status or (process.returncode if process is not None else 0)
        finally:
            admission.close()
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        parser.exit(1, f'qemu-smb-restart: {error}\n')


if __name__ == '__main__':
    raise SystemExit(main())
