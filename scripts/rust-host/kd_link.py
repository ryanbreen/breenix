#!/usr/bin/env python3
"""Link the hosted Rust tools with ELF lld and a shared Breenix libc."""

import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile


def prepare_bunwind(inputs, lld, *, archive=None, rlib=None):
    root = Path(__file__).resolve().parents[2]
    source = rlib or archive or (root / 'userspace/programs/target/hosted/bunwind'
                                 / 'x86_64-breenix-hosted/release/libbunwind.a')
    tools = Path(lld).parent.parent
    ar, nm, objcopy = (tools / name for name in ('llvm-ar', 'llvm-nm', 'llvm-objcopy'))
    members = subprocess.check_output([ar, 't', source], text=True).splitlines()
    bunwind, = (name for name in members if name.startswith('bunwind-'))
    objects = [bunwind]
    if rlib is None:
        core, = (name for name in members if name.startswith('core-'))
        objects.append(core)
    # Use the hosted compiler's builtins and panic runtime. Keep bunwind's
    # own stand-alone panic handler private to its object.
    with tempfile.TemporaryDirectory(dir=inputs) as scratch:
        subprocess.run([ar, 'x', source, *objects], cwd=scratch, check=True)
        symbols = subprocess.check_output(
            [nm, '--defined-only', '--format=posix', bunwind], cwd=scratch, text=True,
        )
        panic, = (line.split()[0] for line in symbols.splitlines()
                  if line.split()[0].endswith('rust_begin_unwind'))
        subprocess.run([objcopy, '--localize-symbol', panic, bunwind], cwd=scratch, check=True)
        subprocess.run([ar, 'crs', 'libbunwind.a', *objects], cwd=scratch, check=True)
        output = inputs / 'libbunwind.a'
        (Path(scratch) / output.name).replace(output)
    return output


def prepare_getrandom(link, inputs, *, arch='x86_64'):
    # Ruling 105's custom getrandom provider needs the exact rlib rustc chose.
    provider = next(Path(arg) for arg in link if Path(arg).name.startswith('libgetrandom-')
                    and Path(arg).with_name(Path(arg).stem[3:] + '.d').is_file()
                    and '/getrandom-0.3.3/src/' in Path(arg).with_name(
                        Path(arg).stem[3:] + '.d').read_text())
    rustc = os.environ['X2_RUSTC']
    target = Path(os.environ['RUST_TARGET_PATH'])
    output = inputs / 'getrandom.o'
    sysroot = ['--sysroot', os.environ['RUSTC_SYSROOT']] if arch == 'aarch64' else []
    subprocess.run([
        rustc, '--edition=2021', '--crate-type=lib', '--emit=obj', '-Dwarnings',
        '-Copt-level=2', '-Crelocation-model=pic', '--target', target / f'{arch}-breenix-hosted.json',
        *sysroot, '-L', f"all={os.environ['X2_SYSROOT_DEPS']}", '-L', f'dependency={provider.parent}',
        '--extern', f'getrandom={provider}',
        Path(__file__).resolve().parents[2] / 'toolchain/rust-host/k5/getrandom.rs',
        '-o', output,
    ], check=True, env={key: value for key, value in os.environ.items() if key != 'CARGO_MAKEFLAGS'})
    return output


def without_runpath(link):
    flags = iter(link)
    for flag in flags:
        if flag in ('-rpath', '--rpath'):
            next(flags)
        elif not flag.startswith(('-rpath=', '--rpath=')):
            yield flag


def require_bunwind_only(link, lld, label):
    ar = Path(lld).parent.parent / 'llvm-ar'
    for arg in link:
        name = Path(arg).name
        if re.fullmatch(r'libunwind-[0-9a-f]+\.rlib', name):
            # Rust's unwind crate carries the bunwind selection metadata.
            members = subprocess.check_output([ar, 't', arg], text=True).splitlines()
            if any(member != 'lib.rmeta' and
                   not re.fullmatch(r'unwind-.*\.rcgu\.o', member) for member in members):
                sys.exit(f'{label} unwind rlib contains a native library member')
        elif arg in ('-lunwind', '--library=unwind') or re.match(
                r'(?:(?:-l|--library=):)?libunwind(?:[.-]|$)', name):
            sys.exit(f'{label} requires bunwind')
    if any(option in ('-l', '--library') and name == 'unwind'
           for option, name in zip(link, link[1:])):
        sys.exit(f'{label} requires bunwind')


