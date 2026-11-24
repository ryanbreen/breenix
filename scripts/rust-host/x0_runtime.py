"""Run the pinned X0 C++ runtime and AArch64 ABI probes on the Mac."""
import hashlib
import os
import re
import signal
import subprocess
import tempfile
import tomllib
from pathlib import Path

BACKGROUND = ('taskpolicy', '-b')
FLOAT_SOURCE = '''#![no_std]

#[no_mangle]
pub extern "C" fn x0_float(value: f64) -> f64 {
    value + value
}
'''
ARCHIVES = ('libc++.a', 'libc++abi.a', 'libunwind.a')
FIRST_ERROR = re.compile(r'^(?:CMake Error\b|.*\b(?:fatal error|error)(?:\[[A-Z0-9]+\])?:)')


def output(*command, cwd=None):
    return subprocess.check_output(command, cwd=cwd, text=True).strip()


def toolchain(root):
    return tomllib.loads((root / 'userspace/programs/rust-toolchain.toml').read_text())['toolchain']['channel']


def llvm_tool(root, name):
    rustc = Path(output('rustup', 'which', '--toolchain', toolchain(root), 'rustc'))
    host = output(rustc, '-vV').split('host: ', 1)[1].splitlines()[0]
    return rustc.parent.parent / 'lib' / 'rustlib' / host / 'bin' / name


def build(command, log, *, cwd=None, env=None):
    with log.open('wb') as transcript:
        with subprocess.Popen(command, cwd=cwd, env=env, stdout=transcript,
                              stderr=subprocess.STDOUT, start_new_session=True) as process:
            try:
                return process.wait(timeout=55)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
                return 124


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def measure_float(root):
    library = root / 'rust-fork/library'
    output(root / 'scripts/rust-fork.sh', 'verify', cwd=root)
    target = root / 'aarch64-breenix.json'
    with tempfile.TemporaryDirectory(prefix='breenix-x0-float-') as directory:
        scratch = Path(directory)
        (scratch / 'Cargo.toml').write_text(
            '[package]\nname = "x0-float"\nversion = "0.0.0"\nedition = "2021"\n'
            '[lib]\npath = "float.rs"\n'
        )
        (scratch / 'float.rs').write_text(FLOAT_SOURCE)
        env = dict(os.environ, CARGO_BUILD_JOBS='4', CARGO_ENCODED_RUSTFLAGS='',
                   __CARGO_TESTS_ONLY_SRC_ROOT=str(library), CARGO_TERM_COLOR='never')
        command = (*BACKGROUND, 'rustup', 'run', toolchain(root), 'cargo', 'rustc',
                   '--offline', '--release', '--lib', '-Z', 'build-std=core',
                   '--target', str(target), '--target-dir', 'target', '--', '--emit=obj')
        log = scratch / 'build.log'
        if build(command, log, cwd=scratch, env=env) != 0:
            raise RuntimeError(log.read_text())
        if re.search(r'^warning', log.read_text(), re.MULTILINE):
            raise RuntimeError(log.read_text())
        obj, = (scratch / 'target/aarch64-breenix/release/deps').glob('x0_float-*.o')
        disassemble = ('--disassemble-symbols=x0_float', '--no-show-raw-insn')
        dump = output(llvm_tool(root, 'llvm-objdump'), *disassemble, obj)
        if 'file format elf64-littleaarch64' not in dump or '<x0_float>:' not in dump:
            raise ValueError('expected the X0.8 symbol in an AArch64 ELF object')
        instructions = [
            ' '.join(match.split()) for match in re.findall(r'^\s*[0-9a-f]+:\s+(.*)$', dump, re.M)
        ]
        # AAPCS64 sections 6.1.2, 6.8.2 rule C.1, and 6.9.
        if any(re.fullmatch(r'fadd d0, d0, d0', line) for line in instructions):
            verdict = 'vector-registers'
        elif 'mov x1, x0' in instructions and not any(
            re.search(r'\b[bhsdqv]\d+\b', line) for line in instructions
        ):
            relocations = output(llvm_tool(root, 'llvm-objdump'), '--reloc', obj)
            if not re.search(r'\bR_AARCH64_(?:CALL|JUMP)26\s+__adddf3\b', relocations):
                raise ValueError(f'unclassified X0.8 instructions: {instructions}')
            verdict = 'integer-registers'
        else:
            raise ValueError(f'unclassified X0.8 instructions: {instructions}')
        return {
            'command': [arg.replace(str(root), '<repo>') for arg in command],
            'disassemble': ['llvm-objdump', *disassemble, '<object>'],
            'target_sha256': digest(target),
            'source': FLOAT_SOURCE,
            'instructions': instructions,
            'verdict': verdict,
        }


def symbols(nm, archive, kind):
    listing = output(nm, '--format=posix', '--extern-only', kind, archive)
    return sorted({line.split()[0] for line in listing.splitlines()
                   if line.strip() and not line.endswith(':')})


