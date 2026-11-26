#!/usr/bin/env python3
"""Link and stage the K5 hosted compiler and record the twin sysroot."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[2]
K5 = ROOT / 'toolchain/rust-host/k5'
CORPUS = K5 / 'corpus'
CACHE = Path.home() / 'Library/Caches/breenix-e25/k5/twin'
MAC = 'aarch64-apple-darwin'
TOOL = 'x86_64-breenix'
HOSTED = 'x86_64-breenix-hosted'
TARGETS = (TOOL, HOSTED)
VERSION_KEYS = ('release', 'commit-hash', 'commit-date', 'LLVM version')
DRIVER_NAME = re.compile(r'librustc_driver-[0-9a-f]+\.so')


def guest_environment(targets):
    """Build the environment used by suite_rustc.rs for compiler spawns."""
    return {"RUST_TARGET_PATH": str(targets)}


def output(*args):
    return subprocess.check_output(args, text=True, timeout=30).strip()


def sha256(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def fork_status(fork):
    return output('git', '-C', fork, 'status', '--porcelain', '--ignore-submodules=none',
                  '--', 'compiler', 'src/llvm-project')


def write_json(path, record):
    contents = json.dumps(record, indent=2) + '\n'
    if '/Users/' in contents:
        raise ValueError(f'{path.name} includes a local path')
    path.write_text(contents)


def link_hosted(args):
    """Use KD's driver link and give the K5 launcher its final RUNPATH."""
    from kd_link import main as kd_link

    flags = args[2:] if args[:2] == ['-flavor', 'gnu'] else args
    link = [flag for arg in flags
            for flag in (arg[4:].split(',') if arg.startswith('-Wl,') else [arg])]
    if any(flag in ('-shared', '--shared') for flag in link) or not any(
            DRIVER_NAME.fullmatch(Path(flag).name) for flag in link):
        kd_link(args)
        return

    # Bootstrap can pass its own -rpath. LLD concatenates repeated paths.
    # /lib is already in ld-breenix.so's default search path.
    filtered = []
    flags = iter(link)
    for flag in flags:
        if flag in ('-rpath', '--rpath'):
            next(flags)
        elif (not flag.startswith(('-rpath=', '--rpath=')) and
              flag not in ('-lc', '-lc++', '-lbunwind') and Path(flag).name != 'libc.a'):
            filtered.append(flag)
    inputs = Path(os.environ['BREENIX_KD_BUILD_DIR'])
    maps = Path(os.environ['BREENIX_X2_STAGE']) / 'maps'
    maps.mkdir(parents=True, exist_ok=True)
    lld = os.environ['BREENIX_X2_LLD']
    os.execv(lld, [lld, *filtered, '--no-dependent-libraries', '--hash-style=sysv',
                   '--fatal-warnings', '-z', 'now', '-z', 'relro', '-rpath', '$ORIGIN/../lib',
                   inputs / 'breenix-crt.o', '--allow-shlib-undefined',
                   '--image-base=0x40000000', '--dynamic-linker=/lib/ld-breenix.so',
                   '--push-state', '--no-as-needed', inputs / 'libc.so', '--pop-state',
                   f'-Map={maps / "rustc.map"}'])


def hosted_artifacts(cache):
    hosted = cache / 'hosted'
    drivers = sorted(path for path in (hosted / 'lib').glob('librustc_driver-*.so')
                     if DRIVER_NAME.fullmatch(path.name))
    if len(drivers) != 1:
        raise ValueError(f'{hosted / "lib"}: expected one hosted rustc driver, found {len(drivers)}')
    compiler = hosted / 'bin/rustc'
    driver, = drivers
    return (compiler, driver, *(hosted / 'maps' / f'{elf.name}.map' for elf in (compiler, driver)))


def record_hosted_link(cache):
    compiler, driver, *maps = hosted_artifacts(cache)
    path = K5 / 'x86_64.json'
    record = json.loads(path.read_text())
    record['hosted_rustc_sha256'] = sha256(compiler)
    record['hosted_driver_sha256'] = sha256(driver)
    record['hosted_link'] = {
        'driver': driver.name,
        'libc_sha256': sha256(cache / 'hosted/lib/libc.so'),
        'maps_sha256': {item.name: sha256(item) for item in maps},
    }
    write_json(path, record)


