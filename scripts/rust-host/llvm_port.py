#!/usr/bin/env python3
"""Configure pinned LLVM for Breenix using native TableGen."""
import argparse
import json
import os
import re
import shlex
import shutil
from pathlib import Path

from cxx_runtime import (ARCHES, BACKGROUND, CLANGXX, LLVM_BIN, POSIX_FLAG,
                         default_llvm, digest, output, run_stage, verify_llvm)

ROOT = Path(__file__).resolve().parents[2]


def bootstrap_targets(root):
    text = (root / 'rust-fork/bootstrap.example.toml').read_text()
    return re.search(r'^#llvm\.targets = "([^"]+)"$', text, re.MULTILINE).group(1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('arch', choices=ARCHES)
    parser.add_argument('--scratch', type=Path, required=True)
    parser.add_argument('--native-run', type=Path)
    parser.add_argument('--resume', action='store_true')
    args = parser.parse_args()
    llvm = default_llvm().resolve()
    pin = verify_llvm(ROOT, llvm)
    run = args.scratch.resolve()
    run.mkdir(parents=True, exist_ok=args.resume)
    targets = bootstrap_targets(ROOT)
    allowed = json.loads((ROOT / 'toolchain/rust-host/k4/native-allowlist.json').read_text())
    flat = {k: v for category in allowed.values() for k, v in category.items()}
    native = run / 'native'
    common = ('-DLLVM_INCLUDE_TESTS=OFF', '-DLLVM_INCLUDE_BENCHMARKS=OFF',
              '-DLLVM_ENABLE_ZLIB=OFF', '-DLLVM_ENABLE_ZSTD=OFF',
              '-DLLVM_ENABLE_LIBXML2=OFF', '-DLLVM_ENABLE_TERMINFO=OFF',
              f'-DLLVM_TARGETS_TO_BUILD={targets}')
    configure_host = (*BACKGROUND, 'cmake', '-G', 'Ninja', '-S', str(llvm / 'llvm'),
                      '-B', str(native), '-DCMAKE_BUILD_TYPE=Release', *common)
    record = {'arch': args.arch, 'llvm_project': pin, 'targets': targets,
              'clang++': {'path': CLANGXX, 'version': output(CLANGXX, '--version').splitlines()[0]},
              'native_allowlist': allowed}
    def stage(name, command, timeout=55, allowlist=None):
        result = run_stage(name, command, run, llvm, timeout=timeout, allowlist=allowlist)
        result['allowlist'] = allowlist or {}
        record[name] = result
        (run / 'record.json').write_text(json.dumps(record, indent=2) + '\n')
        if result['first_error'] is not None:
            raise RuntimeError(f"{name}: {result['first_error']}")
    if args.native_run or args.resume:
        prior = args.native_run.resolve() if args.native_run else run
        prior_record = json.loads((prior / 'record.json').read_text())
        if prior_record['llvm_project'] != pin or prior_record['targets'] != targets:
            raise ValueError('native build pin or targets differ')
        for name in ('host_configure', 'host_build'):
            result = prior_record[name]
            if result['exit_status'] or result['first_error'] is not None:
                raise ValueError(f'{name} did not pass')
            if prior != run:
                shutil.copyfile(prior / f'{name}.log', run / f'{name}.log')
            if digest(run / f'{name}.log') != result['log_sha256']:
                raise ValueError(f'{name} log digest differs')
            record[name] = result
        native = prior / 'native'
    else:
        stage('host_configure', configure_host, allowlist=flat)
        stage('host_build', (*BACKGROUND, 'cmake', '--build', str(native), '--parallel', '4',
                             '--target', 'llvm-tblgen', 'llvm-config'), timeout=1800, allowlist=allowed['host-toolchain archive'])
    runtime = Path(os.environ.get('BREENIX_CXX_RUNTIME_BUILDS', ROOT / 'build/cxx-runtime'))
    headers = (runtime / args.arch / 'build/include/c++/v1').resolve()
    if not (headers / '__config_site').is_file():
        raise ValueError(f'missing K3 configured C++ headers: {headers}')
    flags = shlex.join(('-nostdlibinc', '-isystem', str(ROOT / 'libs/libbreenix-libc/include'), POSIX_FLAG))
    cxxflags = shlex.join(('-nostdlibinc', '-nostdinc++', '-isystem', str(headers),
                            '-isystem', str(ROOT / 'libs/libbreenix-libc/include'), POSIX_FLAG))
    triple = f'{args.arch}-unknown-none'
    command = (*BACKGROUND, 'cmake', '-G', 'Ninja', '-S', str(llvm / 'llvm'), '-B', str(run / 'build'),
               '-DCMAKE_SYSTEM_NAME=Breenix', f'-DCMAKE_MODULE_PATH={ROOT / "toolchain/cmake"}',
               f'-DCMAKE_SYSTEM_PROCESSOR={args.arch}', '-DCMAKE_BUILD_TYPE=Release',
               '-DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY', '-DCMAKE_C_COMPILER=/usr/bin/clang',
               f'-DCMAKE_CXX_COMPILER={CLANGXX}', '-DCMAKE_ASM_COMPILER=/usr/bin/clang',
               f'-DCMAKE_AR={LLVM_BIN / "llvm-ar"}', f'-DCMAKE_RANLIB={LLVM_BIN / "llvm-ranlib"}',
               f'-DCMAKE_C_COMPILER_TARGET={triple}', f'-DCMAKE_CXX_COMPILER_TARGET={triple}',
               f'-DCMAKE_ASM_COMPILER_TARGET={triple}', f'-DCMAKE_C_FLAGS={flags}',
               f'-DCMAKE_CXX_FLAGS={cxxflags}', f'-DLLVM_TABLEGEN={native / "bin/llvm-tblgen"}',
               f'-DLLVM_NATIVE_TOOL_DIR={native / "bin"}', '-DLLVM_ENABLE_THREADS=ON',
               '-DBUILD_SHARED_LIBS=OFF', '-DLLVM_BUILD_LLVM_DYLIB=OFF',
               '-DLLVM_LINK_LLVM_DYLIB=OFF', *common)
    stage('configure', command, allowlist=allowed['upstream cmake'])
    verify_llvm(ROOT, llvm)


if __name__ == '__main__':
    main()
