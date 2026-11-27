#!/usr/bin/env python3
"""Stage the pinned Breenix development toolchain on the Mac."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from rust_host_stage import (ROOT, hosted_runtime_entries, rustc_fixture_entries,
                             sha256, tool_target_link_inputs, verify_hosted_elf)

ARCH = 'x86_64'
TOOLCHAIN_ID = re.compile(r'x86_64-[0-9a-f]{8}-[0-9a-f]{8}')


def read_json(path):
    return json.loads(path.read_text())


def manifest_bytes(record):
    return (json.dumps(record, indent=2, sort_keys=True) + '\n').encode()


def manifest_id(record):
    commit = record['fork_commit']
    if not re.fullmatch(r'[0-9a-f]{40}', commit):
        raise ValueError('invalid K5 fork commit')
    digest = hashlib.sha256(manifest_bytes(record)).hexdigest()
    return f'{ARCH}-{commit[:8]}-{digest[:8]}'


def copy_pinned(root, name, source, mode, digest):
    relative = Path(name)
    if (relative.is_absolute() or '..' in relative.parts or str(relative) != name
            or not re.fullmatch(r'[a-zA-Z0-9_./-]+', name)):
        raise ValueError(f'invalid staging path: {name}')
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    with source.open('rb') as stream, path.open('xb') as output:
        shutil.copyfileobj(stream, output)
    if sha256(path) != digest:
        raise ValueError(f'{source}: SHA-256 differs from the E25 record for {name}')
    path.chmod(int(mode, 8))
    return {'size': path.stat().st_size, 'mode': mode, 'sha256': digest}


def stage(out, records, *, artifacts, sysroot, targets, fixture, cargo, bld, kd):
    k5, k6, k7 = (read_json(records / name / f'{ARCH}.json')
                  for name in ('k5', 'k6', 'k7'))
    if (k7['fork_commit'] != k5['fork_commit']
            or any(k5[name] != digest for name, digest in k7['k5_compiler_digests'].items())
            or (k6['rustc_sha256'], k6['rustc_commit']) !=
            (k5['mac_rustc_sha256'], k5['fork_commit'])):
        raise ValueError('K6 and K7 compiler identities differ from K5')
    entries = list(rustc_fixture_entries(artifacts, sysroot, targets, k5))
    entries.extend(tool_target_link_inputs(fixture, k6))
    entries.extend([
        ('bin/cargo', cargo, '0755', k7['cargo_sha256']),
        ('bin/bld', bld / 'bld', '0755', read_json(bld / 'build.json')['files_sha256']['bld']),
    ])

    kd_record = read_json(kd / 'build.json')
    if kd_record['libc.so'] != read_json(records / f'hosted-libc-{ARCH}.json')['sha256']:
        raise ValueError('KD libc.so SHA-256 differs from the hosted libc pin')
    runtime_entries, runtime_digests = hosted_runtime_entries(kd, kd_record, {})

    # Both trees stay private until every copied file matches its receipt.
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.devbox-stage-', dir=out.parent) as temporary:
        version, runtime = (Path(temporary) / name for name in ('toolchain', 'runtime'))
        files = {name: copy_pinned(version, name, source, mode, digest or
                                   k7['fixture_sha256'][name])
                 for name, source, mode, digest in entries}
        runtime_files = {name: copy_pinned(runtime, Path(name).name, source, mode,
                                           runtime_digests[name])
                         for name, source, mode in runtime_entries}
        verify_hosted_elf(runtime / 'libc.so', version / 'bin/rustc',
                          version / 'lib' / k5['hosted_link']['driver'], version / 'bin/cargo')
        manifest = {'fork_commit': k5['fork_commit'], 'files': files, 'runtime': runtime_files}
        toolchain_id = manifest_id(manifest)
        manifest_path = version / 'MANIFEST.json'
        manifest_path.write_bytes(manifest_bytes({'id': toolchain_id, **manifest}))
        manifest_path.chmod(0o444)
        for tree in (version, runtime):
            for directory, _, _ in os.walk(tree):
                Path(directory).chmod(0o755)

        destination = out / toolchain_id
        runtime_destination = out.parent / 'runtime' / ARCH
        if destination.exists() or destination.is_symlink():
            raise ValueError(f'staged toolchain already exists: {destination}')
        if runtime_destination.exists() or runtime_destination.is_symlink():
            # Multiple versions can share an identical runtime in one cache.
            if (runtime_destination.is_symlink() or
                    {path.name for path in runtime_destination.iterdir()} !=
                    {Path(name).name for name in runtime_files}):
                raise ValueError(f'staged runtime differs: {runtime_destination}')
            for name, facts in runtime_files.items():
                path = runtime_destination / Path(name).name
                if (path.is_symlink() or not path.is_file() or
                        stat.S_IMODE(path.stat().st_mode) != int(facts['mode'], 8) or
                        sha256(path) != facts['sha256']):
                    raise ValueError(f'staged runtime differs: {path}')
        else:
            runtime_destination.parent.mkdir(parents=True, exist_ok=True)
            runtime.rename(runtime_destination)
        version.rename(destination)
    return toolchain_id


def current_line(out, toolchain_id):
    if not TOOLCHAIN_ID.fullmatch(toolchain_id):
        raise ValueError('invalid toolchain id')
    path = out / toolchain_id / 'MANIFEST.json'
    manifest = read_json(path)
    if manifest.pop('id') != toolchain_id or manifest_id(manifest) != toolchain_id:
        raise ValueError(f'manifest does not match {toolchain_id}')
    return toolchain_id


def input_path(variable, default):
    return Path(os.environ.get(variable, default)).expanduser()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    staging = commands.add_parser('stage', help='verify and stage E25 artifacts')
    staging.add_argument('--arch', required=True, choices=[ARCH])
    staging.add_argument('--out', type=Path, required=True)
    programs = ROOT / 'userspace/programs/target'
    cache = Path.home() / 'Library/Caches/breenix-e25'
    fixture = input_path('BREENIX_K6_FIXTURE', cache / 'k6/fixture')
    staging.add_argument('--fixture', type=Path, default=fixture)
    staging.add_argument('--sysroot', type=Path, default=input_path(
        'BREENIX_K5_SYSROOT', fixture / 'sysroot'))
    staging.add_argument('--artifacts', type=Path, default=programs / 'rustc' / ARCH,
                         help='K5 directory with rustc and its driver')
    staging.add_argument('--cargo', type=Path, default=input_path(
        'BREENIX_K7_STAGE', cache / 'k7/stage1') / 'bin/cargo')
    staging.add_argument('--bld', type=Path, default=programs / 'rust-link' / ARCH,
                         help='E25 bld and its build.json')
    staging.add_argument('--kd', type=Path, default=input_path(
        'BREENIX_KD_BUILD_DIR', programs / 'kd'), help='E25 runtime and its build.json')
    current = commands.add_parser('current', help='print the one-line activation payload')
    current.add_argument('--out', type=Path, required=True)
    current.add_argument('id')
    args = parser.parse_args()
    try:
        if args.command == 'stage':
            result = stage(args.out, ROOT / 'toolchain/rust-host', artifacts=args.artifacts,
                           sysroot=args.sysroot, targets=ROOT, fixture=args.fixture,
                           cargo=args.cargo, bld=args.bld, kd=args.kd)
        else:
            result = current_line(args.out, args.id)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, f'{parser.prog}: {error}\n')
    print(result)


if __name__ == '__main__':
    main()
