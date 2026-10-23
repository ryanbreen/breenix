#!/usr/bin/env python3
"""Stage and answer the users suite's real OpenSSH probes on ARM64 QEMU.

BREENIX_USERS_SSH_IDENTITY names the boot host's Ed25519 private key;
BREENIX_USERS_SSH_USER optionally overrides its login name. Only the public
key is staged in the disposable guest image. The private key stays on the host.
"""
import argparse
import getpass
import json
import os
from pathlib import Path
import re
import runpy
import shutil
import signal
import socket
import subprocess
import tempfile
import time

HOSTSSH = runpy.run_path(str(Path(__file__).with_name('metal') / 'users-hostssh'))
FIXTURE = '/etc/breenix/test-ssh/operator-account'


def debugfs():
    for candidate in ('debugfs', '/usr/sbin/debugfs', '/sbin/debugfs',
                      '/opt/homebrew/opt/e2fsprogs/sbin/debugfs',
                      '/usr/local/opt/e2fsprogs/sbin/debugfs'):
        if path := shutil.which(candidate):
            return path
    raise ValueError('users host SSH staging requires e2fsprogs debugfs')


def stage(image, state):
    identity = Path(os.environ.get('BREENIX_USERS_SSH_IDENTITY', '')).expanduser().resolve()
    if not identity.is_file():
        raise ValueError('set BREENIX_USERS_SSH_IDENTITY to the boot host Ed25519 private key')
    user = os.environ.get('BREENIX_USERS_SSH_USER', getpass.getuser())
    if not re.fullmatch(r'[a-z_][a-z0-9_-]{0,31}', user) or user == 'root':
        raise ValueError('users host SSH needs a non-root account name')
    result = subprocess.run(['ssh-keygen', '-y', '-P', '', '-f', str(identity)],
                            capture_output=True, text=True, timeout=10)
    fields = result.stdout.split()
    if result.returncode or len(fields) < 2 or fields[0] != 'ssh-ed25519':
        raise ValueError('users host SSH needs an Ed25519 key available to noninteractive OpenSSH')
    fixture = f'user {user}\nkey {fields[0]} {fields[1]}\n'
    tool = debugfs()
    with tempfile.TemporaryDirectory(prefix='users-hostssh-stage-') as temporary:
        source = Path(temporary) / 'operator-account'
        source.write_text(fixture)
        source.chmod(0o600)
        commands = Path(temporary) / 'commands'
        commands.write_text(f'rm {FIXTURE}\nwrite "{source}" {FIXTURE}\n'
                            f'set_inode_field {FIXTURE} mode 0100600\n'
                            f'set_inode_field {FIXTURE} uid 0\nset_inode_field {FIXTURE} gid 0\n')
        subprocess.run([tool, '-w', '-f', str(commands), str(image)], capture_output=True, check=True)
        read = subprocess.run([tool, '-R', 'cat ' + FIXTURE, str(image)],
                              capture_output=True, check=True, text=True)
        if read.stdout != fixture:
            raise ValueError('users host SSH fixture readback differs')
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    state.write_text(json.dumps(dict(identity=str(identity), user=user, port=port)) + '\n')
    state.chmod(0o600)
    print(port)


def watch(state, serial, parent):
    config = json.loads(state.read_text())
    records = dict(requests=[])
    transcript = state.with_name('users-hostssh.json')
    answered, offset, pending = set(), 0, b''
    # subprocess.run cleans up its owned SSH child when this signal interrupts it.
    def interrupted(*_):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    try:
        with tempfile.TemporaryDirectory(prefix='users-hostssh-client-') as scratch:
            client = HOSTSSH['Client'](config['identity'], scratch)
            finished = False
            while not finished:
                try:
                    os.kill(parent, 0)
                except ProcessLookupError:
                    break
                with serial.open('rb') as stream:
                    stream.seek(offset)
                    chunk = stream.read()
                offset += len(chunk)
                pending += chunk
                *lines, pending = pending.split(b'\n')
                for raw in lines:
                    line = raw.decode('utf-8', errors='replace')
                    if HOSTSSH['DONE'].fullmatch(line):
                        finished = True
                    request = HOSTSSH['REQUEST'].fullmatch(line)
                    if not request or request.group(1, 2) in answered:
                        continue
                    answered.add(request.group(1, 2))
                    case, seq, attempt, addr, port, user, fingerprint = request.groups()
                    if user != config['user'] or port != '22':
                        record = dict(case=case, seq=int(seq), error='unexpected guest account or port')
                    else:
                        # QEMU's localhost forward reaches precisely this guest's port 22.
                        # Client still verifies the fingerprint printed by this boot, a
                        # deliberately wrong pinned key, and root's public-key refusal.
                        record = client.answer(case, seq, attempt, '127.0.0.1', str(config['port']), user, fingerprint)
                        record['guest_addr'] = addr
                    records['requests'].append(record)
                    transcript.write_text(json.dumps(records, indent=2) + '\n')
                    print(f"users-hostssh: {case} seq={seq} " + (record.get('statement') or record.get('error', 'error')), flush=True)
                if not finished:
                    time.sleep(0.2)
    except KeyboardInterrupt:
        return 130
    return 0 if records['requests'] and all(
        not r.get('error') and r.get('steps') and r['steps'][-1]['exit'] == 0
        for r in records['requests']) else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    prepare = sub.add_parser('stage')
    prepare.add_argument('--image', type=Path, required=True)
    prepare.add_argument('--state', type=Path, required=True)
    answer = sub.add_parser('watch')
    answer.add_argument('--state', type=Path, required=True)
    answer.add_argument('--serial', type=Path, required=True)
    answer.add_argument('--parent', type=int, required=True)
    args = parser.parse_args()
    try:
        if args.action == 'stage':
            stage(args.image, args.state)
            return 0
        return watch(args.state, args.serial, args.parent)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f'users-hostssh: {error}\n')


if __name__ == '__main__':
    raise SystemExit(main())
