#!/usr/bin/env python3
"""Run breenfs host evidence and optionally stage its records on an ext2 image."""
import argparse
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
BREENFS = ROOT / 'tools/breenfs'
GUEST = '/usr/local/filesystem-suite/host'
HOST_CASES = (
    'format/mkfs-empty-clean', 'format/mkfs-populate-clean',
    'format/reproducible-image', 'format/violations-refused',
    'crash-images/subset-images', 'crash-images/torn-images',
    'crash-images/corruption-no-panic', 'crash-images/torture-24h',
)


def run(args, cwd=ROOT):
    try:
        result = subprocess.run([str(a) for a in args], cwd=cwd, capture_output=True, text=True)
        return result.returncode, result.stdout + result.stderr
    except OSError as error:
        return 127, f'could not start {Path(str(args[0])).name}: {error.strerror}'


def reason(label, code, _output):
    # Records are visible in guest reports: do not expose host paths.
    return f'FAIL {label} exited {code}'


def torture_record(path):
    if path is None:
        return 'FAIL no torture summary supplied'
    try:
        summary = json.loads(path.read_text())
    except (OSError, UnicodeError, ValueError):
        return 'FAIL torture summary unreadable or invalid JSON'
    if not isinstance(summary, dict):
        return 'FAIL torture summary is not an object'
    elapsed = summary.get('elapsed')
    if isinstance(elapsed, bool) or not isinstance(elapsed, (int, float)) or (isinstance(elapsed, float) and not math.isfinite(elapsed)) or elapsed < 24 * 3600:
        return 'FAIL torture elapsed time is less than 24 hours or invalid'
    failures = summary.get('failures')
    if type(failures) is not int or failures != 0:
        return 'FAIL torture failures is nonzero or invalid'
    return 'PASS'


def collect(work, torture_summary):
    records = {}
    code, output = run(['cargo', 'test', '--manifest-path', BREENFS / 'Cargo.toml'])
    print(output, end='', flush=True)
    tests_ok = code == 0
    records[HOST_CASES[3]] = 'PASS' if tests_ok else reason('breenfs tests', code, output)
    target = Path(os.environ.get('CARGO_TARGET_DIR', BREENFS / 'target'))
    if not target.is_absolute():
        target = ROOT / target
    mkfs, check = target / 'debug/breenfs-mkfs', target / 'debug/breenfs-check'
    common = ['--uuid', '00112233445566778899aabbccddeeff', '--hash-key',
              '000102030405060708090a0b0c0d0e0f', '--time', '123456789', '--size', '67108864']
    def image(name, populate):
        path = work / name
        args = [mkfs, *common]
        if populate:
            args += ['--populate', BREENFS / 'tests/fixture']
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
    manifest = ROOT / 'tools/breenfs-host-tests/Cargo.toml'
    tests = (
        ('crash_images', 'subset_images_recover_to_an_acknowledged_commit'),
        ('crash_images', 'torn_images_recover_to_an_acknowledged_commit'),
        ('corruption', 'bit_flips_error_never_panic_and_checker_names_the_block'),
    )
    for case, (target, name) in zip(HOST_CASES[4:7], tests):
        code, output = run(['cargo', 'test', '--manifest-path', manifest, '--test', target, name, '--', '--exact'])
        print(output, end='', flush=True)
        if code != 0:
            records[case] = f'FAIL host test {name} exited {code}'
        elif f'test {name} ... ok' not in output.splitlines():
            records[case] = f'FAIL host test {name} did not report a pass'
        else:
            records[case] = 'PASS'
    records[HOST_CASES[7]] = torture_record(torture_summary)
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


def stage_read_manifest(image, manifest, work, kind='read'):
    """Stage a fixture manifest without rerunning host evidence."""
    destination = f'/usr/local/filesystem-suite/{kind}/manifest'
    debugfs = debugfs_path()
    commands = []
    for directory in ('/usr', '/usr/local', '/usr/local/filesystem-suite',
                      f'/usr/local/filesystem-suite/{kind}'):
        code, output = run([debugfs, '-R', f'stat {directory}', image])
        if code:
            raise RuntimeError('cannot inspect manifest destination')
        if 'File not found' in output:
            commands += [f'mkdir {directory}', f'sif {directory} mode 040755']
    # The launcher owns this disposable root copy; replace any older fixture manifest.
    commands += [f'rm {destination}', f'write "{manifest}" {destination}',
                 f'sif {destination} mode 0100644']
    batch = work / 'read-commands'
    batch.write_text('\n'.join(commands) + '\n')
    code, output = run([debugfs, '-w', '-f', batch, image])
    if code:
        raise RuntimeError('read manifest staging failed')
    # debugfs writes its version banner on stderr; verify stdout separately.
    readback = subprocess.run([debugfs, '-R', f'cat {destination}', str(image)],
                              capture_output=True, check=False)
    if readback.returncode or readback.stdout != manifest.read_bytes():
        raise RuntimeError('read manifest did not stage byte for byte')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', type=Path)
    parser.add_argument('--torture-summary', type=Path)
    parser.add_argument('--read-manifest', type=Path,
                        help='stage this read manifest only (requires --image)')
    parser.add_argument('--corrupt-manifest', type=Path,
                        help='stage this corruption manifest (requires --image)')
    args = parser.parse_args()
    if (args.read_manifest or args.corrupt_manifest) and not args.image:
        parser.error('fixture manifests require --image')
    with tempfile.TemporaryDirectory(prefix='filesystem-suite-') as directory:
        work = Path(directory)
        if args.read_manifest:
            stage_read_manifest(args.image.resolve(), args.read_manifest.resolve(), work)
        if args.corrupt_manifest:
            stage_read_manifest(args.image.resolve(), args.corrupt_manifest.resolve(), work, 'corrupt')
        if args.read_manifest or args.corrupt_manifest:
            return 0
        records = collect(work, args.torture_summary)
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
