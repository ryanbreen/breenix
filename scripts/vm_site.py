#!/usr/bin/env python3
"""Private site configuration for per-run QEMU server tests."""
import argparse
import os
import secrets
from pathlib import Path
import stat
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent / 'metal'))
import metal_root

REQUIRED = ('OPERATOR_USER', 'OPERATOR_UID', 'OPERATOR_PASSWORD',
            'OPERATOR_PASSWORD_HASH', 'OPERATOR_SSH_PUBLIC_KEY',
            'CONSOLE_USER', 'CONSOLE_PASSWORD', 'SNTP_SERVERS', 'ROOT_PASSWORD_HASH')


def load_site(path=None):
    path = Path(path or os.environ.get('BREENIX_VM_SITE') or Path.home() / '.config/breenix/vm-site')
    fd = os.open(path, os.O_RDONLY | getattr(os, 'O_NOFOLLOW', 0))
    with os.fdopen(fd, 'r') as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or stat.S_IMODE(info.st_mode) != 0o600 or info.st_uid != os.getuid():
            raise ValueError('VM site file must be owned by the user and mode 0600')
        values = {}
        for raw in source:
            line = raw.strip()
            if not line or line.startswith('#'):
                continue
            key, separator, value = line.partition('=')
            if not separator or not key.strip():
                raise ValueError('invalid VM site line')
            value = value.strip()
            if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
                value = value[1:-1]
            key = key.strip()
            if key in values:
                raise ValueError('duplicate VM site field: ' + key)
            values[key] = value
    missing = [key for key in REQUIRED if not values.get(key)]
    if missing:
        raise ValueError('VM site missing fields: ' + ', '.join(missing))
    return values


def site(run, path=None):
    values = load_site(path)
    port = int((Path(run) / 'ssh-port').read_text().strip())
    if not 1 <= port <= 65535:
        raise ValueError('invalid VM SSH port')
    result = {key: values[key] for key in REQUIRED if key != 'OPERATOR_PASSWORD_HASH'}
    result.update(SERVER_SSH_HOST='127.0.0.1', SERVER_SSH_PORT=port,
                  BOOT_URL='http://10.0.2.2/', operatorKeys=metal_root.operator_keys(values))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    account = sub.add_parser('operator-account')
    account.add_argument('--out', type=Path, required=True)
    token = sub.add_parser('power-token')
    token.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    rendered = secrets.token_hex(32).encode() if args.command == 'power-token' else metal_root.operator_account(load_site()).encode()
    fd = os.open(args.out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'wb') as output:
        output.write(rendered)


if __name__ == '__main__':
    main()
