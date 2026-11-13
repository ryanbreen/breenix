#!/usr/bin/env python3
"""Run cowfs host evidence and optionally stage its records on an ext2 image."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
COWFS = ROOT / 'tools/cowfs'
GUEST = '/usr/local/filesystem-suite/host'
HOST_CASES = (
    'format/mkfs-empty-clean', 'format/mkfs-populate-clean',
    'format/reproducible-image', 'format/violations-refused',
)


def run(args, cwd=ROOT):
    try:
        result = subprocess.run([str(a) for a in args], cwd=cwd, capture_output=True, text=True)
        return result.returncode, result.stdout + result.stderr
    except OSError as error:
        return 127, f'could not start {Path(str(args[0])).name}: {error.strerror}'


def reason(label, code, output):
    # Records are visible in guest reports: do not expose host paths.
    lines = [line for line in output.splitlines() if line.startswith(('error:', 'test result:'))]
    detail = ' '.join(lines[-2:]).replace(str(ROOT), '<checkout>')
    return f'FAIL {label} exited {code}' + (f': {detail}' if detail else '')


def collect(work):
    records = {}
    code, output = run(['cargo', 'test', '--manifest-path', COWFS / 'Cargo.toml'])
    print(output, end='', flush=True)
    tests_ok = code == 0
    records[HOST_CASES[3]] = 'PASS' if tests_ok else reason('cowfs tests', code, output)
    target = Path(os.environ.get('CARGO_TARGET_DIR', COWFS / 'target'))
    if not target.is_absolute():
        target = ROOT / target
    mkfs, check = target / 'debug/cowfs-mkfs', target / 'debug/cowfs-check'
    common = ['--uuid', '00112233445566778899aabbccddeeff', '--hash-key',
              '000102030405060708090a0b0c0d0e0f', '--time', '123456789', '--size', '67108864']
    def image(name, populate):
        path = work / name
        args = [mkfs, *common]
        if populate:
            args += ['--populate', COWFS / 'tests/fixture']
        code, output = run([*args, path])
        return path, code, output
    images = []
    for case, populated in zip(HOST_CASES[:2], (False, True)):
        path, code, output = image(case.split('/')[-1], populated)
        images.append(path)
        if code == 0:
            code, output = run([check, '--full', path])
        records[case] = 'PASS' if code == 0 else reason('formatter or checker', code, output)
    other, code, output = image('repeat', True)
    if code != 0:
        records[HOST_CASES[2]] = reason('formatter', code, output)
    elif records[HOST_CASES[1]] != 'PASS':
        records[HOST_CASES[2]] = 'FAIL populated image was not produced and checked'
    else:
        records[HOST_CASES[2]] = 'PASS' if images[1].read_bytes() == other.read_bytes() else 'FAIL fixed inputs produced different images'
    return records


def debugfs_path():
    for name in ('debugfs', '/sbin/debugfs', '/usr/sbin/debugfs', '/opt/homebrew/opt/e2fsprogs/sbin/debugfs', '/usr/local/opt/e2fsprogs/sbin/debugfs'):
        found = shutil.which(name)
        if found:
            return found
    raise RuntimeError('debugfs is required to stage an image')


def stage(image, work, records):
    debugfs = debugfs_path()
    code, output = run([debugfs, '-R', f'stat {GUEST}', image])
    if 'File not found' not in output:
        raise RuntimeError('image already holds host records or cannot be inspected')
    commands = []
    parts = GUEST.strip('/').split('/')
    for i in range(1, len(parts) + 1):
        directory = '/' + '/'.join(parts[:i])
        _, found = run([debugfs, '-R', f'stat {directory}', image])
        if 'File not found' in found:
            commands += [f'mkdir {directory}', f'sif {directory} mode 040755']
    for category in sorted({case.split('/')[0] for case in HOST_CASES}):
        commands += [f'mkdir {GUEST}/{category}', f'sif {GUEST}/{category} mode 040755']
    for index, case in enumerate(HOST_CASES):
        path = work / str(index)
        path.write_text(records[case] + '\n')
        commands += [f'write "{path}" {GUEST}/{case}', f'sif {GUEST}/{case} mode 0100644']
    batch = work / 'commands'
    batch.write_text('\n'.join(commands) + '\n')
    code, output = run([debugfs, '-w', '-f', batch, image])
    if code:
        raise RuntimeError('debugfs staging failed')
    for case in HOST_CASES:
        code, output = run([debugfs, '-R', f'cat {GUEST}/{case}', image])
        if code or not output.startswith(records[case] + '\n'):
            raise RuntimeError(f'failed to read back staged record {case}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='filesystem-suite-') as directory:
        work = Path(directory)
        records = collect(work)
        for case in HOST_CASES:
            print(f'{case}: {records[case]}', flush=True)
        if args.image:
            try:
                stage(args.image.resolve(), work, records)
            except RuntimeError as error:
                print(f'filesystem-suite: {error}', file=sys.stderr)
                return 1
        return 0 if all(value == 'PASS' for value in records.values()) else 1


if __name__ == '__main__':
    sys.exit(main())
