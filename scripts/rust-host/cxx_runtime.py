#!/usr/bin/env python3
"""Configure and build the pinned, unmodified K3 C++ runtime workloads."""
import argparse
import json
import os
import re
import shutil
from pathlib import Path

from x0_runtime import BACKGROUND, FIRST_ERROR, build, digest, output

ROOT = Path(__file__).resolve().parents[2]
ARCHES = ('x86_64', 'aarch64')


def default_llvm():
    return Path(os.environ.get('BREENIX_LLVM_PROJECT', str(ROOT / 'rust-fork/src/llvm-project')))


def verify_llvm(root, llvm):
    pin = output('git', '-C', root / 'rust-fork', 'ls-tree', 'HEAD', 'src/llvm-project').split()[2]
    if output('git', '-C', llvm, 'rev-parse', 'HEAD') != pin or output('git', '-C', llvm, 'status', '--porcelain'):
        raise ValueError('LLVM workload must be clean and match the pinned gitlink')
    return pin


def configure_command(root, llvm, directory, arch):
    if arch not in ARCHES:
        raise ValueError(f'unsupported architecture: {arch}')
    flags = f'-nostdlibinc -isystem {root / "libs/libbreenix-libc/include"}'
    triple = f'{arch}-unknown-none'
    return (*BACKGROUND, 'cmake', '-G', 'Ninja', '-S', str(llvm / 'runtimes'), '-B', str(directory),
            '-DCMAKE_SYSTEM_NAME=Breenix', f'-DCMAKE_MODULE_PATH={root / "toolchain/cmake"}',
            f'-DCMAKE_SYSTEM_PROCESSOR={arch}',
            '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY',
            '-DCMAKE_C_COMPILER=/usr/bin/clang', '-DCMAKE_CXX_COMPILER=/usr/bin/clang++',
            '-DCMAKE_ASM_COMPILER=/usr/bin/clang', f'-DCMAKE_C_COMPILER_TARGET={triple}',
            f'-DCMAKE_CXX_COMPILER_TARGET={triple}', f'-DCMAKE_ASM_COMPILER_TARGET={triple}',
            f'-DCMAKE_C_FLAGS={flags}', f'-DCMAKE_CXX_FLAGS={flags}',
            '-DLLVM_ENABLE_RUNTIMES=libcxx;libcxxabi', '-DLLVM_INCLUDE_TESTS=OFF',
            '-DLIBCXX_CXX_ABI=libcxxabi', '-DLIBCXX_ENABLE_LOCALIZATION=OFF',
            '-DLIBCXX_ENABLE_THREADS=ON', '-DTHREADS_PREFER_PTHREAD_FLAG=ON',
            '-DLIBCXX_HAS_PTHREAD_API=ON', '-DLIBCXXABI_HAS_PTHREAD_API=ON',
            '-DLIBCXX_ENABLE_EXCEPTIONS=OFF', '-DLIBCXXABI_ENABLE_EXCEPTIONS=OFF',
            '-DLIBCXXABI_USE_LLVM_UNWINDER=OFF', '-DLIBCXX_ENABLE_SHARED=OFF',
            '-DLIBCXXABI_ENABLE_SHARED=OFF', '-DLIBCXX_ENABLE_STATIC=ON',
            '-DLIBCXXABI_ENABLE_STATIC=ON', '-DLIBCXX_ENABLE_STATIC_ABI_LIBRARY=OFF')


def run_stage(name, command, scratch, llvm):
    log = scratch / f'{name}.log'
    status = build(command, log, env=dict(os.environ, CARGO_BUILD_JOBS='4'))
    lines = log.read_text(errors='replace').splitlines()
    def normalize(line):
        for path, label in ((llvm, '<llvm-project>'), (scratch, '<scratch>'),
                            (ROOT, '<repo>'), (Path.home(), '<home>')):
            line = line.replace(str(path), label)
        return line
    error = next((normalize(line).strip() for line in lines if FIRST_ERROR.search(line)), None)
    if status and not error:
        error = 'Process exceeded 55 seconds' if status == 124 else f'Process exited {status}'
    return {'command': [normalize(str(arg)) for arg in command], 'exit_status': status,
            'first_error': error, 'warnings': [normalize(line) for line in lines
              if re.search(r'^(?:CMake Warning\b|.*\bwarning:)', line)], 'log_sha256': digest(log)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('arch', choices=ARCHES)
    parser.add_argument('--llvm-project', type=Path, default=default_llvm())
    parser.add_argument('--scratch', type=Path, required=True)
    parser.add_argument('--record', type=Path, required=True)
    args = parser.parse_args()
    llvm = args.llvm_project.resolve()
    pin = verify_llvm(ROOT, llvm)
    scratch = args.scratch.resolve()
    scratch.mkdir(parents=True, exist_ok=False)
    directory = scratch / 'build'
    configure = run_stage('configure', configure_command(ROOT, llvm, directory, args.arch), scratch, llvm)
    built = None
    archives = {}
    if configure['exit_status'] == 0:
        built = run_stage('build', (*BACKGROUND, 'cmake', '--build', str(directory), '--parallel', '4',
                                    '--target', 'cxx_static', 'cxxabi_static'), scratch, llvm)
        if built['exit_status'] == 0:
            for name, component in (('libc++.a', 'libcxx'), ('libc++abi.a', 'libcxxabi')):
                archive, = directory.rglob(name)
                notices = archive.parent / (name + '.LICENSE.TXT')
                shutil.copyfile(llvm / component / 'LICENSE.TXT', notices)
                archives[name] = {'sha256': digest(archive), 'notice_sha256': digest(notices)}
    verify_llvm(ROOT, llvm)
    record = {'claim-lint:ok': 'claim-lint:ok: compiler target triples, see toolchain/rust-host/k3/README.md',
              'arch': args.arch, 'llvm_project': pin, 'configure': configure,
              'build': built, 'archives': archives,
              'generic_collision_targets': ['cxxabi_shared', 'cxxabi_static']}
    args.record.parent.mkdir(parents=True, exist_ok=True)
    args.record.write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps(record, indent=2))
    return 0 if configure['exit_status'] == 0 else 1


if __name__ == '__main__':
    raise SystemExit(main())
