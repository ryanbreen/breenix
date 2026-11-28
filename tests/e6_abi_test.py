"""Check the pinned mixed Rust/C ABI using relocatable objects and CPU emulation."""

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / 'tests/e6_abi'


def run(*args, **kwargs):
    return subprocess.check_output(args, text=True, cwd=ROOT, **kwargs)


def check_target(spec, arch):
    target = json.loads(run('rustc', '-Zunstable-options', '--target', spec,
                            '--print', 'target-spec-json'))
    features = set(target['features'].split(','))
    if arch == 'x86_64':
        assert {'+sse', '+sse2'} <= features and 'rustc-abi' not in target, target
    else:
        assert '+v8a' in features and '+strict-align' not in features, target


def function(disassembly, name):
    return disassembly.split(f'<{name}>:', 1)[1].split('\n\n', 1)[0]


def execute(binary, arch):
    from unicorn import Uc, UC_ARCH_ARM64, UC_ARCH_X86, UC_MODE_ARM, UC_MODE_64
    from unicorn.arm64_const import UC_ARM64_REG_LR, UC_ARM64_REG_PC, UC_ARM64_REG_SP, UC_ARM64_REG_W0
    from unicorn.x86_const import UC_X86_REG_EAX, UC_X86_REG_RIP, UC_X86_REG_RSP

    is_arm = arch == 'aarch64'
    cpu = Uc(UC_ARCH_ARM64, UC_MODE_ARM) if is_arm else Uc(UC_ARCH_X86, UC_MODE_64)
    start, end, stack, stack_end = 0x100000, 0x10f000, 0x200000, 0x210000
    code = binary.read_bytes()
    assert len(code) < end - start
    cpu.mem_map(start, 0x10000)
    cpu.mem_write(start, code)
    cpu.mem_map(stack, stack_end - stack)
    if is_arm:
        cpu.reg_write(UC_ARM64_REG_LR, end)
        cpu.reg_write(UC_ARM64_REG_SP, stack_end)
    else:
        cpu.reg_write(UC_X86_REG_RSP, stack_end - 8)
        cpu.mem_write(stack_end - 8, end.to_bytes(8, 'little'))
    cpu.emu_start(start, end, count=10000)
    assert cpu.reg_read(UC_ARM64_REG_PC if is_arm else UC_X86_REG_RIP) == end, 'probe did not return'
    result = cpu.reg_read(UC_ARM64_REG_W0 if is_arm else UC_X86_REG_EAX)
    assert result == 0, f'mixed-language probe returned {result}'


def check_objects(root, spec, arch):
    host = next(line.removeprefix('host: ') for line in run('rustc', '-vV').splitlines()
                if line.startswith('host: '))
    tools = Path(run('rustc', '--print', 'sysroot').strip()) / 'lib/rustlib' / host / 'bin'
    with tempfile.TemporaryDirectory(prefix='e6-abi-', dir=root.parent) as directory:
        scratch = Path(directory)
        c, rust, binary = (scratch / name for name in ('c.o', 'rust.o', 'probe.bin'))
        run(ROOT / 'scripts/breenix-cc', '--arch', arch, '-O2', '-Wall', '-Wextra', '-Werror',
            '-ffunction-sections', '-c', FIXTURE / 'probe.c', '-o', c)
        run('rustc', '--edition=2021', '--crate-type=lib', '--emit=obj', '-Copt-level=2',
            '-Dwarnings', '--target', spec, '--sysroot', root / 'sysroot',
            FIXTURE / 'probe.rs', '-o', rust)
        asm = run(tools / 'llvm-objdump', '-d', rust)
        if arch == 'aarch64':
            assert re.search(r'\bldur\s+x0, \[x0, #(?:0x)?1\]', function(asm, 'rust_packed')), asm
            for name in ('rust_atomic', 'rust_device'):
                assert re.search(r'\bldr\s+x0, \[x0\]', function(asm, name)), asm
        else:
            assert '%xmm' in function(asm, 'rust_fp'), asm
            assert re.search(r'\bf(?:ld|ild|add)', function(run(tools / 'llvm-objdump', '-d', c),
                                                           'c_long_double'))
        layout = scratch / 'probe.ld'
        layout.write_text(
            'SECTIONS { . = 0x100000; .text : { *(.text.check) *(.text .text.*) } '
            '.rodata : { *(.rodata .rodata.*) } .data : { *(.data .data.*) } }\n')
        run(tools / 'gcc-ld/ld.lld', '-static', '-e', 'check', '--oformat=binary',
            '-T', layout, c, rust, '-o', binary)
        execute(binary, arch)


def check_manifest(root, arch):
    guard = ROOT / 'scripts/userspace_generation.py'
    target = arch + '-breenix'
    archive = next((root / 'sysroot/lib/rustlib' / target / 'lib').glob('libcore-*.rlib'))
    run(sys.executable, guard, 'verify-inputs', root, target, archive)
    legacy_target = ROOT / f'{target}.json'
    mixed = subprocess.run([sys.executable, guard, 'verify-inputs', root, legacy_target, archive],
                           capture_output=True, text=True)
    assert mixed.returncode and 'generation target' in mixed.stderr, mixed
    with tempfile.TemporaryDirectory(prefix='e6-legacy-', dir=root.parent) as directory:
        legacy_archive = Path(directory) / 'libcore.rlib'
        # Identical bytes alone cannot authorize an archive from another root.
        legacy_archive.write_bytes(archive.read_bytes())
        mixed = subprocess.run([sys.executable, guard, 'verify-inputs', root, target, legacy_archive],
                               capture_output=True, text=True)
        assert mixed.returncode and 'outside the E6 generation' in mixed.stderr, mixed
    incomplete = subprocess.run([sys.executable, guard, 'verify-release', root, 'install/sdk.tar'],
                                capture_output=True, text=True)
    assert incomplete.returncode and 'incomplete E6 generation' in incomplete.stderr, incomplete


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path, nargs='?')
    parser.add_argument('--legacy', action='store_true', help='fail-first target-spec control')
    args = parser.parse_args()
    if not args.legacy and args.root is None:
        parser.error('the staged sysroot root is required')
    failures = 0
    for arch in ('x86_64', 'aarch64'):
        for suffix in ('', '-hosted'):
            name = f'{arch}-breenix{suffix}'
            root = args.root.resolve() / arch if args.root else None
            try:
                spec = ROOT / f'{name}.json' if args.legacy else root / 'targets' / f'{name}.json'
                check_target(spec, arch)
                if not args.legacy:
                    check_objects(root, spec, arch)
                print(f'{name}: ABI check passed')
            except AssertionError as error:
                print(f'{name}: ABI check failed: {error}', file=sys.stderr)
                failures += 1
        if not args.legacy:
            check_manifest(root, arch)
    return bool(failures)


if __name__ == '__main__':
    sys.exit(main())
