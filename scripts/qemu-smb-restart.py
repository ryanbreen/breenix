#!/usr/bin/env python3
"""Answer GUI SMB stop/start requests from a QEMU serial capture."""
import argparse
import json
import os
from pathlib import Path
import re
import runpy
import shlex
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parent
GUI_REQUEST = runpy.run_path(str(ROOT / 'metal/smb-restart'))['GUI_REQUEST']
LEASE_CHECK = '''import json
from pathlib import Path
state = Path('/var/lib/breenix-metal')
if not state.is_dir():
    raise SystemExit('boot state unavailable')
try:
    json.loads((state / 'lease.json').read_text())
except FileNotFoundError:
    print('clear')
except (OSError, ValueError):
    print('unknown')
else:
    print('claimed')
'''


def check():
    result = subprocess.run(['ssh', 'beast', 'sudo', '-n', 'incus', 'exec', 'breenix-metal', '--',
                             'python3', '-c', shlex.quote(LEASE_CHECK)], capture_output=True, text=True, timeout=30)
    if result.returncode or result.stdout.strip() != 'clear':
        raise RuntimeError('metal boot lease is active or could not be checked')


def watch(serial, instance):
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
            return subprocess.run(['ssh', 'beast', 'sudo', '-n', 'incus', 'exec', instance,
                                   '--', 'systemctl', action, 'smbd'], timeout=30).returncode
        except (OSError, subprocess.TimeoutExpired):
            return -1

    signal.signal(signal.SIGTERM, finish)
    signal.signal(signal.SIGINT, finish)
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
                if not done:
                    time.sleep(.2)
    finally:
        if stopped:
            records['cleanupStartExit'] = execute('start')
        save()
    return int(records.get('cleanupStartExit', 0) != 0 or any(not r['ok'] for r in records['requests']))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('check')
    watcher = sub.add_parser('watch')
    watcher.add_argument('--serial', type=Path, required=True)
    watcher.add_argument('--instance', required=True)
    args = parser.parse_args()
    try:
        if args.command == 'check':
            check()
            return 0
        return watch(args.serial, args.instance)
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        parser.exit(1, f'qemu-smb-restart: {error}\n')


if __name__ == '__main__':
    raise SystemExit(main())
