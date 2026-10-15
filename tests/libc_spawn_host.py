#!/usr/bin/env python3
"""Test the real clean-room spawn implementation with controlled syscall results.

No host libc implementation is linked into the code under test. The host allocator
backs action storage; raw syscalls are replaced so child ordering, PATH errors and
parent mask restoration can be checked without starting a guest.
"""
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / 'libs/libbreenix-libc/src/spawn.rs').read_text()
source = source.replace('//!', '//').replace('#[no_mangle]\n', '')
source = source.replace('use libbreenix::', 'use crate::libbreenix::')
numbers = (ROOT / 'libs/libbreenix/src/syscall.rs').read_text()
harness = (ROOT / 'tests/libc-spawn-host/cases.rs').read_text()
for arch in ('aarch64', 'x86_64'):
    # Select source branches textually so both guest syscall ABIs run on either
    # host architecture, without pretending rustc targets a different host.
    selected = re.sub(r'#\[cfg\(target_arch = "(aarch64|x86_64)"\)\]',
                      lambda m: '#[cfg(' + ('all()' if m[1] == arch else 'any()') + ')]', source)
    block = numbers.split('#[cfg(target_arch = "' + arch + '")]\npub mod nr {', 1)[1].split('\n}', 1)[0]
    used = set(re.findall(r'nr::(\w+)', selected))
    used.discard('FORK' if arch == 'aarch64' else 'CLONE')
    constants = '\n'.join(line for line in block.splitlines()
                          if re.search(r'pub const (\w+)', line)
                          and re.search(r'pub const (\w+)', line).group(1) in used)
    with tempfile.TemporaryDirectory(prefix='breenix-spawn-host-') as tmp:
        path = Path(tmp) / 'tests.rs'
        path.write_text('const GUEST_ARCH: &str = "' + arch + '";\n'
                        + 'mod libbreenix { pub mod syscall { pub mod nr {\n' + constants
                        + '\n} } pub use crate::mock_raw as raw; pub mod process {\n'
                        + 'pub fn exit(code: i32) -> ! { std::panic::panic_any(code) } } }\n'
                        + selected + harness)
        subprocess.run(['rustc', '--edition=2021', '--test', '-Dwarnings', str(path),
                        '-o', str(Path(tmp) / 'tests')], cwd=ROOT, check=True)
        subprocess.run([str(Path(tmp) / 'tests'), '--test-threads=1'], check=True)
