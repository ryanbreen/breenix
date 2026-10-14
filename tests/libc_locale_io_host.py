#!/usr/bin/env python3
"""Exercise this lane's wide I/O against Breenix FILE/format on host adapters.

Only export attributes are removed from production sources so the host's FILE
symbols cannot be interposed. The host services (errno, locks, syscalls and the
STRMATH number cores) are the STDIO owner's tests/stdio-host/support.rs; only the
locale I/O cases run here. Guest acceptance remains the unchanged C suite.
"""
from pathlib import Path
import re
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
src = root / 'libs/libbreenix-libc/src'
with tempfile.TemporaryDirectory(prefix='breenix-locale-io-') as directory:
    tmp = Path(directory)
    for module in ('stdio', 'locale'):
        dest = tmp / module
        dest.mkdir()
        for source in (src / module).glob('*.rs'):
            if module == 'locale' and source.name == 'clocks.rs':
                continue  # Kernel syscall wrappers; nothing to run on the host.
            # C names such as stdin and tzname stay lowercase once unexported.
            text = source.read_text().replace('#[no_mangle]', '#[allow(non_upper_case_globals)]')
            if module == 'stdio':
                text = text.replace('\nmod misc;', '\npub(crate) mod misc;')
                # STDIO's unit tests belong to STDIO's runner.
                text = text.split('#[cfg(test)]\nmod tests')[0]
            if module == 'locale':
                # The stream, format and scan parts are what these cases test.
                text = text.replace('#[cfg(not(test))]\nmod clocks;', '')
                text = text.replace('#[cfg(not(test))]', '')
                text = text.split('#[cfg(test)]\nmod tests')[0]
                if source.name == 'mod.rs':
                    text += '\n#[cfg(test)] mod io_cases;\n'
            (dest / source.name).write_text(text)
    (tmp / 'locale/io_cases.rs').write_text((root / 'tests/locale-host/cases.rs').read_text())
    dependencies = {'stdlib': ('floatconv.rs', 'numscan.rs'), 'math': ('fenv.rs',),
                    'string': ('errtext.rs',)}
    for module, names in dependencies.items():
        (tmp / module).mkdir()
        for name in names:
            text = (src / module / name).read_text().replace('#[no_mangle]', '')
            (tmp / module / name).write_text(text.split('#[cfg(test)]')[0])
    (tmp / 'stdlib/mod.rs').write_text(
        'pub(crate) mod floatconv; pub(crate) mod numscan; pub(crate) const EINVAL:i32=22;')
    # Compile the real STRMATH reporting service used by the shared scanner.
    math_source = (src / 'math/mod.rs').read_text()
    start = math_source.index('pub(crate) fn report(')
    end = math_source.index('\n}\n', start) + 3
    (tmp / 'math/mod.rs').write_text(
        'pub(crate) mod fenv;\n'
        'use crate::stdlib::floatconv::Converted;\n'
        'use crate::pthread::set_errno;\n'
        'const ERANGE: i32 = 34; // Breenix errno ABI, as in stdlib/mod.rs.\n'
        + math_source[start:end])
    (tmp / 'string/mod.rs').write_text('mod errtext; pub(crate) use errtext::error_text;')
    support = (root / 'tests/stdio-host/support.rs').read_text()
    # STDIO's own cases belong to STDIO's runner.
    support = re.sub(r'#\[cfg\(test\)\]\s*mod cases;', '', support)
    # Only STDIO's cases read the host environment.
    support = support.replace('    fn host_environ()', '    #[allow(dead_code)]\n    fn host_environ()')
    (tmp / 'support.rs').write_text(support)
    subprocess.run(['rustc', '--edition=2021', '--test', '-O',
                    str(tmp / 'support.rs'), '-o', str(tmp / 'tests')], cwd=root, check=True)
    subprocess.run([str(tmp / 'tests'), '--test-threads=1'], check=True)