def stage_hosted(cache, stage):
    record = json.loads((K5 / 'x86_64.json').read_text())
    hosted = record['hosted_link']
    compiler, driver, *maps = hosted_artifacts(cache)
    if driver.name != hosted['driver']:
        raise ValueError(f'{driver.name}: expected {hosted["driver"]} from the K5 build record')

    expected = {
        compiler.name: record['hosted_rustc_sha256'],
        driver.name: record['hosted_driver_sha256'],
        **hosted['maps_sha256'],
    }
    libc = cache / 'hosted/lib/libc.so'
    if sha256(libc) != hosted['libc_sha256']:
        raise ValueError(f'{libc}: SHA-256 differs from the K5 build record')
    stage.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=stage.parent) as scratch:
        pending = Path(scratch)
        for source in (compiler, driver, *maps):
            artifact = pending / source.name
            shutil.copy2(source, artifact)
            if sha256(artifact) != expected[artifact.name]:
                raise ValueError(f'{source}: SHA-256 differs from the K5 build record')
        stage.mkdir(exist_ok=True)
        for artifact in pending.iterdir():
            artifact.replace(stage / artifact.name)
        for previous in stage.glob('librustc_driver-*.so'):
            if previous.name != driver.name and DRIVER_NAME.fullmatch(previous.name):
                previous.unlink()
                previous.with_suffix('.so.map').unlink(missing_ok=True)
    print(f'Staged the K5 compiler, driver and link maps in {stage}')


def stage_sysroot(source, sysroot, rustc):
    for target in TARGETS:
        relative = Path('lib/rustlib') / target / 'lib'
        archives = sorted((source / relative).glob('*.rlib'))
        for crate in ('core', 'alloc', 'std'):
            if sum(path.name.startswith(f'lib{crate}-') for path in archives) != 1:
                raise ValueError(f'{target} requires one {crate} rlib')
        directory = sysroot / relative
        directory.mkdir(parents=True)
        for archive in archives:
            artifact = directory / archive.name
            shutil.copyfile(archive, artifact)
            # rustc's metadata dump rejects an archive from a different compiler.
            # A mismatch can report success, so also require the decoded target.
            metadata = output(rustc, '-Zls=root', artifact)
            if f'triple {target}' not in metadata.splitlines():
                raise ValueError(f'{artifact.name}: {metadata}')


def compile_corpus(rustc, sysroot, scratch):
    common = [rustc, '--edition=2021', '--sysroot', sysroot, '-Dwarnings', '--emit=obj']
    for target in TARGETS:
        command = [*common, '--target', ROOT / f'{target}.json']
        subprocess.run([*command, '--crate-type=lib', 'hello.rs', '-o', scratch / f'{target}.o'],
                       cwd=CORPUS, check=True, timeout=15)
        subprocess.run([*command, 'uses-std.rs', '-o', scratch / 'uses-std.o'],
                       cwd=CORPUS, check=True, timeout=15)
    failed = subprocess.run([*common, '--target', ROOT / f'{TOOL}.json',
                             '--error-format=json', 'type-error.rs',
                             '-o', scratch / 'type-error.o'],
                            cwd=CORPUS, capture_output=True, text=True, timeout=15)
    if failed.returncode != 1 or (scratch / 'type-error.o').exists():
        raise ValueError('type-error.rs did not fail with exit 1 and no object')
    diagnostics = [json.loads(line) for line in failed.stderr.splitlines()]
    error, = (diagnostic for diagnostic in diagnostics if diagnostic['level'] == 'error'
              and (diagnostic.get('code') or {}).get('code') == 'E0308')
    primary, = (span for span in error['spans'] if span['is_primary'])
    if primary['file_name'] != 'type-error.rs':
        raise ValueError('type error was reported outside type-error.rs')
    return {
        'hello_o_sha256': sha256(scratch / f'{TOOL}.o'),
        'type_error': f"type-error.rs:{primary['line_start']}:{primary['column_start']}",
    }


