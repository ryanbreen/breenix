#!/usr/bin/env python3
"""Install the hosted suite, probes and fixture on a private image."""

import argparse
from array import array
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent / 'metal'))
from metal_root import require_e2fsprogs

ROOT = Path(__file__).resolve().parents[1]
PROGRAMS = ROOT / 'userspace/programs'
SUITE_PATH = '/sbin/suite-rust-host'
PROBE_PATH = '/usr/libexec/breenix/rust-host/startup-probe'
UNWIND_PROBE_PATH = '/usr/libexec/breenix/rust-host/unwind-probe'
PROCESS_PROBE_PATH = '/usr/libexec/breenix/rust-host/process-probe'
MMAP_FIXTURE_PATH = '/usr/libexec/breenix/rust-host/mmap-fixture'
MMAP_FIXTURE_BYTES = 163_067_344  # Largest file mapping in the X0.9 profile.
# Leave room for the fixture and ext2 allocation metadata.
SCRATCH_HEADROOM = 192 * 1024 * 1024


def probe_for_suite(selection, probe):
    return probe if 'rust-host' in selection.split(',') else None


def write_mmap_fixture(path):
    with path.open('wb') as output:
        chunk_bytes = 1024 * 1024
        for start in range(0, MMAP_FIXTURE_BYTES, chunk_bytes):
            end = min(start + chunk_bytes, MMAP_FIXTURE_BYTES)
            words = array('Q', range(start, end, 8))
            if sys.byteorder != 'little':
                words.byteswap()
            words.tofile(output)


def stage(image, selection, suite, probe, process_probe, unwind_probe):
    probe = probe_for_suite(selection, probe)
    if probe is None:
        return
    entries = ((SUITE_PATH, suite), (PROBE_PATH, probe), (PROCESS_PROBE_PATH, process_probe), (UNWIND_PROBE_PATH, unwind_probe))
    for _, elf in entries:
        with elf.open('rb') as source:
            if source.read(4) != b'\x7fELF':
                raise ValueError(f'hosted Rust binary is not an ELF: {elf}')

    debugfs, resize2fs = require_e2fsprogs('debugfs', 'resize2fs')
    with image.open('r+b') as disk:
        disk.truncate(image.stat().st_size + SCRATCH_HEADROOM)
    subprocess.run([resize2fs, image], check=True, capture_output=True)
    with tempfile.TemporaryDirectory(prefix='rust-host-stage-') as temporary:
        directory = Path(temporary)
        fixture = directory / 'mmap-fixture'
        write_mmap_fixture(fixture)
        commands = directory / 'commands'
        commands.write_text(
            'mkdir /sbin\nmkdir /usr\nmkdir /usr/libexec\nmkdir /usr/libexec/breenix\n'
            'mkdir /usr/libexec/breenix/rust-host\n'
            + ''.join(f'rm {path}\nwrite "{elf.resolve()}" {path}\n'
                      f'set_inode_field {path} mode 0100755\n'
                      f'set_inode_field {path} uid 0\nset_inode_field {path} gid 0\n'
                      for path, elf in entries)
            + f'rm {MMAP_FIXTURE_PATH}\nwrite "{fixture.resolve()}" {MMAP_FIXTURE_PATH}\n'
            f'set_inode_field {MMAP_FIXTURE_PATH} mode 0100444\n'
            f'set_inode_field {MMAP_FIXTURE_PATH} uid 0\nset_inode_field {MMAP_FIXTURE_PATH} gid 0\n'
        )
        subprocess.run([debugfs, '-w', '-f', commands, image], check=True, capture_output=True)
        # debugfs may exit successfully after a command failed. Read the file back.
        with fixture.open('rb') as source, subprocess.Popen(
            [debugfs, '-R', f'cat {MMAP_FIXTURE_PATH}', image],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        ) as copied:
            expected = hashlib.file_digest(source, 'sha256').digest()
            actual = hashlib.file_digest(copied.stdout, 'sha256').digest()
            if copied.wait() != 0 or actual != expected:
                raise ValueError(f'hosted Rust staging readback differs for {MMAP_FIXTURE_PATH}')
        stat = subprocess.check_output(
            [debugfs, '-R', f'stat {MMAP_FIXTURE_PATH}', image], stderr=subprocess.DEVNULL,
        )
        if b'Mode:  0444' not in stat or not re.search(rb'User:\s+0\s+Group:\s+0', stat):
            raise ValueError(f'hosted Rust staging metadata differs for {MMAP_FIXTURE_PATH}')
    # debugfs may exit successfully after an individual command failed.
    for path, elf in entries:
        data = subprocess.check_output([debugfs, '-R', f'cat {path}', image], stderr=subprocess.DEVNULL)
        stat = subprocess.check_output([debugfs, '-R', f'stat {path}', image], stderr=subprocess.DEVNULL)
        if (data != elf.read_bytes() or b'Mode:  0755' not in stat
                or not re.search(rb'User:\s+0\s+Group:\s+0', stat)):
            raise ValueError(f'hosted Rust staging readback differs for {path}')


