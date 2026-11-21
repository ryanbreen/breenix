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
    fields = {'kernel', 'kernelSha256', 'userland'}
    if not isinstance(value, dict) or set(value) not in (fields, fields | {'layout'}):
        raise ValueError('missing or incomplete slot')
    kernel, digest, userland = (value[k] for k in ('kernel', 'kernelSha256', 'userland'))
    if not isinstance(kernel, str) or not re.fullmatch(r'[A-Za-z0-9_-][A-Za-z0-9_.-]*', kernel) or kernel in ('.', '..'):
        raise ValueError('invalid slot kernel')
    if not isinstance(digest, str) or not re.fullmatch(r'[0-9a-f]{64}', digest):
        raise ValueError('invalid slot kernel digest')
    if not isinstance(userland, str) or not re.fullmatch(r'[A-Za-z0-9-]{1,64}', userland):
        raise ValueError('invalid slot userland')
    if value.get('layout', 'ext2') not in ('ext2', 'ext2-rescue', 'breenfs'):
        raise ValueError('invalid slot layout')
    return value


def select(run, slot):
    if slot not in ('current', 'previous', 'next'):
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
    value = entry(value)
    if value.get('layout', 'ext2') != 'ext2':
        raise ValueError('migration slot requires a format 3 intent')
    return f"BREENIX-BOOT 2\ntarget persistent\nuserland {value['userland']}\n".encode('ascii')


def migration_intent(phase, userland, guids):
    sys.path.insert(0, str(Path(__file__).with_name('metal')))
    from metal_persist import intent as format_intent

    if phase == 'migrate':
        target = 'persistent-migrate'
        mounts = [('test-root', '/', False), ('system', '/migrate/ext2-system', False),
                  ('home', '/migrate/ext2-home', False), ('breenfs-system', '/migrate/system', True),
                  ('breenfs-home', '/migrate/home', True)]
    elif phase in ('cutover', 'return'):
        target = 'persistent'
        mounts = [('breenfs-system', '/', True), ('breenfs-home', '/home', True)]
    elif phase == 'rescue':
        target = 'persistent'
        mounts = [('system', '/', False), ('home', '/home', False)]
    else:
        raise ValueError('invalid migration phase')
    return format_intent(target, userland=userland,
                         mounts=[(guids[role], path, writable) for role, path, writable in mounts],
                         mount_guids=guids)


def migration_slot(phase, promoted):
    if phase == 'cutover' and not promoted:
        return 'next'
    if phase == 'rescue' and promoted:
        return 'previous'
    return 'current'


def write_json(path, record):
    tmp = path.with_suffix('.json.tmp')
    tmp.write_text(json.dumps(record) + '\n')
    tmp.replace(path)


def write(run, record):
    write_json(Path(run) / 'slots.json', record)


def promote(run):
    record = read(run)
    if record is None:
        raise ValueError('missing slot record')
    # Only the first install has neither a current nor a previous generation.
    # An established record must still have a complete current slot.
    if record['current'] is not None or record['previous'] is not None:
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