def main(args):
    fallback = os.environ['BREENIX_X2_FALLBACK_LINKER']
    arch = os.environ.get('BREENIX_X2_ARCH', 'x86_64')
    if args[:2] == ['-flavor', 'gnu']:
        args = args[2:]
    link = []
    for arg in args:
        link.extend(arg[4:].split(',') if arg.startswith('-Wl,') else [arg])
    fallback_args = link if arch == 'aarch64' else args
    if '-o' not in link:
        os.execv(fallback, [fallback, *fallback_args])
    output = Path(link[link.index('-o') + 1])
    shared = any(arg in ('-shared', '--shared') for arg in link)
    driver = shared and re.fullmatch(r'librustc_driver-[0-9a-f]+\.so', output.name)
    executable = not shared and any(
        re.fullmatch(r'librustc_driver-[0-9a-f]+\.so', Path(arg).name) for arg in link
    )
    cargo = not shared and re.fullmatch(r'cargo(?:-[0-9a-f]+)?', output.name)
    if not (driver or executable or cargo):
        os.execv(fallback, [fallback, *fallback_args])

    inputs = Path(os.environ['BREENIX_KD_BUILD_DIR'])
    lld = os.environ['BREENIX_X2_LLD']
    maps = Path(os.environ['BREENIX_X2_STAGE']) / 'maps'
    maps.mkdir(parents=True, exist_ok=True)
    link = [arg for arg in link
            if arg not in ('-lc', '-lc++', '-lbunwind') and Path(arg).name != 'libc.a']
    runpath = '$ORIGIN/../lib:/lib'
    if arch == 'aarch64':
        if not cargo:
            require_bunwind_only(link, lld, 'ARM hosted rustc')
        link = list(without_runpath(link))
        runpath = '$ORIGIN/../lib'
        link.extend(('--fatal-warnings', '-z', 'max-page-size=4096'))
    link.extend(('--no-dependent-libraries', '--hash-style=sysv', '-z', 'now',
                 '-z', 'relro', '-rpath', runpath))

    if driver:
        runtime = Path(os.environ['BREENIX_X2_RUNTIME'])
        link.extend(('-T', Path(__file__).with_name('kd-bases.ld'),
                     inputs / 'hosted_dso_handle.o', prepare_getrandom(link, inputs, arch=arch)))
        if arch == 'x86_64':
            link.append(runtime / 'dynamic-libs/libblake3_sse2_sse41_avx2_assembly.a')
        bunwind = Path(os.environ['BREENIX_X2_BUNWIND']) if arch == 'aarch64' else None
        link.extend((runtime / 'cxx-pic/lib/libc++.a', runtime / 'cxx-pic/lib/libc++abi.a',
                     prepare_bunwind(inputs, lld, archive=bunwind)))
        map_name = output.name
    elif cargo:
        require_bunwind_only(link, lld, 'hosted Cargo')
        cargo_runtime = inputs / 'cargo'
        cargo_runtime.mkdir(exist_ok=True)
        link.extend(('-T', Path(__file__).with_name('kd-bases.ld'),
                     inputs / 'breenix-crt.o', '--no-pie', '--fatal-warnings',
                     '--no-allow-shlib-undefined',
                     '--image-base=0x40000000', '--dynamic-linker=/lib/ld-breenix.so',
                     prepare_getrandom(link, cargo_runtime, arch=arch),
                     prepare_bunwind(cargo_runtime, lld,
                                     rlib=Path(os.environ['BREENIX_K7_BUNWIND']))))
        map_name = 'cargo'
    else:
        if arch == 'aarch64':
            link.append('--no-pie')
        link.extend((inputs / 'breenix-crt.o', '--allow-shlib-undefined',
                     '--image-base=0x40000000',
                     '--dynamic-linker=/lib/ld-breenix.so'))
        map_name = 'rustc'
    link.extend(('--push-state', '--no-as-needed', inputs / 'libc.so', '--pop-state',
                 f'-Map={maps / (map_name + ".map")}'))
    os.execv(lld, [lld, *map(str, link)])


if __name__ == '__main__':
    main(sys.argv[1:])
