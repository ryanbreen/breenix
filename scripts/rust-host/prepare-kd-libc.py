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
    with root.open('a') as source:
        source.write('\n#[cfg(feature = "hosted-rustc")]\nmod entropy;\n'
                     f'\n{enabled}\nmod tls_module;\n\n{enabled}\nmod dl_abi;\n')
    pthread = crate / 'src/pthread.rs'
    replace_line(
        pthread, '\npub(crate) fn static_tls_data()',
        '\n#[cfg(not(all(feature = "hosted-rustc", target_arch = "x86_64")))]\n'
        'pub(crate) fn static_tls_data()',
    )
    for line, addition in (
        ('\nmod spin;\n', 'pub(crate) mod hosted_tls;'),
        ('\n    reads: *mut ReadBlock,\n', '    dtv: hosted_tls::Dtv,'),
        ('\n    reads: *mut ReadBlock,\n', "    dl_error: core::cell::Cell<Option<&'static core::ffi::CStr>>,"),
        ('\n            reads: null_mut(),\n', '            dl_error: core::cell::Cell::new(None),'),
        ('\n            reads: null_mut(),\n', '            dtv: hosted_tls::Dtv::new(),'),
        ('\n        unsafe { run_thread_exit(|| take_cxa_destructor(p), || destructors(p)) };\n',
         '        unsafe { (*p).dtv = hosted_tls::Dtv::new() };'),
    ):
        indent = addition[:len(addition) - len(addition.lstrip())]
        replace_line(pthread, line, f'{line}{indent}{enabled}\n{addition}\n')


def select_aarch64_hosted_tls(crate):
    # Keep the ordinary pthread source and its artifact locations untouched.
    root = crate / 'src/lib.rs'
    with root.open('a') as source:
        source.write('\nmod entropy;\nmod tls_module;\nmod tls_variant_i;\nmod dl_abi;\n')
    pthread = crate / 'src/pthread.rs'
    for original, replacement in (
        ('\nmod spin;\n',
         '\nmod spin;\n#[path = "pthread/hosted_tls_aarch64.rs"]\npub(crate) mod hosted_tls;\n'),
        ('\n    reads: *mut ReadBlock,\n',
         "\n    reads: *mut ReadBlock,\n    dl_error: core::cell::Cell<Option<&'static core::ffi::CStr>>,\n"),
        ('\n            reads: null_mut(),\n',
         '\n            reads: null_mut(),\n            dl_error: core::cell::Cell::new(None),\n'),
        ('unsafe fn control_len() -> Option<usize> {\n    let align = TLS_ALIGN.max(16);',
         'unsafe fn control_len() -> Option<usize> {\n'
         '    let (size, align) = hosted_tls::layout()\n'
         '        .map_or((TLS_SIZE, TLS_ALIGN.max(16)), |layout| (layout.end, layout.align));'),
        ('\n        .checked_add(TLS_SIZE)?\n        .checked_add(align * 2)?',
         '\n        .checked_add(size)?\n        .checked_add(align.checked_mul(2)?)?'),
        ('\n    let align = TLS_ALIGN.max(16);\n    let block',
         '\n    let layout = hosted_tls::layout();\n'
         '    let align = layout.map_or(TLS_ALIGN.max(16), |layout| layout.align);\n'
         '    let block'),
        ('(tp, rounded(tp + 16, align).unwrap())',
         '(tp, rounded(tp + 16, TLS_ALIGN.max(16)).unwrap())'),
        ('    if TLS_FILE != 0 {\n',
         '    if layout.is_some() {\n'
         '        hosted_tls::initialize(tp as *mut u8);\n'
         '    } else if TLS_FILE != 0 {\n'),
        ('\npub(crate) fn static_tls_data()', '\n#[cfg(not(feature = "hosted-rustc"))]\npub(crate) fn static_tls_data()'),
    ):
        replace_line(pthread, original, replacement)


def prepare(source, build, arch):
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
    replace_line(crate / 'Cargo.toml', '\nhosted-llvm = []\n',
                 '\nhosted-llvm = []\nhosted-rustc = []\n')
    replace_line(crate / 'src/lib.rs', '\nmod crt;\n',
                 '\n#[path = "crt_shared.rs"]\nmod crt;\n')
    replace_line(crate / 'src/lifecycle.rs', '\npub(crate) unsafe fn run_main<T>(',
                 '\n#[cfg(not(feature = "hosted-rustc"))]\npub(crate) unsafe fn run_main<T>(')
    if arch == 'aarch64':
        replace_line(crate / 'src/lib.rs', '\nmod link;\n',
                     '\n#[path = "link_hosted.rs"]\nmod link;\n')
        select_aarch64_hosted_tls(crate)
    else:
        replace_line(crate / 'src/lib.rs', '\nmod link;\n',
                     '\n#[cfg_attr(all(feature = "hosted-rustc", target_arch = "x86_64"), '
                     'path = "link_hosted.rs")]\nmod link;\n')
        select_hosted_tls(crate)
    print(crate / 'Cargo.toml')


if __name__ == '__main__':
    source, build = (Path(arg).resolve() for arg in sys.argv[1:3])
    prepare(source, build, sys.argv[3])
