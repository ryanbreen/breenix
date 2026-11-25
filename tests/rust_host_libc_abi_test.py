#!/usr/bin/env python3
"""Compare the X2 libc declarations with Breenix's headers for one hosted sysroot."""
import argparse
from pathlib import Path
import re
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
LIBC = ROOT / 'libs/libbreenix-libc/include'
NAMES = '''FIONREAD LOCK_SH LOCK_EX LOCK_NB LOCK_UN ST_RDONLY ST_NOSUID
POSIX_FADV_NORMAL POSIX_FADV_RANDOM POSIX_FADV_SEQUENTIAL POSIX_FADV_WILLNEED
POSIX_FADV_DONTNEED POSIX_FADV_NOREUSE GETENTROPY_MAX'''.split()
LAYOUTS = {
    'fsid_t': ['__val'],
    'statfs': '''f_type f_bsize f_blocks f_bfree f_bavail f_files f_ffree f_fsid
                 f_namelen f_frsize f_flags f_spare'''.split(),
    'iovec': ['iov_base', 'iov_len'],
    'timespec': ['tv_sec', 'tv_nsec'],
}
SIGNATURES = {
    'dup3': ('int', 'int, int, int'),
    'faccessat': ('int', 'int, const char *, int, int'),
    'flock': ('int', 'int, int'),
    'fstatfs': ('int', 'int, struct statfs *'),
    'statfs': ('int', 'const char *, struct statfs *'),
    'getentropy': ('int', 'void *, size_t'),
    'ioctl': ('int', 'int, unsigned long, ...'),
    'mknodat': ('int', 'int, const char *, mode_t, dev_t'),
    'msync': ('int', 'void *, size_t, int'),
    'posix_fadvise': ('int', 'int, off_t, off_t, int'),
    'posix_fallocate': ('int', 'int, off_t, off_t'),
    'preadv': ('ssize_t', 'int, const struct iovec *, int, off_t'),
    'pthread_atfork': ('int', 'void (*)(void), void (*)(void), void (*)(void)'),
    'pwritev': ('ssize_t', 'int, const struct iovec *, int, off_t'),
    'seekdir': ('void', 'DIR *, long'),
    'sync': ('void', 'void'),
    'utimensat': ('int', 'int, const char *, const struct timespec *, int'),
}
RUST_TYPES = {
    'int': 'c_int', 'unsigned long': 'c_ulong', 'long': 'c_long',
    'const char *': '*const libc::c_char', 'void *': '*mut libc::c_void',
    'struct statfs *': '*mut libc::statfs', 'DIR *': '*mut libc::DIR',
    'const struct iovec *': '*const libc::iovec',
    'const struct timespec *': '*const libc::timespec',
    'void (*)(void)': 'Option<unsafe extern "C" fn()>',
}


def rust_type(name):
    mapped = RUST_TYPES.get(name, name)
    return mapped if mapped.startswith(('*', 'Option<')) else f'libc::{mapped}'


def check(arch, deps, rustc, clang):
    clines = ['#define _GNU_SOURCE 1']
    for header in ('stddef.h', 'limits.h', 'fcntl.h', 'dirent.h', 'pthread.h', 'unistd.h', 'sys/file.h',
                   'sys/ioctl.h', 'sys/mman.h', 'sys/stat.h', 'sys/statfs.h', 'sys/statvfs.h', 'sys/uio.h',
                   'sys/sysmacros.h'):
        clines.append(f'#include <{header}>')
    rlines = ['#![no_std]', '#![feature(rustc_private)]', 'extern crate libc;']
    for name, (result, args) in SIGNATURES.items():
        clines.append(f'_Static_assert(__builtin_types_compatible_p(__typeof__(&{name}), '
                      f'{result} (*)({args})), "{name}");')
        rust_args = ', '.join(arg if arg == '...' else rust_type(arg)
                              for arg in args.split(', ') if arg != 'void')
        rust_result = '' if result == 'void' else f' -> {rust_type(result)}'
        rlines.append(f'const _: unsafe extern "C" fn({rust_args}){rust_result} = libc::{name};')

    cvalues = list(NAMES)
    rvalues = [f'libc::{name} as u64' for name in NAMES]
    for name, members in LAYOUTS.items():
        ctype = name if name == 'fsid_t' else f'struct {name}'
        rtype = f'libc::{name}'
        cvalues += [f'sizeof({ctype})', f'_Alignof({ctype})']
        rvalues += [f'core::mem::{operation}::<{rtype}>() as u64'
                    for operation in ('size_of', 'align_of')]
        cvalues += [f'offsetof({ctype}, {member})' for member in members]
        rvalues += [f'core::mem::offset_of!({rtype}, {member}) as u64' for member in members]
    for pair in ((0, 0), (0xfff, 0xfffff), (0x1042, 0x1abcd7)):
        for expr in ('makedev({}, {})', 'major(makedev({}, {}))', 'minor(makedev({}, {}))'):
            value = expr.format(*pair)
            cvalues.append(value)
            rvalues.append(re.sub(r'(major|minor|makedev)\(', r'libc::\1(', value) + ' as u64')
    clines.append('const unsigned long long ABI[] = {' + ', '.join(cvalues) + '};')
    rlines.append(f'#[no_mangle] pub static ABI: [u64; {len(rvalues)}] = [' +
                  ', '.join(rvalues) + '];')

    def compile_ir(command, file, lines):
        file.write_text('\n'.join(lines) + '\n')
        ir = subprocess.check_output([*command, str(file), '-o', '-'], text=True, timeout=15)
        return next(line for line in ir.splitlines() if line.startswith('@ABI = '))

    with tempfile.TemporaryDirectory(prefix='breenix-x2-libc-abi-') as temp:
        work = Path(temp)
        rust = compile_ir([rustc, '--target', ROOT / f'{arch}-breenix-hosted.json', '--edition=2021',
                           '--crate-type=lib', '--emit=llvm-ir', '-Dwarnings', '-L', f'all={deps}',
                           '--extern', 'libc'], work / 'abi.rs', rlines)
        c = compile_ir([clang, f'--target={arch}-unknown-none', '-std=c17', '-Werror', '-nostdinc',
                        '-isystem', LIBC, '-isystem', ROOT / 'toolchain/include', '-S', '-emit-llvm'],
                       work / 'abi.c', clines)
        expected = [int(item) for item in re.findall(r'i64 (\d+)', c)]
        assert len(expected) == len(cvalues), c
        # rustc lowers this named byte array to an LLVM c-string, clang to i64s.
        encoded = re.search(r'c"([^"]*)"', rust)[1]
        raw = bytes(int(token[1:], 16) if token.startswith('\\') else ord(token)
                    for token in re.findall(r'\\[0-9A-Fa-f]{2}|.', encoded))
        actual = [int.from_bytes(raw[i:i + 8], 'little') for i in range(0, len(raw), 8)]
        assert len(actual) == len(expected), rust
        for name, got, want in zip(cvalues, actual, expected):
            assert got == want, f'{arch}: {name}: Rust={got}, C={want}'
    print(f'X2 libc ABI ({arch}): PASS')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('arch', choices=('x86_64', 'aarch64'))
    parser.add_argument('deps', type=Path, help='hosted std release/deps with its libc rlib')
    parser.add_argument('--rustc', default='rustc')
    parser.add_argument('--clang', default='clang')
    args = parser.parse_args()
    check(args.arch, args.deps.resolve(), args.rustc, args.clang)
