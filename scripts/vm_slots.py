#!/usr/bin/env python3
"""Manage disposable VM generation slots and construct persistent boot intent."""
import hashlib
import json
from pathlib import Path
import re
import sys


def read(run):
    path = Path(run) / 'slots.json'
    if not path.exists() and not path.is_symlink():
        return None
    record = json.loads(path.read_text())
    if not isinstance(record, dict) or set(record) != {'current', 'previous', 'next'}:
        raise ValueError('invalid slot record')
    return record


def entry(value):
    if not isinstance(value, dict) or set(value) != {'kernel', 'kernelSha256', 'userland'}:
        raise ValueError('missing or incomplete slot')
    kernel, digest, userland = (value[k] for k in ('kernel', 'kernelSha256', 'userland'))
    if not isinstance(kernel, str) or not re.fullmatch(r'[A-Za-z0-9_-][A-Za-z0-9_.-]*', kernel) or kernel in ('.', '..'):
        raise ValueError('invalid slot kernel')
    if not isinstance(digest, str) or not re.fullmatch(r'[0-9a-f]{64}', digest):
        raise ValueError('invalid slot kernel digest')
    if not isinstance(userland, str) or not re.fullmatch(r'[A-Za-z0-9-]{1,64}', userland):
        raise ValueError('invalid slot userland')
    return value


def select(run, slot):
    if slot not in ('current', 'next'):
        raise ValueError('invalid boot slot')
    record = read(run)
    if record is None:
        return None
    value = entry(record[slot])
    kernel = Path(run) / value['kernel']
    if kernel.is_symlink() or not kernel.is_file() or hashlib.sha256(kernel.read_bytes()).hexdigest() != value['kernelSha256']:
        raise ValueError('slot kernel missing or differs from its digest')
    return value


def intent(value):
    return f"BREENIX-BOOT 2\ntarget persistent\nuserland {entry(value)['userland']}\n".encode('ascii')


def write(run, record):
    path = Path(run) / 'slots.json'
    tmp = path.with_suffix('.json.tmp')
    tmp.write_text(json.dumps(record) + '\n')
    tmp.replace(path)


def promote(run):
    record = read(run)
    if record is None:
        raise ValueError('missing slot record')
    entry(record['current'])
    entry(record['next'])
    record['previous'], record['current'], record['next'] = record['current'], record['next'], None
    write(run, record)


def rollback(run):
    record = read(run)
    if record is None:
        raise ValueError('missing slot record')
    entry(record['current'])
    record['next'] = None
    write(run, record)


if __name__ == '__main__':
    try:
        operation, run, *args = sys.argv[1:]
        if operation == 'select' and len(args) == 1:
            value = select(run, args[0])
            if value is not None:
                (Path(run) / 'boot-intent').write_bytes(intent(value))
                print(value['kernel'])
        elif operation == 'promote' and not args:
            promote(run)
        elif operation == 'rollback' and not args:
            rollback(run)
        else:
            raise ValueError('invalid slot operation')
    except (ValueError, OSError, json.JSONDecodeError) as error:
        sys.exit(f'VM slots: {error}')
