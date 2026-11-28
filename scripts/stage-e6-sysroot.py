#!/usr/bin/env python3
"""Build a private E6 Rust sysroot from the verified ordinary and hosted fork snapshots.

Run scripts/rust-fork.sh snapshot first. Pass the ordinary snapshot directory;
the hosted snapshot is taken from its -hosted sibling. Cargo uses its local
download cache offline. No C runtime, program installs or images are staged here.
"""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib

from userspace_generation import ARCHES, MARKER, TARGETS, digest, verify_inputs

ROOT = Path(__file__).resolve().parents[1]
PROFILE = {'abort': 'release/abort', 'unwind': 'release/unwind'}


def command(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


def clean_environment(work):
    # Reuse the download caches without inheriting a developer's Cargo config,
    # linker, rustc wrapper, profile flags or previously built target objects.
    env = {name: os.environ[name] for name in ('PATH', 'HOME', 'RUSTUP_HOME', 'CARGO_BUILD_JOBS')
           if name in os.environ}
    env['CARGO_HOME'] = str(work / 'cargo-home')
    env['CARGO_NET_OFFLINE'] = 'true'
    env['RUSTUP_TOOLCHAIN'] = tomllib.loads(
        (ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel']
    cargo_home = Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo')).resolve()
    for cache in ('registry/index', 'registry/cache', 'git/db'):
        source = cargo_home / cache
        if source.is_dir():
            link = work / 'cargo-home' / cache
            link.parent.mkdir(parents=True, exist_ok=True)
            link.symlink_to(source, target_is_directory=True)
    return env


def fork_libraries(work, snapshot, env):
    (work / 'libs').symlink_to(ROOT / 'libs', target_is_directory=True)
    for suffix in ('', '-hosted'):
        source = snapshot.with_name(snapshot.name + suffix)
        if not (source / '.rust-fork-snapshot.json').is_file():
            raise ValueError(f'expected a verified fork snapshot at {source}')
        shutil.copytree(source, work / f'rust-fork{suffix}')
    library = work / 'rust-fork/library'
    fork_env = dict(env, BREENIX_RUST_FORK_LIBRARY=str(library))
    tool = ROOT / 'scripts/rust-fork.sh'
    subprocess.run([tool, 'verify'], env=fork_env, check=True)
    return library, Path(command(tool, 'hosted-library', env=fork_env))


def record_artifact(result, record, source, destination, target, profile, link_inputs):
    shutil.copyfile(source, destination)
    record['artifacts'][str(destination.relative_to(result))] = {
        'target': target, 'sha256': digest(destination), 'profile': profile,
        'link_inputs': link_inputs,
    }


def build_bunwind(work, result, record, manifest, target, lib, cargo, build_env):
    # Use the staged hosted core and builtins in the standalone abort-only
    # archive. The future hosted link can use its rlib via kd_link's existing
    # extraction recipe when it needs the hosted panic runtime.
    manifest.write_text(json.dumps(record) + '\n')
    inputs = [next(lib.glob(f'lib{name}-*.rlib')) for name in ('core', 'compiler_builtins')]
    verify_inputs(result, target.stem, inputs)
    source = work / 'bunwind'
    shutil.copytree(ROOT / 'libs/bunwind', source, ignore=shutil.ignore_patterns('target'))
    output = work / 'target/bunwind'
    env = dict(build_env, CARGO_PROFILE_RELEASE_PANIC='abort')
    env['CARGO_ENCODED_RUSTFLAGS'] += f'\x1f--sysroot={result / "sysroot"}'
    subprocess.run([cargo, 'build', '--locked', '--release', '--lib', '--target', target,
                    '--target-dir', output], cwd=source, env=env, check=True)
    for suffix, dependencies in (('.a', inputs), ('.rlib', inputs[:1])):
        name = f'libbunwind{suffix}'
        record_artifact(result, record, output / target.stem / 'release' / name, lib / name,
                        target.stem, PROFILE['abort'],
                        {str(path.relative_to(result)): digest(path) for path in dependencies})


def stage(arch, snapshot, output):
    if output.exists() or output.is_symlink():
        raise ValueError(f'output already exists: {output}')
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.e6-sysroot-', dir=output.parent) as directory:
        work = Path(directory).resolve()
        result = work / 'staged'
        for name in ('targets', 'sysroot', 'install', 'images'):
            (result / name).mkdir(parents=True)
        manifest = result / MARKER
        manifest.write_text('{"generation":"e6"}\n')
        env = clean_environment(work)
        ordinary_library, hosted_library = fork_libraries(work, snapshot, env)
        rustc = Path(command('rustup', 'which', 'rustc', env=env))
        cargo = Path(command('rustup', 'which', 'cargo', env=env))
        compiler = {'version': command(rustc, '-vV', env=env), 'sha256': digest(rustc)}
        record = {'generation': 'e6', 'arch': arch, 'compiler': compiler,
                  'targets': {}, 'artifacts': {}}
        pins = tomllib.loads((ROOT / 'rust-fork.toml').read_text())
        crate = work / 'probe'
        (crate / 'src').mkdir(parents=True)
        (crate / 'src/lib.rs').write_text('#![no_std]\n')
        (crate / 'Cargo.toml').write_text(
            '[package]\nname = "breenix-e6-sysroot"\nversion = "0.0.0"\n'
            'edition = "2021"\npublish = false\n\n[workspace]\n')
        for path in sorted(TARGETS.glob(f'{arch}-*.json')):
            shutil.copyfile(path, result / 'targets' / path.name)

        for suffix, library, pin, panic in (
            ('', ordinary_library, pins, 'abort'),
            ('-hosted', hosted_library, pins['hosted'], 'unwind'),
        ):
            name = f'{arch}-breenix{suffix}'
            target = result / 'targets' / f'{name}.json'
            commit = pin['commit']
            target_hash = digest(target)
            record['targets'][name] = {'sha256': target_hash, 'fork_commit': commit}
            target_dir = work / 'target' / name
            print(f'{name}: building std and panic_{panic} from {commit}', flush=True)
            build_env = dict(env, RUSTC=str(rustc), __CARGO_TESTS_ONLY_SRC_ROOT=str(library),
                             CARGO_PROFILE_RELEASE_PANIC=panic)
            build_env['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join((
                '-Dwarnings', '-Cembed-bitcode=yes',
                f'-Cmetadata=breenix_e6_{arch}_{target_hash}_{commit}',
                f'--remap-path-prefix={work}=/breenix-e6',
                f'--remap-path-prefix={ROOT}=/breenix-src',
            ))
            subprocess.run([cargo, 'build', '--release', '--target', target, '--target-dir', target_dir,
                            f'-Zbuild-std=std,panic_{panic}', '-Zbuild-std-features=compiler-builtins-mem'],
                           cwd=crate, env=build_env, check=True)
            lib = result / 'sysroot/lib/rustlib' / name / 'lib'
            lib.mkdir(parents=True)
            for rlib in sorted((target_dir / name / 'release/deps').glob('lib*.rlib')):
                if rlib.name.startswith('libbreenix_e6_sysroot-'):
                    continue
                record_artifact(result, record, rlib, lib / rlib.name, name, PROFILE[panic], {})
            required = ['core', 'alloc', 'std', 'compiler_builtins', f'panic_{panic}']
            if panic == 'unwind':
                required.append('unwind')
            for component in required:
                if len(list(lib.glob(f'lib{component}-*.rlib'))) != 1:
                    raise ValueError(f'{name}: missing or ambiguous {component} archive')
            if panic == 'unwind':
                build_bunwind(work, result, record, manifest, target, lib, cargo, build_env)
            subprocess.run(['bash', '-c', '. "$1"; shift; sdk_no_libm_math "$@"',
                            'stage-e6-sysroot', ROOT / 'sdk/builder/lib.sh', *sorted(lib.glob('*.rlib'))],
                           env=env, check=True)

        if arch == 'x86_64':
            # The staged C-facing alias has the same ABI as ordinary Rust.
            # It has no independent std or native libc in PR 2.
            alias = 'x86_64-breenix-c'
            record['targets'][alias] = {'sha256': digest(result / 'targets' / f'{alias}.json'),
                                       'fork_commit': pins['commit']}
        manifest.write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')
        for target in record['targets']:
            verify_inputs(result, target, [
                result / name for name, info in record['artifacts'].items()
                if info['target'] == target
            ])
        result.rename(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--arch', choices=ARCHES, required=True)
    parser.add_argument('--fork-snapshot', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    try:
        stage(args.arch, args.fork_snapshot.absolute(), args.output.absolute())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'{parser.prog}: {error}\n')


if __name__ == '__main__':
    main()
