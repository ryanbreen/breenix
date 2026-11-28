#!/usr/bin/env python3
"""Extend a private E6 Rust sysroot with native runtimes, headers and bld.

Pass one architecture's output from stage-e6-sysroot.py. The completed runtime
is renamed into a new staging directory, leaving the source sysroot intact.
Use the same verified fork snapshots supplied to the Rust sysroot builder.
"""

import argparse
from importlib import import_module
import json
from pathlib import Path
import shutil
import shlex
import subprocess
import sys
import tempfile

from userspace_generation import MARKER, digest, read_generation, verify_inputs

sysroot_builder = import_module('stage-e6-sysroot')
ROOT = sysroot_builder.ROOT
LIBC = ROOT / 'libs/libbreenix-libc'
LOADER = ROOT / 'libs/ld-breenix'
LINK = ROOT / 'userspace/programs'
PROFILE = 'release/abort'
PIC_PROFILE = 'release/abort/pic'


def write_manifest(root, record):
    (root / MARKER).write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')


def copy_sysroot(source, result, record):
    for name in ('targets', 'sysroot', 'install', 'images'):
        (result / name).mkdir(parents=True)
    for target in record['targets']:
        paths = [source / name for name, item in record['artifacts'].items()
                 if item['target'] == target]
        spec, inputs = verify_inputs(source, target, paths)
        shutil.copyfile(spec, result / spec.relative_to(source))
        for path in inputs:
            destination = result / path.relative_to(source)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, destination)
    write_manifest(result, record)


