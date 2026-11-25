#!/usr/bin/env python3
"""Cross-link and inspect the x86-64 hosted dynamic suite fixtures."""

from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
SOURCES = ROOT / 'toolchain/rust-host/kd'
INTERPRETER = '/lib/ld-breenix.so'
MISSING = '/lib/ld-breenix-kd-missing.so'


def inspect(readobj, path, libraries, interp):
    report = subprocess.check_output([
        readobj, '--elf-output-style=GNU', '--file-header', '--program-headers',
        '--relocations', '--dynamic-table', path,
    ], text=True)
    needed = re.findall(r'\(NEEDED\).*?\[(.*?)\]', report)
    interpreters = re.findall(r'\[Requesting program interpreter: (.*?)\]', report)
    require(needed == libraries, path, f'unexpected DT_NEEDED entries {needed}')
    require(interpreters == ([interp] if interp else []), path,
            f'unexpected PT_INTERP entries {interpreters}')
    require('Advanced Micro Devices X86-64' in report, path, 'expected AMD64 ELF')
    if interp:
        require(re.search(r'^ +Type:\s+EXEC\b', report, re.MULTILINE),
                path, 'expected ET_EXEC')
        require(re.search(r'\(RUNPATH\).*?\[\$ORIGIN\]', report),
                path, 'expected DT_RUNPATH $ORIGIN')
    else:
        require(re.search(r'^ +Type:\s+DYN\b', report, re.MULTILINE),
                path, 'expected ET_DYN')
    return report


def require(condition, artifact, problem):
    if not condition:
        raise ValueError(f'{artifact.name}: {problem}')


def build(output, rust_tools, cc):
    lld = rust_tools / 'gcc-ld/ld.lld'
    readobj = rust_tools / 'llvm-readobj'
    libc = output / 'libc.so'
    soname = 'libkd-probe.so'
    with tempfile.TemporaryDirectory(prefix='kd-fixtures-', dir=output) as temporary:
        work = Path(temporary)
        dso = work / soname
        compile_flags = [
            '--no-default-config', '--target=x86_64-unknown-none-elf', '-D__breenix__=1',
            '-D_POSIX_C_SOURCE=202405L', '-nostdinc', '-isystem', ROOT / 'toolchain/include',
            '-isystem', ROOT / 'libs/libbreenix-libc/include', '-std=c17', '-O2',
            '-Wall', '-Wextra', '-Werror', '-fPIC', '-fplt', '-fno-builtin',
            '-fno-stack-protector', '-mno-red-zone',
        ]
        for source in ('dynamic-dso', 'dynamic-probe'):
            subprocess.run([cc, *compile_flags, '-ftls-model=local-dynamic', '-c',
                            SOURCES / f'{source}.c', '-o', work / f'{source}.o'], check=True)
        common = [lld, '--no-undefined', '--fatal-warnings', '--hash-style=sysv',
                  '-z', 'now', '-z', 'relro']
        subprocess.run([*common, '-shared', '-soname', soname, '-o', dso,
                        work / 'dynamic-dso.o', libc], check=True)
        dso_report = inspect(readobj, dso, ['libc.so'], None)
        require(re.search(r'\bTLS\s+.*?\s0x000000\s+0x000030\s+R\s+0x8\b', dso_report),
                dso, 'expected PT_TLS with 48 zero-fill bytes and alignment 8')
        require(len(re.findall(r'\bR_X86_64_DTPMOD64\b', dso_report)) == 1,
                dso, 'expected one local-dynamic TLS module slot')
        require(re.search(r'\bR_X86_64_64\s+\S+\s+kd_export\s+\+\s+4\b', dso_report),
                dso, 'expected a symbol-plus-addend relocation to kd_export')
        require(len(re.findall(r'\bR_X86_64_RELATIVE\b', dso_report)) == 1,
                dso, 'expected one base-relative relocation for kd_relative')

        for name, interp in (('dynamic-probe', INTERPRETER), ('missing-interp', MISSING)):
            probe = work / name
            subprocess.run([
                *common, '--image-base=0x40000000', '--entry=_start',
                '--dynamic-linker', interp, '-rpath', '$ORIGIN', '-o', probe,
                output / 'breenix-crt.o', work / 'dynamic-probe.o', dso, libc,
            ], check=True)
            report = inspect(readobj, probe, [soname, 'libc.so'], interp)
            require(re.search(r'\bR_X86_64_GLOB_DAT\s+\S+\s+kd_export\b', report),
                    probe, 'kd_export must be resolved through the GOT')
            require(re.search(r'\bR_X86_64_JUMP_SLOT\s+\S+\s+kd_relative_value\b', report),
                    probe, 'kd_relative_value must be resolved through the PLT')
            observed = set(re.findall(r'\bR_X86_64_\w+', report + dso_report))
            expected = {f'R_X86_64_{name}' for name in
                        ('64', 'DTPMOD64', 'GLOB_DAT', 'JUMP_SLOT', 'RELATIVE')}
            require(observed == expected, probe, f'expected the X2 relocation kinds, saw {sorted(observed)}')

        for name in (soname, 'dynamic-probe', 'missing-interp'):
            (work / name).replace(output / name)


if __name__ == '__main__':
    build(Path(sys.argv[1]).resolve(), Path(sys.argv[2]), Path(sys.argv[3]))
