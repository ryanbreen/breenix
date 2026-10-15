#!/usr/bin/env python3
"""Test the real clean-room spawn implementation with controlled syscall results.

No host libc implementation is linked into the code under test. The host allocator
backs action storage; raw syscalls are replaced so child ordering, PATH errors and
parent mask restoration can be checked without starting a guest.
"""
from pathlib import Path
import os
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
if 'SPAWN_TEST_ARCH' not in os.environ:
    for target in ('aarch64', 'x86_64'):
        subprocess.run(['python3', str(Path(__file__).resolve())], env=dict(os.environ, SPAWN_TEST_ARCH=target), check=True)
    raise SystemExit(0)
source = (ROOT / 'libs/libbreenix-libc/src/spawn.rs').read_text()
source = source.replace('//!', '//').replace('#[no_mangle]\n', '')
source = source.replace('use libbreenix::', 'use crate::libbreenix::')
for target in ('aarch64', 'x86_64'):
    source = source.replace('#[cfg(target_arch = \"' + target + '\")] ', '#[cfg(spawn_' + target + ')] ')
    source = source.replace('#[cfg(target_arch = \"' + target + '\")]\n', '#[cfg(spawn_' + target + ')]\n')
arch = os.environ['SPAWN_TEST_ARCH']
numbers = (ROOT / 'libs/libbreenix/src/syscall.rs').read_text()
block = numbers.split('#[cfg(target_arch = "' + arch + '")]\npub mod nr {', 1)[1].split('\n}', 1)[0]
used = set(re.findall(r'nr::(\w+)', source)) | {'EXIT_GROUP'}
used.discard('FORK' if arch == 'aarch64' else 'CLONE')
constants = '\n'.join(line for line in block.splitlines()
                      if re.search(r'pub const (\w+)', line)
                      and re.search(r'pub const (\w+)', line).group(1) in used)
harness = (ROOT / 'tests/libc-spawn-host/cases.rs').read_text()
with tempfile.TemporaryDirectory(prefix='breenix-spawn-host-') as tmp:
    path = Path(tmp) / 'tests.rs'
    path.write_text('mod libbreenix { pub mod syscall { pub mod nr {\n' + constants
                    + '\n} } pub use crate::mock_raw as raw; }\n'
                    + 'unsafe fn _exit(code: i32) -> ! { assert_eq!(nr::EXIT_GROUP, if cfg!(spawn_aarch64) {94} else {231}); std::panic::panic_any(code) }\n'
                    + source + harness)
    env = dict(os.environ, CARGO_BUILD_JOBS='4')
    subprocess.run(['rustc', '--edition=2021', '--test', '-Dwarnings', '--cfg', 'spawn_' + arch, str(path),
                    '-o', str(Path(tmp) / 'tests')], cwd=ROOT, env=env, check=True)
    subprocess.run([str(Path(tmp) / 'tests'), '--test-threads=1'], check=True)
