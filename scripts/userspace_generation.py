#!/usr/bin/env python3
"""Verify E6 build inputs and keep staging trees out of legacy publishers."""

import argparse
import hashlib
import json
from pathlib import Path
import sys

MARKER = '.breenix-generation.json'
ARCHES = ('x86_64', 'aarch64')
TARGETS = Path(__file__).resolve().parents[1] / 'toolchain/targets/e6'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def require_legacy(*paths):
    """Reject staging roots and descendants, including through a symlink."""
    for path in paths:
        if path is None:
            continue
        path = Path(path).absolute()
        resolved = path.resolve()
        for directory in {path, *path.parents, resolved, *resolved.parents}:
            marker = directory / MARKER
            if marker.exists() or marker.is_symlink():
                raise ValueError(f'staged E6 input {path} cannot enter a legacy publisher ({marker})')


def read_generation(root):
    record = json.loads((root / MARKER).read_text())
    if record['generation'] != 'e6' or record['arch'] not in ARCHES:
        raise ValueError(f'invalid E6 generation at {root}')
    return record


def checked_file(root, path, sha256):
    if Path(path).is_absolute() or '..' in Path(path).parts:
        raise ValueError(f'input outside the E6 generation: {path}')
    file = root / path
    if file.resolve() != file or not file.is_file():
        raise ValueError(f'input missing or outside the E6 generation: {file}')
    if digest(file) != sha256:
        raise ValueError(f'E6 input SHA-256 differs: {file}')
    return file


def verify_inputs(root, target, inputs):
    import tomllib

    root = root.resolve(strict=True)
    record = read_generation(root)
    if target not in record['targets'] or not target.startswith(record['arch'] + '-breenix'):
        raise ValueError(f'{target} is not a generation target')
    spec = Path('targets') / f'{target}.json'
    target_record = record['targets'][target]
    pins = tomllib.loads((TARGETS.parents[2] / 'rust-fork.toml').read_text())
    pin = pins['hosted'] if target.endswith('-hosted') else pins
    if pin['commit'] != target_record['fork_commit']:
        raise ValueError(f'{target} does not use the pinned E6 fork library')
    target_path = checked_file(root, spec, target_record['sha256'])
    if digest(TARGETS / spec.name) != target_record['sha256']:
        raise ValueError(f'{target_path} differs from the repository E6 target')
    paths = []
    for path in inputs:
        path = Path(path).absolute()
        if not path.is_relative_to(root):
            raise ValueError(f'input outside the E6 generation: {path}')
        paths.append(str(path.relative_to(root)))
    remaining = list(paths)
    checked = {}
    while remaining:
        relative = remaining.pop()
        if relative in checked:
            continue
        artifact = record['artifacts'].get(relative)
        if artifact is None or artifact['target'] != target:
            raise ValueError(f'unrecorded E6 input for {target}: {relative}')
        checked[relative] = checked_file(root, relative, artifact['sha256'])
        for name, sha256 in artifact['link_inputs'].items():
            dependency = record['artifacts'].get(name)
            if dependency is None or dependency['target'] != target or dependency['sha256'] != sha256:
                raise ValueError(f'unrecorded E6 dependency for {target}: {name}')
            remaining.append(name)
    return target_path, [checked[name] for name in paths]


def verify_release(root, expected):
    """Require an inventory supplied by the publisher, independently of the record."""
    root = root.resolve(strict=True)
    record = read_generation(root)
    if not expected or any(Path(name).parts[:1] not in {('install',), ('images',)} for name in expected):
        raise ValueError(f'incomplete E6 generation at {root}: no release inventory')
    missing = set(expected) - record['artifacts'].keys()
    if missing:
        raise ValueError(f'incomplete E6 generation at {root}: missing {", ".join(sorted(missing))}')
    unknown_targets = {item['target'] for item in record['artifacts'].values()} - record['targets'].keys()
    if unknown_targets:
        raise ValueError(f'invalid E6 artifact targets at {root}: {sorted(unknown_targets)}')
    for target in record['targets']:
        verify_inputs(root, target, [
            root / name for name, artifact in record['artifacts'].items()
            if artifact['target'] == target
        ])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('legacy').add_argument('paths', nargs='+', type=Path)
    inputs = commands.add_parser('verify-inputs')
    inputs.add_argument('root', type=Path)
    inputs.add_argument('target')
    inputs.add_argument('paths', nargs='+', type=Path)
    release = commands.add_parser('verify-release')
    release.add_argument('root', type=Path)
    release.add_argument('expected', nargs='+')
    args = parser.parse_args()
    try:
        if args.command == 'legacy':
            require_legacy(*args.paths)
        elif args.command == 'verify-inputs':
            verify_inputs(args.root, args.target, args.paths)
        else:
            verify_release(args.root, args.expected)
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f'{parser.prog}: {error}\n')


if __name__ == '__main__':
    main()
