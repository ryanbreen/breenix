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
CARGO_HEADROOM = 64 * 1024 * 1024
RUST_TARGETS = ('x86_64-breenix', 'x86_64-breenix-hosted')


def sha256(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


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


def rustc_fixture_entries(artifacts, sysroot, targets, k5):
    driver = k5['hosted_link']['driver']
    yield 'bin/rustc', artifacts / 'rustc', '0755', k5['hosted_rustc_sha256']
    yield f'lib/{driver}', artifacts / driver, '0755', k5['hosted_driver_sha256']
    files = k5['sysroot']['files_sha256']
    for target in RUST_TARGETS:
        relative = f'lib/rustlib/{target}/lib'
        archives = sorted((sysroot / relative).glob('*.rlib'))
        if not archives or {str(p.relative_to(sysroot)) for p in archives} != {
                name for name in files if name.startswith(relative + '/') }:
            raise ValueError(f'{target} sysroot differs from K5 build record')
        for source in archives:
            name = str(source.relative_to(sysroot))
            yield f'sysroot/{name}', source, '0444', files[name]
    for target in RUST_TARGETS:
        name = f'{target}.json'
        yield f'targets/{name}', targets / name, '0444', None


def verify_hosted_libc(kd, records, k5, rustc, cargo):
    libc = kd / 'libc.so'
    pin = json.loads((records / 'hosted-libc-x86_64.json').read_text())
    runtime = json.loads((kd / 'build.json').read_text())
    if runtime['libc.so'] != pin['sha256'] or sha256(libc) != pin['sha256']:
        raise ValueError(f'{libc}: SHA-256 differs from the hosted libc pin')
    libdir = subprocess.check_output(['rustc', '--print', 'target-libdir'], cwd=ROOT,
                                     text=True, timeout=10).strip()
    readobj = Path(libdir).parent / 'bin/llvm-readobj'

    def symbols(path, needed, soname=None):
        elf, = json.loads(subprocess.check_output(
            [readobj, '--elf-output-style=JSON', '--dyn-symbols', '--needed-libs', path],
            timeout=15))
        summary = elf['FileSummary']
        if (summary['Format'] != 'elf64-x86-64' or sorted(elf['NeededLibraries']) != sorted(needed)
                or (soname is not None and summary['LoadName'] != soname)):
            raise ValueError(f'{path.name}: unexpected hosted ELF dependencies')
        imports, exports = set(), set()
        # ELF gABI §5.1–5.4. The null symbol has no name. Definitions must
        # be global or weak and externally visible, as in ld-breenix lookup.
        for entry in elf['DynamicSymbols']:
            symbol = entry['Symbol']
            name = symbol['Name']['Name']
            if not name:
                continue
            if symbol['Section']['Value'] == 0:
                imports.add(name)
            elif symbol['Binding']['Value'] in (1, 2) and (symbol['Other']['Value'] & 3) in (0, 3):
                exports.add(name)
        return imports, exports

    def verify(path, imports, exports):
        missing = sorted(imports - exports)
        if missing:
            raise ValueError(f'{path.name}: unresolved hosted imports: {", ".join(missing)}')

    libc_imports, libc_exports = symbols(libc, [], libc.name)
    verify(libc, libc_imports, set())
    driver = rustc / k5['hosted_link']['driver']
    driver_imports, driver_exports = symbols(driver, [libc.name], driver.name)
    verify(driver, driver_imports, libc_exports)
    compiler = rustc / 'rustc'
    compiler_imports, _ = symbols(compiler, [driver.name, libc.name])
    verify(compiler, compiler_imports, libc_exports | driver_exports)
    if cargo is not None:
        cargo_imports, _ = symbols(cargo, [libc.name])
        verify(cargo, cargo_imports, libc_exports)
    return runtime


def hosted_runtime_entries(kd, runtime, already_staged):
    entries, expected = [], {}
    for name in ('ld-breenix.so', 'libc.so'):
        path = f'/lib/{name}'
        if path in already_staged:
            if already_staged[path] != runtime[name]:
                raise ValueError(f'{path} differs from the hosted runtime build record')
        else:
            entries.append((path, kd / name, '0755'))
            expected[path] = runtime[name]
    return entries, expected


def stage_rustc(image, selection, arch, suite, artifacts, sysroot, targets, corpus, kd, k5,
                kd_record, *, already_staged=None):
    if 'rustc' not in selection.split(','):
        return {}
    if arch != 'x86_64':
        raise ValueError('the hosted rustc is only built for x86-64')
    prefix = '/usr/libexec/breenix/rustc'
    entries, expected = hosted_runtime_entries(kd, kd_record, already_staged or {})
    entries.append(('/sbin/suite-rustc', suite, '0755'))
    for name, source, mode, digest in rustc_fixture_entries(artifacts, sysroot, targets, k5):
        path = f'{prefix}/{name}'
        entries.append((path, source, mode))
        if digest is not None:
            expected[path] = digest
    sources = sorted(path for path in corpus.iterdir() if path.is_file())
    if not sources:
        raise ValueError('empty rustc corpus')
    entries.extend((f'{prefix}/corpus/{source.name}', source, '0444') for source in sources)
    directories = ['/lib', prefix, f'{prefix}/bin', f'{prefix}/lib', f'{prefix}/sysroot',
                   f'{prefix}/sysroot/lib', f'{prefix}/sysroot/lib/rustlib']
    for target in RUST_TARGETS:
        directories.extend([f'{prefix}/sysroot/lib/rustlib/{target}',
                            f'{prefix}/sysroot/lib/rustlib/{target}/lib'])
    directories.extend([f'{prefix}/targets', f'{prefix}/corpus'])
    return stage_files(image, directories, entries, extra_headroom=16384, recorded=expected)


def ext2_stats(image, debugfs):
    output = subprocess.run([debugfs, '-R', 'stats', image], check=True,
                            capture_output=True, text=True,
                            env={**os.environ, 'LC_ALL': 'C'}).stdout
    return dict(line.split(':', 1) for line in output.splitlines() if ':' in line)


def stage_files(image, directories, entries, extra_headroom=0, *, align_to_block=False,
                recorded=None, scratch_headroom=None):
    expected = {}
    total_size = 0
    for path, source, mode in entries:
        with source.open('rb') as stream:
            if mode == '0755' and stream.read(4) != b'\x7fELF':
                raise ValueError(f'suite binary is not an ELF: {source}')
            stream.seek(0)
            expected[path] = hashlib.file_digest(stream, 'sha256').hexdigest()
        if recorded is not None and path in recorded and expected[path] != recorded[path]:
            raise ValueError(f'{path} differs from its build record')
        total_size += source.stat().st_size
    debugfs, resize2fs = require_e2fsprogs('debugfs', 'resize2fs')
    growth = (total_size + scratch_headroom if scratch_headroom is not None
              else total_size * 2 + 16 * 1024 * 1024 + extra_headroom)
    target_size = image.stat().st_size + growth
    resize_command = [resize2fs, image]
    if align_to_block or scratch_headroom is not None:
        block_size = int(ext2_stats(image, debugfs).get('Block size', '0'))
        if block_size <= 0:
            raise ValueError(f'cannot read ext2 block size for {image}')
        blocks = (target_size + block_size - 1) // block_size
        target_size = blocks * block_size
        # Automatic sizing can round down even when the backing length is aligned.
        resize_command.append(str(blocks))
    with image.open('r+b') as disk:
        disk.truncate(target_size)
    subprocess.run(resize_command, check=True, capture_output=True)
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
    if scratch_headroom is not None:
        free = int(ext2_stats(image, debugfs)['Free blocks']) * block_size
        if image.stat().st_size != target_size or free < scratch_headroom:
            raise ValueError(f'{image}: unexpected backing size or insufficient scratch space '
                             f'({free} free bytes, need {scratch_headroom})')
    return expected


def rust_link_fixture_entries(artifacts, fixture, k6, *, tools_only=False):
    build = json.loads((artifacts / 'build.json').read_text())['files_sha256']
    if not tools_only:
        yield '/sbin/suite-rust-link', artifacts / 'suite-rust-link', '0755', build['suite-rust-link']
    yield 'bin/bld', artifacts / 'bld', '0755', build['bld']
    if tools_only:
        for name in ('linker.ld', 'libc.a'):
            relative = f'lib/tools/{name}'
            yield f'lib/{name}', fixture / relative, '0444', k6['inputs'][relative]['sha256']
    else:
        for name, record in k6['inputs'].items():
            yield name, fixture / name, '0444', record['sha256']


def stage_rust_link(image, arch, artifacts, fixture, records):
    if arch != 'x86_64':
        raise ValueError('the recorded Rust linker fixture is only built for x86-64')
    prefix = '/usr/libexec/breenix/rust-link'
    k5 = json.loads((records / 'k5/x86_64.json').read_text())
    receipt = records / 'k6/x86_64.json'
    expectations = records / 'k6/expect.json'
    k6 = json.loads(receipt.read_text())
    expect = json.loads(expectations.read_text())
    if (k6['rustc_sha256'], k6['rustc_commit']) != (k5['mac_rustc_sha256'], k5['fork_commit']):
        raise ValueError('K6 compiler identity differs from the K5 build record')

    entries = []
    expected = {}

    def add(path, source, mode, digest):
        if sha256(source) != digest or (path in expected and expected[path] != digest):
            raise ValueError(f'{path} differs from its build record')
        if path not in expected:
            entries.append((path, source, mode))
            expected[path] = digest

    # The K6 input inventory includes the exact libbunwind.a used for the
    # hosted replay. An independently rebuilt archive cannot replace it.
    for name, source, mode, digest in rust_link_fixture_entries(artifacts, fixture, k6):
        add(name if name.startswith('/') else f'{prefix}/{name}', source, mode, digest)
    for name, digest in k5['sysroot']['files_sha256'].items():
        source = f'sysroot/{name}'
        add(f'{prefix}/{source}', fixture / source, '0444', digest)

    directories = set()
    for name, link in k6['links'].items():
        relative = f'links/{name}/argv.json'
        source = fixture / relative
        argv_bytes = source.read_bytes()
        if json.loads(argv_bytes) != link['argv']:
            raise ValueError(f'{relative} differs from its build record')
        add(f'{prefix}/{relative}', source, '0444', hashlib.sha256(argv_bytes).hexdigest())
        reference = f'links/{name}/reference.elf'
        if name in expect and expect[name]['reference_sha256'] != link['reference_sha256']:
            raise ValueError(f'{reference} differs between the K6 record and expect.json')
        add(f'{prefix}/{reference}', fixture / reference, '0755', link['reference_sha256'])
        # Some recorded -L directories are empty. Keep the relative layout.
        directories.update(f'{prefix}/{arg}' for arg, option in zip(link['argv'][1:], link['argv'])
                           if option == '-L')
    control = expect['control']['archive']
    add(f'{prefix}/{control}', fixture / control, '0444', expect['control']['sha256'])
    metadata = expect['metadata']['archive']
    add(f'{prefix}/{metadata}', records / 'k6' / metadata, '0444', expect['metadata']['sha256'])
    for source in (receipt, expectations):
        add(f'{prefix}/{source.name}', source, '0444', sha256(source))
    for path in [*expected, *directories]:
        directories.update(str(parent) for parent in Path(path).parents
                           if str(parent).startswith(prefix))

    # Budget for the staged payload and a payload-sized replay workspace,
    # plus 16 MiB of headroom. Ext2 allocation also draws from this budget.
    staged = stage_files(image, sorted(directories), entries, align_to_block=True)
    if staged != expected:
        raise ValueError('Rust linker staging readback differs from the build records')
    return staged


def stage_cargo(image, arch, suite, artifacts, rust_link, sysroot, fixture, kd, k7_stage,
                records, kd_record, already_staged):
    if arch != 'x86_64':
        raise ValueError('the recorded Cargo fixture is only built for x86-64')
    prefix = '/usr/libexec/breenix/cargo'
    k5 = json.loads((records / 'k5/x86_64.json').read_text())
    k6 = json.loads((records / 'k6/x86_64.json').read_text())
    k7 = json.loads((records / 'k7/x86_64.json').read_text())
    if (any(k5[name] != digest for name, digest in k7['k5_compiler_digests'].items())
            or (k6['rustc_sha256'], k6['rustc_commit']) !=
            (k5['mac_rustc_sha256'], k5['fork_commit'])):
        raise ValueError('K7 and K6 compiler identities must match the K5 build record')

    digests = k7['fixture_sha256']
    files = [
        *rustc_fixture_entries(artifacts, sysroot, ROOT, k5),
        *rust_link_fixture_entries(rust_link, fixture, k6, tools_only=True),
        ('bin/cargo', k7_stage / 'bin/cargo', '0755', k7['cargo_sha256']),
        ('cargo-home/config.toml', records / 'k7/corpus/no-network/.cargo/config.toml', '0444', None),
    ]
    files.extend((name, records / 'k7' / name, '0444', digest)
                 for name, digest in digests.items() if name.startswith('corpus/'))
    entries, expected = hosted_runtime_entries(kd, kd_record, already_staged)
    entries.append(('/sbin/suite-cargo', suite, '0755'))
    for name, source, mode, digest in files:
        path = f'{prefix}/{name}'
        entries.append((path, source, mode))
        expected[path] = digests[name] if digest is None else digest
    directories = {'/lib'}
    directories.update(str(parent) for path, _, _ in entries for parent in Path(path).parents
                       if parent == Path(prefix) or Path(prefix) in parent.parents)
    return stage_files(image, sorted(directories), entries, recorded=expected,
                       scratch_headroom=CARGO_HEADROOM)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--arch', choices=('x86_64', 'aarch64'), required=True)
    parser.add_argument('--suite', required=True)
    parser.add_argument('--image', type=Path, required=True)
    args = parser.parse_args()
    suites = args.suite.split(',')
    release = PROGRAMS / f'target/hosted/{args.arch}-breenix-hosted/release'
    kd = Path(os.environ.get('BREENIX_KD_BUILD_DIR', PROGRAMS / 'target/kd'))
    cache = Path(os.environ.get('BREENIX_RUSTC_PORT_BUILDS', ROOT / 'build/rustc-port'))
    records = ROOT / 'toolchain/rust-host'
    rustc = PROGRAMS / 'target/rustc' / args.arch
    rust_link = PROGRAMS / 'target/rust-link' / args.arch
    fixture = Path(os.environ.get('BREENIX_K6_FIXTURE', cache / 'k6/fixture'))
    if 'rustc' in suites or 'cargo' in suites:
        if args.arch != 'x86_64':
            parser.error('hosted rustc and Cargo staging requires x86-64')
        sysroot = Path(os.environ.get('BREENIX_K5_SYSROOT', cache / 'sysroot'))
        k7_stage = Path(os.environ.get('BREENIX_K7_STAGE', cache / 'k7/stage1'))
        k5 = json.loads((records / 'k5/x86_64.json').read_text())
        cargo = k7_stage / 'bin/cargo' if 'cargo' in suites else None
        kd_record = verify_hosted_libc(kd, records, k5, rustc, cargo)

    staged = stage_cxx_runtime(args.image, args.suite, release / 'suite-cxx-runtime',
                               [PROGRAMS / 'target/cxx-runtime' / args.arch / name for name in CXX_PROBES])
    staged.update(stage_llvm(args.image, args.suite, release / 'suite-llvm',
                             PROGRAMS / 'target/llvm' / args.arch,
                             ROOT / 'toolchain/rust-host/k4/corpus', PROGRAMS / 'linker.ld'))
    staged.update(stage_dynamic(args.image, args.suite, args.arch, kd))
    if 'rustc' in suites:
        staged.update(stage_rustc(args.image, args.suite, args.arch, release / 'suite-rustc',
                                  rustc, sysroot, ROOT,
                                  records / 'k5/corpus', kd, k5, kd_record, already_staged=staged))
    if 'rust-link' in suites:
        staged.update(stage_rust_link(args.image, args.arch, rust_link, fixture, records))
    if 'cargo' in suites:
        staged.update(stage_cargo(args.image, args.arch, release / 'suite-cargo', rustc, rust_link,
                                  sysroot, fixture, kd, k7_stage, records, kd_record, staged))
    stage(args.image, args.suite, release / 'suite-rust-host',
          release / 'rust-host-startup-probe', release / 'rust-host-process-probe',
          release / 'rust-host-unwind-probe')
    if staged:
        print(json.dumps(staged, sort_keys=True))


if __name__ == '__main__':
    main()
