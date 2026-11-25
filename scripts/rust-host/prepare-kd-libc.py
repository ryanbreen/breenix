#!/usr/bin/env python3
"""Prepare libc's shared startup in a build-local copy of our own crate."""

from pathlib import Path
import shutil
import sys


def replace_line(path, original, replacement):
    text = path.read_text()
    if text.count(original) != 1:
        raise ValueError(f'{path.name}: expected one hosted startup selection point')
    path.write_text(text.replace(original, replacement))


def select_hosted_tls(crate):
    # Select the DTV alongside the shared CRT. Editing pthread.rs in the
    # ordinary crate would also move call-site locations encoded in libc.a.
    enabled = '#[cfg(all(feature = "hosted-rustc", target_arch = "x86_64"))]'
    root = crate / 'src/lib.rs'
    line = '\nmod entropy;\n'
    replace_line(root, line, f'{line}\n{enabled}\nmod tls_module;\n')
    pthread = crate / 'src/pthread.rs'
    for line, addition in (
        ('\nmod spin;\n', 'mod hosted_tls;'),
        ('\n    reads: *mut ReadBlock,\n', '    dtv: hosted_tls::Dtv,'),
        ('\n            reads: null_mut(),\n', '            dtv: hosted_tls::Dtv::new(),'),
        ('\n        unsafe { run_thread_exit(|| take_cxa_destructor(p), || destructors(p)) };\n',
         '        unsafe { (*p).dtv = hosted_tls::Dtv::new() };'),
    ):
        indent = addition[:len(addition) - len(addition.lstrip())]
        replace_line(pthread, line, f'{line}{indent}{enabled}\n{addition}\n')


def prepare(source, build):
    crate = build / source.name
    crate.mkdir(parents=True, exist_ok=True)
    for name in ('Cargo.toml', 'Cargo.lock'):
        shutil.copy2(source / name, crate / name)
    shutil.copytree(source / 'src', crate / 'src', dirs_exist_ok=True)
    for name in ('libbreenix', 'input-abi', 'breengel-abi', 'gpu-abi'):
        link = build / name
        if link.is_symlink():
            link.unlink()
        link.symlink_to(source.parent / name, target_is_directory=True)

    # The default Rust crate keeps the ordinary CRT and its static lifecycle.
    replace_line(crate / 'src/lib.rs', '\nmod crt;\n',
                 '\n#[path = "crt_shared.rs"]\nmod crt;\n')
    replace_line(crate / 'src/lifecycle.rs', '\npub(crate) unsafe fn run_main<T>(',
                 '\n#[cfg(not(feature = "hosted-rustc"))]\npub(crate) unsafe fn run_main<T>(')
    select_hosted_tls(crate)
    print(crate / 'Cargo.toml')


if __name__ == '__main__':
    prepare(*[Path(arg).resolve() for arg in sys.argv[1:]])