CXX_PROBES = ('static-init-order', 'guarded-static', 'containers-threads')


def stage_cxx_runtime(image, selection, suite, probes):
    if 'cxx-runtime' not in selection.split(','):
        return {}
    if len(probes) != len(CXX_PROBES):
        raise ValueError('expected all three C++ probes')
    prefix = '/usr/libexec/breenix/cxx-runtime'
    entries = [('/sbin/suite-cxx-runtime', suite, '0755')] + [
        (f'{prefix}/{name}', probe, '0755')
        for name, probe in zip(CXX_PROBES, probes)
    ]
    return stage_files(image, [prefix], entries)


def stage_llvm(image, selection, suite, artifacts, corpus, linker):
    if 'llvm' not in selection.split(','):
        return {}
    prefix = '/usr/libexec/breenix/llvm'
    manifest = corpus / 'expect.json'
    inputs = sorted(corpus.glob('*.ll'))
    if not inputs or {path.name for path in inputs} != {
        entry['input'] for entry in json.loads(manifest.read_text())['objects']
    }:
        raise ValueError('LLVM corpus files differ from expect.json')
    entries = [('/sbin/suite-llvm', suite, '0755')]
    entries.extend((f'{prefix}/bin/{name}', artifacts / name, '0755')
                   for name in ('llc', 'llvm-mc', 'bld'))
    entries.extend([(f'{prefix}/lib/libc.a', artifacts / 'libc.a', '0444'),
                    (f'{prefix}/linker.ld', linker, '0444')])
    entries.extend((f'{prefix}/corpus/{path.name}', path, '0444') for path in [manifest, *inputs])
    return stage_files(image, [prefix, f'{prefix}/bin', f'{prefix}/lib', f'{prefix}/corpus'], entries)


def stage_dynamic(image, selection, arch, artifacts):
    if 'dynamic' not in selection.split(','):
        return {}
    if arch != 'x86_64':
        raise ValueError('the dynamic interpreter is only built for x86-64')
    prefix = '/usr/libexec/breenix/dynamic'
    entries = [('/sbin/suite-dynamic', artifacts / 'suite-dynamic', '0755')]
    entries.extend((f'/lib/{name}', artifacts / name, '0755') for name in ('ld-breenix.so', 'libc.so'))
    entries.extend((f'{prefix}/{name}', artifacts / name, '0755')
                   for name in ('dynamic-probe', 'missing-interp', 'libkd-probe.so',
                                'dynamic-panic-probe', 'libkd-panic.so'))
    return stage_files(image, ['/lib', prefix], entries)


def stage_rustc(image, selection, arch, suite, artifacts, sysroot, targets, corpus, kd, k5, kd_record):
    if 'rustc' not in selection.split(','):
        return {}
    if arch != 'x86_64':
        raise ValueError('the hosted rustc is only built for x86-64')
    prefix = '/usr/libexec/breenix/rustc'
    driver = k5['hosted_link']['driver']
    entries = [('/sbin/suite-rustc', suite, '0755'),
               (f'{prefix}/bin/rustc', artifacts / 'rustc', '0755'),
               (f'{prefix}/lib/{driver}', artifacts / driver, '0755'),
               ('/lib/ld-breenix.so', kd / 'ld-breenix.so', '0755'),
               ('/lib/libc.so', kd / 'libc.so', '0755')]
    expected = {f'{prefix}/bin/rustc': k5['hosted_rustc_sha256'],
                f'{prefix}/lib/{driver}': k5['hosted_driver_sha256'],
                '/lib/ld-breenix.so': kd_record['ld-breenix.so'],
                '/lib/libc.so': k5['hosted_link']['libc_sha256']}
    if kd_record['libc.so'] != expected['/lib/libc.so']:
        raise ValueError('KD libc differs from K5 build record')
    files = k5['sysroot']['files_sha256']
    for target in ('x86_64-breenix', 'x86_64-breenix-hosted'):
        relative = f'lib/rustlib/{target}/lib'
        archives = sorted((sysroot / relative).glob('*.rlib'))
        if not archives or {str(p.relative_to(sysroot)) for p in archives} != {
                name for name in files if name.startswith(relative + '/') }:
            raise ValueError(f'{target} sysroot differs from K5 build record')
        for source in archives:
            path = f'{prefix}/sysroot/{source.relative_to(sysroot)}'
            entries.append((path, source, '0444'))
            expected[path] = files[str(source.relative_to(sysroot))]
    for directory, sources in (('targets', [targets / f'{target}.json' for target in
                                  ('x86_64-breenix', 'x86_64-breenix-hosted')]),
                               ('corpus', sorted(p for p in corpus.iterdir() if p.is_file()))):
        if not sources:
            raise ValueError(f'empty rustc {directory}')
        entries.extend((f'{prefix}/{directory}/{source.name}', source, '0444') for source in sources)
    for path, source, _ in entries:
        if path in expected:
            with source.open('rb') as stream:
                if hashlib.file_digest(stream, 'sha256').hexdigest() != expected[path]:
                    raise ValueError(f'{path} differs from its build record')
    directories = ['/lib', prefix, f'{prefix}/bin', f'{prefix}/lib', f'{prefix}/sysroot',
                   f'{prefix}/sysroot/lib', f'{prefix}/sysroot/lib/rustlib']
    for target in ('x86_64-breenix', 'x86_64-breenix-hosted'):
        directories.extend([f'{prefix}/sysroot/lib/rustlib/{target}',
                            f'{prefix}/sysroot/lib/rustlib/{target}/lib'])
    directories.extend([f'{prefix}/targets', f'{prefix}/corpus'])
    return stage_files(image, directories, entries, extra_headroom=16384)