class Runtime:
    def __init__(self, work, result, record):
        self.work = work
        self.result = result
        self.record = record
        self.arch = record['arch']
        self.ordinary = f'{self.arch}-breenix'
        self.hosted = f'{self.ordinary}-hosted'
        self.sysroot = result / 'sysroot'
        self.native = self.sysroot / 'usr'
        self.lib = self.native / 'lib'
        self.lib.mkdir(parents=True)
        (self.native / 'bin').mkdir()

        self.env = sysroot_builder.clean_environment(work)
        command = sysroot_builder.command
        self.rustc = Path(command('rustup', 'which', 'rustc', env=self.env))
        self.cargo = Path(command('rustup', 'which', 'cargo', env=self.env))
        version = command(self.rustc, '-vV', env=self.env)
        if record['compiler'] != {'version': version, 'sha256': digest(self.rustc)}:
            raise ValueError('the runtime compiler must match the E6 sysroot compiler')
        installed = Path(command(self.rustc, '--print', 'sysroot', env=self.env))
        host = next(line.removeprefix('host: ') for line in version.splitlines()
                    if line.startswith('host: '))
        self.tools = installed / 'lib/rustlib' / host / 'bin'
        self.lld = self.tools / 'gcc-ld/ld.lld'
        self.flags = [
            '-Dwarnings',
            f'--remap-path-prefix={self.work}=/breenix-e6',
            f'--remap-path-prefix={ROOT}=/breenix-src',
        ]

    def verify(self, target, paths):
        write_manifest(self.result, self.record)
        return verify_inputs(self.result, target, paths)[0]

    def target_lib(self, target, sysroot=None):
        return (sysroot or self.sysroot) / 'lib/rustlib' / target / 'lib'

    def core_inputs(self, target, sysroot=None):
        files = []
        for component in ('core', 'compiler_builtins'):
            matches = list(self.target_lib(target, sysroot).glob(f'lib{component}-*.rlib'))
            if len(matches) != 1:
                raise ValueError(f'{target}: missing or ambiguous {component}')
            files.extend(matches)
        return files

    def record_file(self, source, destination, target, dependencies=(), profile=PROFILE):
        destination.parent.mkdir(parents=True, exist_ok=True)
        keys = [str(path.relative_to(self.result)) for path in dependencies]
        sysroot_builder.record_artifact(
            self.result, self.record, source, destination, target, profile,
            {path: self.record['artifacts'][path]['sha256'] for path in keys},
        )

    def build(self, target, name, directory, select, flags, output, *args, sysroot=None):
        target_dir = self.work / 'cargo' / name
        env = dict(self.env, RUSTC=str(self.rustc), CARGO_PROFILE_RELEASE_PANIC='abort',
                   CARGO_ENCODED_RUSTFLAGS='\x1f'.join(
                       [*self.flags, f'--sysroot={sysroot or self.sysroot}', *flags]))
        # The working directory is isolated from repo or developer Cargo config.
        subprocess.run([self.cargo, *select, '--locked', '--release',
                        '--manifest-path', directory / 'Cargo.toml',
                        '--target', self.result / 'targets' / f'{target}.json',
                        '--target-dir', target_dir, *args],
                       cwd=self.work, env=env, check=True)
        return target_dir / target / 'release' / output

    def clean_archive(self, path, shared=False):
        return subprocess.run(
            [sys.executable, ROOT / 'scripts/libc-clean-archive.py', '--archive', path,
             '--tool-dir', self.tools, *(['--shared-libc', self.arch] if shared else [])],
            check=True, env=self.env,
        )

    def stage_headers(self):
        generation = self.work / 'generation.h'
        generation.write_text('/* This sysroot uses the E6 userspace ABI. */\n'
                              '#define __BREENIX_E6 1\n')
        for path in sorted((LIBC / 'include').rglob('*')):
            if path.is_file():
                relative = path.relative_to(LIBC / 'include')
                source = generation if relative.as_posix() == '__breenix/generation.h' else path
                self.record_file(source, self.native / 'include' / relative, self.ordinary)

    def stage_static(self):
        target = self.ordinary
        core = self.core_inputs(target)
        self.verify(target, core)
        archive = self.build(target, 'libc', LIBC, ['build', '--lib', '--features', 'e6'],
                             [], 'libc.a')
        self.clean_archive(archive)
        self.record_file(archive, self.lib / 'libc.a', target, core)
        for name in ('linker.ld', 'linker-hosted.ld'):
            self.record_file(LINK / name, self.lib / name, target)

    def stage_pic_sysroot(self, snapshot):
        # Hosted std uses the unwind profile. The interpreter and C DSO need
        # their own position-independent core and builtins with abort semantics.
        _, hosted_library = sysroot_builder.fork_libraries(self.work, snapshot, self.env)
        crate = self.work / 'pic-core'
        (crate / 'src').mkdir(parents=True)
        (crate / 'src/lib.rs').write_text('#![no_std]\n')
        (crate / 'Cargo.toml').write_text(
            '[package]\nname = "breenix-e6-pic-core"\nversion = "0.0.0"\n'
            'edition = "2021"\npublish = false\n\n[workspace]\n')
        spec = self.verify(self.hosted, [])
        target_dir = self.work / 'cargo/pic-core'
        env = dict(self.env, RUSTC=str(self.rustc), CARGO_PROFILE_RELEASE_PANIC='abort',
                   __CARGO_TESTS_ONLY_SRC_ROOT=str(hosted_library),
                   CARGO_ENCODED_RUSTFLAGS='\x1f'.join([*self.flags, '-Crelocation-model=pic']))
        subprocess.run([self.cargo, 'build', '--release', '--target', spec, '--target-dir',
                        target_dir, '-Zbuild-std=core,compiler_builtins',
                        '-Zbuild-std-features=compiler-builtins-mem'],
                       cwd=crate, env=env, check=True)
        pic_root = self.sysroot / 'pic'
        lib = self.target_lib(self.hosted, pic_root)
        for rlib in (target_dir / self.hosted / 'release/deps').glob('lib*.rlib'):
            if rlib.name.startswith(('libcore-', 'libcompiler_builtins-')):
                self.record_file(rlib, lib / rlib.name, self.hosted, profile=PIC_PROFILE)
        return pic_root

    def stage_shared(self, snapshot):
        target = self.hosted
        pic_root = self.stage_pic_sysroot(snapshot)
        core = self.core_inputs(target, pic_root)
        spec = self.verify(target, core)
        directory = Path(sysroot_builder.command(
            sys.executable, ROOT / 'scripts/rust-host/prepare-kd-libc.py',
            LIBC, self.work / 'shared-source', self.arch, env=self.env,
        )).parent
        pic = ['-Crelocation-model=pic', '-Cdefault-linker-libraries=no']
        archive = self.build(target, 'shared-libc', directory,
                             ['build', '--lib', '--features', 'hosted-rustc,e6'], pic, 'libc.a',
                             sysroot=pic_root)
        shared_archive = self.work / 'libc-pic.a'
        subprocess.run([self.tools / 'llvm-objcopy', '--rename-section',
                        '.rodata.breenix_dso_handle=.data.rel.ro.breenix_dso_handle,alloc,load,data,contents',
                        archive, shared_archive], check=True, env=self.env)
        self.clean_archive(shared_archive, shared=True)
        exports = self.lib / 'hosted-libc.exports'
        layout = self.lib / 'kd-bases.ld'
        self.record_file(LIBC / exports.name, exports, target)
        self.record_file(ROOT / 'scripts/rust-host' / layout.name, layout, target)
        self.verify(target, [*core, exports, layout])
        shared = self.work / 'libc.so'
        subprocess.run([self.lld, '-shared', '--no-undefined', '--fatal-warnings', '--eh-frame-hdr',
                        '--hash-style=sysv', '-soname', 'libc.so',
                        '--version-script', exports,
                        '-o', shared, '--whole-archive', shared_archive, '--no-whole-archive'],
                       check=True, env=self.env)
        self.record_file(shared, self.lib / shared.name, target, [*core, exports], PIC_PROFILE)

        loader = self.build(target, 'loader', LOADER,
                            ['rustc', '--features', 'e6', '--bin', 'ld-breenix'],
                            [*pic, f'-Clinker={self.tools / "rust-lld"}'], 'ld-breenix',
                            '--', '-Clink-arg=-shared', '-Clink-arg=-Bsymbolic',
                            '-Clink-arg=--no-undefined', '-Clink-arg=--fatal-warnings',
                            f'-Clink-arg=-T{layout}',
                            '-Clink-arg=--entry=_start', '-Clink-arg=-soname=ld-breenix.so',
                            sysroot=pic_root)
        self.record_file(loader, self.lib / 'ld-breenix.so', target, [*core, layout], PIC_PROFILE)
        for name, filename in (('hosted_crt', 'breenix-crt.o'),
                               ('hosted_dso_handle', 'hosted_dso_handle.o')):
            obj = self.work / filename
            subprocess.run(
                [self.rustc, *self.flags, f'--sysroot={pic_root}', '--edition=2021', '--crate-type=lib', '--emit=obj',
                 '-Cpanic=abort', '-Copt-level=2', *pic, '--target', spec,
                 LIBC / 'src' / (name + '.rs'), '-o', obj],
                check=True, env=self.env,
            )
            self.record_file(obj, self.lib / filename, target, core, PIC_PROFILE)

    def stage_bld(self):
        target = self.ordinary
        archive, linker = (self.lib / name for name in ('libc.a', 'linker.ld'))
        inputs = [archive, linker, *sorted(self.target_lib(target).glob('*.rlib'))]
        self.verify(target, inputs)
        deps = self.work / 'bld.d'
        flags = [f'-Clinker={self.tools / "rust-lld"}', f'-Lnative={self.lib}',
                 f'-Clink-arg=-T{linker}', f'-Clink-arg=--dependency-file={deps}',
                 '-Clink-arg=-static',
                 '-Clink-arg=--allow-multiple-definition', '-Cdefault-linker-libraries=no']
        bld = self.build(target, 'bld', ROOT / 'toolchain',
                         ['build', '-p', 'bld', '--bin', 'bld'], flags, 'bld')
        paths = set(shlex.split(deps.read_text().replace('\\\n', '')))
        linked = [path for path in inputs if str(path) in paths]
        if archive not in linked or linker not in linked or not any(
            path.name.startswith('libstd-') for path in linked
        ):
            raise ValueError(f'{target}: bld link omitted the staged libc, layout or std'
                             f' (matched {[path.name for path in linked]})')
        self.record_file(bld, self.native / 'bin/bld', target, linked)
        (self.native / 'bin/bld').chmod(0o755)


def stage(source, snapshot, output):
    if output.exists() or output.is_symlink():
        raise ValueError(f'output already exists: {output}')
    record = read_generation(source)
    if record.get('std_patch_sha256') != digest(sysroot_builder.STD_PATCH):
        raise ValueError('rebuild the E6 sysroot with the current Rust std stack minimum patch')
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.e6-runtime-', dir=output.parent) as directory:
        work = Path(directory).resolve()
        result = work / 'staged'
        copy_sysroot(source, result, record)
        runtime = Runtime(work, result, record)
        runtime.stage_headers()
        runtime.stage_static()
        # Link bld while the native search directory contains only libc.a.
        runtime.stage_bld()
        runtime.stage_shared(snapshot)
        for target in record['targets']:
            runtime.verify(target, [
                result / name for name, item in record['artifacts'].items()
                if item['target'] == target
            ])
        result.rename(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--sysroot', required=True, type=Path)
    parser.add_argument('--fork-snapshot', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    try:
        stage(args.sysroot.resolve(strict=True), args.fork_snapshot.resolve(strict=True),
              args.output.absolute())
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'{parser.prog}: {error}\n')


if __name__ == '__main__':
    main()
