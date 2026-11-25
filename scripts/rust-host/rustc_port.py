#!/usr/bin/env python3
"""Stage the K5 twin sysroot and record the Mac compiler's corpus outputs."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fork', required=True, type=Path)
    parser.add_argument('--cache', type=Path, default=CACHE)
    args = parser.parse_args()
    try:
        record_twin(args.fork.resolve(), args.cache.resolve())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f'{error}\n')


if __name__ == '__main__':
    main()