def stage_files(image, directories, entries, extra_headroom=0):
    expected = {}
    total_size = 0
    for path, source, mode in entries:
        with source.open('rb') as stream:
            if mode == '0755' and stream.read(4) != b'\x7fELF':
                raise ValueError(f'suite binary is not an ELF: {source}')
            stream.seek(0)
            expected[path] = hashlib.file_digest(stream, 'sha256').hexdigest()
        total_size += source.stat().st_size
    debugfs, resize2fs = require_e2fsprogs('debugfs', 'resize2fs')
    with image.open('r+b') as disk:
        disk.truncate(image.stat().st_size + total_size * 2 + 16 * 1024 * 1024 + extra_headroom)
    subprocess.run([resize2fs, image], check=True, capture_output=True)
    with tempfile.TemporaryDirectory(prefix='hosted-suite-stage-') as temporary:
        commands = Path(temporary) / 'commands'
        parents = ['/sbin', '/usr', '/usr/libexec', '/usr/libexec/breenix']
        commands.write_text(
            ''.join(f'mkdir {path}\n' for path in parents + directories)
            + ''.join(f'rm {path}\nwrite "{source.resolve()}" {path}\n'
                      f'set_inode_field {path} mode 010{mode}\n'
                      f'set_inode_field {path} uid 0\nset_inode_field {path} gid 0\n'
                      for path, source, mode in entries)
        )
        subprocess.run([debugfs, '-w', '-f', commands, image], check=True, capture_output=True)
    # debugfs can exit successfully even when one of its commands failed.
    for path, _, mode in entries:
        with subprocess.Popen([debugfs, '-R', f'cat {path}', image], stdout=subprocess.PIPE,
                              stderr=subprocess.DEVNULL) as copied:
            actual = hashlib.file_digest(copied.stdout, 'sha256').hexdigest()
            if copied.wait() != 0 or actual != expected[path]:
                raise ValueError(f'staging readback differs for {path}')
        stat = subprocess.check_output([debugfs, '-R', f'stat {path}', image],
                                       stderr=subprocess.DEVNULL, text=True)
        if (f'Type: regular    Mode:  {mode}' not in stat
                or not re.search(r'User:\s+0\s+Group:\s+0', stat)):
            raise ValueError(f'staging metadata differs for {path}')
    return expected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--arch', choices=('x86_64', 'aarch64'), required=True)
    parser.add_argument('--suite', required=True)
    parser.add_argument('--image', type=Path, required=True)
    args = parser.parse_args()
    release = PROGRAMS / f'target/hosted/{args.arch}-breenix-hosted/release'
    staged = stage_cxx_runtime(args.image, args.suite, release / 'suite-cxx-runtime',
                               [PROGRAMS / 'target/cxx-runtime' / args.arch / name for name in CXX_PROBES])
    staged.update(stage_llvm(args.image, args.suite, release / 'suite-llvm',
                             PROGRAMS / 'target/llvm' / args.arch,
                             ROOT / 'toolchain/rust-host/k4/corpus', PROGRAMS / 'linker.ld'))
    staged.update(stage_dynamic(
        args.image, args.suite, args.arch,
        Path(os.environ.get('BREENIX_KD_BUILD_DIR', PROGRAMS / 'target/kd')),
    ))
    if 'rustc' in args.suite.split(','):
        cache = Path(os.environ.get('BREENIX_RUSTC_PORT_BUILDS', ROOT / 'build/rustc-port'))
        sysroot = Path(os.environ.get('BREENIX_K5_SYSROOT', cache / 'sysroot'))
        kd = Path(os.environ.get('BREENIX_KD_BUILD_DIR', PROGRAMS / 'target/kd'))
        k5 = json.loads((ROOT / 'toolchain/rust-host/k5/x86_64.json').read_text())
        kd_record = json.loads((kd / 'build.json').read_text())
        staged.update(stage_rustc(args.image, args.suite, args.arch, release / 'suite-rustc',
                                  PROGRAMS / 'target/rustc' / args.arch, sysroot, ROOT,
                                  ROOT / 'toolchain/rust-host/k5/corpus', kd, k5, kd_record))
    stage(args.image, args.suite, release / 'suite-rust-host',
          release / 'rust-host-startup-probe', release / 'rust-host-process-probe',
          release / 'rust-host-unwind-probe')
    if staged:
        print(json.dumps(staged, sort_keys=True))


if __name__ == '__main__':
    main()
