#!/usr/bin/env python3
"""Link X2's two hosted outputs with ELF lld and a shared Breenix libc."""

import os
from pathlib import Path
import re
import subprocess
import sys


def main(args):
    fallback = os.environ['BREENIX_X2_FALLBACK_LINKER']
    if args[:2] == ['-flavor', 'gnu']:
        args = args[2:]
    link = []
    for arg in args:
        link.extend(arg[4:].split(',') if arg.startswith('-Wl,') else [arg])
    if '-o' not in link:
        os.execv(fallback, [fallback, *args])
    output = Path(link[link.index('-o') + 1])
    shared = any(arg in ('-shared', '--shared') for arg in link)
    driver = shared and re.fullmatch(r'librustc_driver-[0-9a-f]+\.so', output.name)
    executable = not shared and any(
        re.fullmatch(r'librustc_driver-[0-9a-f]+\.so', Path(arg).name) for arg in link
    )
    if not (driver or executable):
        os.execv(fallback, [fallback, *args])

    inputs = Path(os.environ['BREENIX_KD_BUILD_DIR'])
    runtime = Path(os.environ['BREENIX_X2_RUNTIME'])
    lld = os.environ['BREENIX_X2_LLD']
    maps = Path(os.environ['BREENIX_X2_STAGE']) / 'maps'
    maps.mkdir(parents=True, exist_ok=True)
    link = [arg for arg in link
            if arg not in ('-lc', '-lc++', '-lbunwind') and Path(arg).name != 'libc.a']
    link.extend(('--no-dependent-libraries', '--hash-style=sysv', '-z', 'now',
                 '-z', 'relro', '-rpath', '$ORIGIN/../lib:/lib'))

    if driver:
        # Ruling 105's custom getrandom provider needs the exact rlib rustc chose.
        provider = next(Path(arg) for arg in link if Path(arg).name.startswith('libgetrandom-')
                        and Path(arg).with_name(Path(arg).stem[3:] + '.d').is_file()
                        and '/getrandom-0.3.3/src/' in Path(arg).with_name(
                            Path(arg).stem[3:] + '.d').read_text())
        rustc = os.environ['X2_RUSTC']
        target = Path(os.environ['RUST_TARGET_PATH'])
        subprocess.run([
            rustc, '--edition=2021', '--crate-type=lib', '--emit=obj', '-Dwarnings',
            '-Copt-level=2', '-Crelocation-model=pic', '--target', target / 'x86_64-breenix-hosted.json',
            '-L', f"all={os.environ['X2_SYSROOT_DEPS']}", '-L', f'dependency={provider.parent}',
            '--extern', f'getrandom={provider}',
            Path(__file__).resolve().parents[2] / 'toolchain/rust-host/k5/getrandom.rs',
            '-o', inputs / 'getrandom.o',
        ], check=True, env={key: value for key, value in os.environ.items() if key != 'CARGO_MAKEFLAGS'})
        link.extend((inputs / 'hosted_dso_handle.o', inputs / 'getrandom.o',
                     runtime / 'dynamic-libs/libblake3_sse2_sse41_avx2_assembly.a',
                     runtime / 'cxx-pic/lib/libc++.a', runtime / 'cxx-pic/lib/libc++abi.a',
                     runtime / 'dynamic-libs/libbunwind.a'))
        map_name = output.name
    else:
        link.extend((inputs / 'breenix-crt.o', '--allow-shlib-undefined',
                     '--image-base=0x40000000',
                     '--dynamic-linker=/lib/ld-breenix.so'))
        map_name = 'rustc'
    link.extend(('--push-state', '--no-as-needed', inputs / 'libc.so', '--pop-state',
                 f'-Map={maps / (map_name + ".map")}'))
    os.execv(lld, [lld, *map(str, link)])


if __name__ == '__main__':
    main(sys.argv[1:])