def record_twin(fork, cache):
    before = (cache / 'o5-before').read_text().strip()
    after = fork_status(fork)
    (cache / 'o5-after').write_text(after + '\n' if after else '')
    if before or after:
        raise ValueError('O5: compiler/ or src/llvm-project has changes')
    pin = tomllib.loads((ROOT / 'rust-fork.toml').read_text())['hosted']['commit']
    if output('git', '-C', fork, 'rev-parse', 'HEAD') != pin:
        raise ValueError('K5 requires the pinned hosted Rust fork')
    stage = json.loads((cache / 'bootstrap.json').read_text())
    if stage['exit_status'] != 0 or stage.get('timed_out'):
        raise ValueError('twin bootstrap did not complete successfully')
    attempts = {}
    for name in ('bootstrap-first', 'bootstrap'):
        receipt = cache / f'{name}.json'
        if receipt.is_file():
            attempts[name] = {
                **json.loads(receipt.read_text()),
                'log_sha256': sha256(cache / f'{name}.log'),
            }
    config = tomllib.loads((cache / 'bootstrap.toml').read_text())
    bootstrap = config['build']
    hosts = [Path(host).stem for host in bootstrap['host']]
    targets = [Path(target).stem for target in bootstrap['target']]
    if hosts != [MAC, HOSTED] or targets != list(TARGETS) or not 1 <= bootstrap['jobs'] <= 4:
        raise ValueError('bootstrap must build both twin hosts and both targets with at most four jobs')

    build = Path(bootstrap['build-dir'])
    mac = build / MAC / 'stage1'
    hosted = build / HOSTED / 'stage1'
    rustc = mac / 'bin/rustc'
    verbose = output(rustc, '-vV').splitlines()
    identity = dict(line.split(': ', 1) for line in verbose[1:])
    if identity['host'] != MAC or identity['commit-hash'] != pin:
        raise ValueError('Mac twin did not report the pinned compiler identity')
    llvm_versions = {host: output(config['target'][key]['llvm-config'], '--version')
                     for host, key in zip(hosts, bootstrap['host'], strict=True)}
    if set(llvm_versions.values()) != {identity['LLVM version']}:
        raise ValueError('twin LLVM versions differ')
    llvm_commit = output('git', '-C', fork / 'src/llvm-project', 'rev-parse', 'HEAD')
    if llvm_commit != output('git', '-C', fork, 'ls-tree', pin, 'src/llvm-project').split()[2]:
        raise ValueError('LLVM checkout differs from the fork gitlink')
    hosted_rustc = hosted / 'bin/rustc'
    driver, = (hosted / 'lib').glob('librustc_driver-*.so')
    for artifact in (hosted_rustc, driver):
        with artifact.open('rb') as stream:
            if stream.read(4) != b'\x7fELF':
                raise ValueError(f'{artifact.name} is not an ELF artifact')

    sysroot = cache / 'sysroot'
    if sysroot.exists():
        raise ValueError('choose a fresh cache output for the staged sysroot')
    with tempfile.TemporaryDirectory(dir=cache) as directory:
        scratch = Path(directory)
        pending = scratch / 'sysroot'
        stage_sysroot(mac, pending, rustc)
        expectations = compile_corpus(rustc, pending, scratch)
        files = {str(path.relative_to(pending)): sha256(path)
                 for path in sorted(pending.rglob('*.rlib'))}
        shutil.copyfile(scratch / f'{TOOL}.o', cache / 'hello.o')
        pending.rename(sysroot)

    # Keep both compilers accessible beside their shared target sysroot.
    (cache / 'mac').symlink_to(mac, target_is_directory=True)
    for name in ('bin', 'lib'):
        (cache / 'hosted' / name).symlink_to(hosted / name, target_is_directory=True)
    expected = {
        'rustc_vv': [f'{key}: {identity[key]}' for key in VERSION_KEYS],
        'hosted_host': HOSTED,
        **expectations,
    }
    record = {
        'fork_commit': pin,
        'compiler_tree': output('git', '-C', fork, 'rev-parse', f'{pin}:compiler'),
        'llvm_commit': llvm_commit,
        'llvm_versions': llvm_versions,
        'pinned_status_before': before,
        'pinned_status_after': after,
        'bootstrap': {
            'command': ['taskpolicy', '-b', 'nice', '-n', '15', 'python3', 'x.py', 'build',
                        '--stage', '1', 'compiler/rustc', 'library/std',
                        '--config', '<cache>/bootstrap.toml', '--verbose'],
            'hosts': hosts,
            'targets': targets,
            'jobs': bootstrap['jobs'],
            'attempts': attempts,
        },
        'mac_rustc_sha256': sha256(rustc),
        'hosted_rustc_sha256': sha256(hosted_rustc),
        'hosted_driver_sha256': sha256(driver),
        'sysroot': {
            'rustc_version': verbose[0],
            'commit_hash': pin,
            'files_sha256': files,
        },
    }
    write_json(K5 / 'x86_64.json', record)
    write_json(CORPUS / 'expect.json', expected)


def main():
    if sys.argv[1:2] == ['--bootstrap-linker']:
        link_hosted(sys.argv[2:])
        return
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--fork', type=Path, help='record and stage the twin sysroot')
    mode.add_argument('--record-hosted-link', action='store_true',
                      help='record the hosted artifacts from the K5 bootstrap link')
    mode.add_argument('--stage-hosted', type=Path,
                      help='stage the verified hosted compiler and maps in this directory')
    parser.add_argument('--cache', type=Path, default=CACHE)
    args = parser.parse_args()
    try:
        if args.fork:
            record_twin(args.fork.resolve(), args.cache.resolve())
        elif args.record_hosted_link:
            record_hosted_link(args.cache)
        else:
            stage_hosted(args.cache, args.stage_hosted)
    except (KeyError, OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f'{error}\n')


if __name__ == '__main__':
    main()