def inventory(root, directory, libc, builtins):
    nm = llvm_tool(root, 'llvm-nm')
    archives = {}
    for name in ARCHIVES:
        path, = directory.rglob(name)
        archives[name] = path
    undefined = {name: symbols(nm, path, '--undefined-only') for name, path in archives.items()}
    providers = {'libc.a': libc, 'compiler_builtins': builtins}
    definitions = {name: symbols(nm, path, '--defined-only') for name, path in providers.items()}
    return {
        'archive_sha256': {name: digest(path) for name, path in (archives | providers).items()},
        'undefined_by_archive': undefined,
        'defined_by_provider': definitions,
        'remaining_undefined': sorted(set().union(*undefined.values())
                                      - set().union(*definitions.values())),
    }


def measure_runtime(root, llvm, llvm_pin, scratch, libc, builtins):
    if (output('git', '-C', llvm, 'rev-parse', 'HEAD') != llvm_pin
            or output('git', '-C', llvm, 'status', '--porcelain')):
        raise ValueError('the LLVM workload must be clean and at the pinned gitlink')
    scratch.mkdir()
    build_dir = scratch / 'build'
    flags = f'-nostdlibinc -isystem {root / "toolchain/include"}'
    triple = 'x86_64-unknown-none'
    configure = (
        *BACKGROUND, 'cmake', '-G', 'Ninja', '-S', str(llvm / 'runtimes'), '-B', str(build_dir),
        '-DCMAKE_SYSTEM_NAME=Generic', '-DCMAKE_SYSTEM_PROCESSOR=x86_64',
        '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY',
        '-DCMAKE_C_COMPILER=/usr/bin/clang', '-DCMAKE_CXX_COMPILER=/usr/bin/clang++',
        '-DCMAKE_ASM_COMPILER=/usr/bin/clang', f'-DCMAKE_C_COMPILER_TARGET={triple}',
        f'-DCMAKE_CXX_COMPILER_TARGET={triple}', f'-DCMAKE_ASM_COMPILER_TARGET={triple}',
        f'-DCMAKE_C_FLAGS={flags}', f'-DCMAKE_CXX_FLAGS={flags}',
        '-DLLVM_ENABLE_RUNTIMES=libcxx;libcxxabi;libunwind', '-DLLVM_INCLUDE_TESTS=OFF',
        '-DLIBCXX_CXX_ABI=libcxxabi', '-DLIBCXX_ENABLE_LOCALIZATION=OFF',
        '-DLIBCXX_ENABLE_THREADS=ON', '-DTHREADS_PREFER_PTHREAD_FLAG=ON',
        '-DLIBCXX_HAS_PTHREAD_API=ON', '-DLIBCXXABI_HAS_PTHREAD_API=ON',
        '-DLIBCXXABI_USE_LLVM_UNWINDER=ON', '-DLIBUNWIND_ENABLE_THREADS=ON',
        '-DLIBCXX_ENABLE_SHARED=OFF', '-DLIBCXXABI_ENABLE_SHARED=OFF',
        '-DLIBUNWIND_ENABLE_SHARED=OFF',
    )

    def normalize(line):
        for path, label in ((llvm, '<llvm-project>'), (build_dir, '<build>'),
                            (root, '<repo>'), (Path.home(), '<home>')):
            line = line.replace(str(path), label)
        return line

    def stage(name, command):
        log = scratch / f'{name}.log'
        status = build(command, log)
        lines = log.read_text(errors='replace').splitlines()
        first_error = None
        if status != 0:
            for index, line in enumerate(lines):
                if FIRST_ERROR.search(line):
                    first_error = normalize(line).strip()
                    if line.startswith('CMake Error') and index + 1 < len(lines):
                        first_error += '\n' + normalize(lines[index + 1]).strip()
                    break
            if first_error is None:
                first_error = ('Build process exceeded 55 seconds.' if status == 124
                               else f'Process exited with status {status} without an error diagnostic.')
        return {
            'command': [normalize(arg) for arg in command],
            'exit_status': status,
            'first_error': first_error,
            'warnings': [normalize(line) for line in lines
                         if re.search(r'^(?:CMake Warning\b|.*\bwarning:)', line)],
            'log_sha256': digest(log),
        }

    configured = stage('configure', configure)
    built = None
    symbols_record = None
    pthread_cache = []
    if configured['exit_status'] == 0:
        pthread_cache = [
            line for line in (build_dir / 'CMakeCache.txt').read_text().splitlines()
            if line.startswith(('LIBCXXABI_HAS_PTHREAD_API:', 'LIBCXX_HAS_PTHREAD_API:',
                                'LIBUNWIND_ENABLE_THREADS:'))
        ]
        built = stage('build', (*BACKGROUND, 'cmake', '--build', str(build_dir),
                                '--parallel', '4', '--target', 'cxx', 'cxxabi', 'unwind'))
        if built['exit_status'] == 0:
            if libc is None or builtins is None:
                raise ValueError('pass the x86 C-ABI libc.a and pinned compiler_builtins rlib')
            symbols_record = inventory(root, build_dir, libc, builtins)
    if output('git', '-C', llvm, 'status', '--porcelain'):
        raise ValueError('runtime probe modified the pinned LLVM tree')
    return {
        'llvm_project': llvm_pin,
        'tools': {name: output(name, '--version').splitlines()[0]
                  for name in ('cmake', 'ninja', '/usr/bin/clang++')},
        'configure': configured,
        'pthread_cache': pthread_cache,
        'build': built,
        'inventory': symbols_record,
    }
