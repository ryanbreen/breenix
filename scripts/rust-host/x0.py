#!/usr/bin/env python3
"""Record X0 host observations without displaying workload source."""
import argparse
import datetime
import hashlib
import json
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TREE = Path.home() / 'fun/code/rust.worktrees/x0-666252fd'
RECORDS = ROOT / 'toolchain/rust-host/x0'
BASE = '706f244db'
PIN = '666252fd61839b9e9b1bc9e777a0c84c5cf3dc6d'


def run(*args, check=True):
    return subprocess.run(args, text=True, capture_output=True, check=check)


def git(*args, check=True):
    return run('git', '-C', str(TREE), *args, check=check)


def new_record(number):
    destination = RECORDS / f'{number}.json'
    if destination.exists():
        raise SystemExit('probe record already exists')
    return destination


def check_licenses(now):
    licenses = json.loads((RECORDS / 'licenses.json').read_text())
    if not licenses or any(datetime.datetime.fromisoformat(v['recorded_at']) >= now
                           for v in licenses.values()):
        raise SystemExit('licenses must be recorded before the probe')


def record_one():
    destination = new_record(1)
    now = datetime.datetime.now(datetime.timezone.utc)
    check_licenses(now)
    if git('rev-parse', 'HEAD').stdout.strip() != PIN:
        raise SystemExit('probe tree is not at the fork pin')
    status = git('status', '--porcelain', '--', 'compiler', 'src/llvm-project').stdout
    if status:
        raise SystemExit('probe tree is dirty in the scoped paths')
    trees = {}
    for ref in (BASE, PIN):
        trees[ref] = {
            'compiler': git('rev-parse', ref + ':compiler').stdout.strip(),
            'llvm_project': git('ls-tree', ref, 'src/llvm-project').stdout.split()[2],
        }
    diff = git('diff', '--quiet', BASE, PIN, '--', 'compiler', 'src/llvm-project', check=False)
    if diff.returncode not in (0, 1):
        raise SystemExit('git diff failed: ' + diff.stderr)
    executable = Path(shutil.which('git')).resolve()
    record = {
        'command': ['git rev-parse <ref>:compiler', 'git ls-tree <ref> src/llvm-project',
                    'git diff --quiet 706f244db 666252fd -- compiler src/llvm-project'],
        'tools': {'git': {'version': run('git', '--version').stdout.strip(),
                          'sha256': hashlib.sha256(executable.read_bytes()).hexdigest()}},
        'output': {'trees': trees, 'diff_status': diff.returncode},
        'verdict': 'same' if diff.returncode == 0 else 'different',
        'time': now.isoformat(), 'probe_tree_status': status,
        'observation': 'Tree IDs were read before the ledger line; no prediction is claimed (ruling 76).',
    }
    destination.write_text(json.dumps(record, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(record['output'], sort_keys=True))


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='action', required=True)
    rec = sub.add_parser('record')
    rec.add_argument('number', type=int, choices=[1, 9])
    rec.add_argument('--capture', type=Path)
    rec.add_argument('--release-parity', type=Path)
    profile = sub.add_parser('profile-linux')
    profile.add_argument('--scratch', type=Path, required=True)
    profile.add_argument('--workspace-commit', required=True)
    profile.add_argument('--prediction-file', type=Path, required=True)
    args = parser.parse_args()
    if args.action == 'profile-linux':
        from x0_profile import capture

        started = datetime.datetime.now(datetime.timezone.utc)
        check_licenses(started)
        prediction = json.loads(args.prediction_file.read_text())
        if (not prediction['prediction'].strip()
                or datetime.datetime.fromisoformat(prediction['prediction_time']) >= started):
            parser.error('the prediction must precede the X0.9 run')
        capture(args.scratch, args.workspace_commit, prediction, started)
    elif args.number == 1:
        record_one()
    else:
        if not args.capture or not args.release_parity:
            parser.error('record 9 requires --capture and --release-parity')
        record = json.loads(args.capture.read_text())
        record['release_parity'] = json.loads(args.release_parity.read_text())
        new_record(9).write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')


if __name__ == '__main__':
    main()
