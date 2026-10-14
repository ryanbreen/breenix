#!/usr/bin/env python3
"""Exercise BUSYBOX exec search without booting a guest.

Compile the actual functions with mocked syscall outcomes; no other libc modules
are built and no host libc source is used.
"""
from pathlib import Path
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / 'libs/libbreenix-libc/src/unistd.rs').read_text()
names = {'ENOEXEC', 'ENOMEM', 'EACCES', 'ENAMETOOLONG', 'ENOENT',
         'ENOTDIR', 'ENOSYS', 'SHELL', 'DEFAULT_PATH'}
constants = '\n'.join(line for line in source.splitlines()
                      if line.startswith('const ') and line.split()[1].rstrip(':') in names) + '\n'
functions = source[source.index('fn set_errno'):source.index('// ---------------------------------------------------------------------------------\n// Waiting')]
# Public exec wrappers are unnecessary in this focused harness and must not replace
# the host process's libc entry points.
functions = functions.replace('#[no_mangle]\n', '')
prctl = ''
harness = (ROOT / 'tests/libc-exec-host/cases.rs').read_text()
with tempfile.TemporaryDirectory(prefix='breenix-exec-host-') as tmp:
    path = Path(tmp) / 'tests.rs'
    path.write_text('#![deny(warnings)]\n#![feature(c_variadic)]\nuse core::ffi::VaList;\nuse core::ptr::null;\n'
                    + (constants + functions + prctl + harness).replace('environ', 'ENVIRON'))
    env = dict(os.environ, CARGO_BUILD_JOBS='6')
    subprocess.run(['rustc', '--edition=2021', '--test', str(path), '-o', str(Path(tmp) / 'tests')],
                   cwd=ROOT, env=env, check=True)
    subprocess.run([str(Path(tmp) / 'tests'), '--test-threads=1'], check=True)
