#!/usr/bin/env python3
"""Run only the clean-room stdio modules on the host, with host syscall adapters.

Remove export attributes so these tests never replace the host's FILE ABI.
The stream/format code is otherwise compiled verbatim from the production files.
"""
from pathlib import Path
import subprocess, tempfile, sys
root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='breenix-stdio-') as tmp:
    tmp = Path(tmp)
    for module in ('stdio', 'locale'):
        destination = tmp / module
        destination.mkdir()
        for source in (root / 'libs/libbreenix-libc/src' / module).glob('*.rs'):
            # C names such as stdin and tzname stay lowercase once unexported.
            text = source.read_text().replace('#[no_mangle]', '#[allow(non_upper_case_globals)]')
            if module == 'stdio' and source.name == 'mod.rs' and '--format-only' in sys.argv:
                text = text.replace('mod scan;', '').replace('mod misc;', '')
            elif module == 'stdio' and source.name == 'mod.rs':
                text = text.replace('\nmod misc;', '\npub(crate) mod misc;')
            # The whole locale module is compiled as a service. The test build sets
            # cfg(test), which leaves out its stream and formatting parts; its own
            # unit tests are stripped because this runner does not run them.
            if module == 'locale':
                text = text.split('#[cfg(test)]\nmod tests')[0]
            (destination / source.name).write_text(text)
    # Shared dependencies are compiled as services for stdio's tests only.
    # Strip their unit tests; this lane does not run another owner's host tests.
    dependencies={'stdlib':('floatconv.rs','numscan.rs'),'math':('fenv.rs',),'string':('errtext.rs',)}
    for module,names in dependencies.items():
        (tmp/module).mkdir()
        for name in names:
            source=root/'libs/libbreenix-libc/src'/module/name
            text=source.read_text().replace('#[no_mangle]','').split('#[cfg(test)]')[0]
            (tmp/module/name).write_text(text)
    (tmp/'stdlib/mod.rs').write_text('pub(crate) mod floatconv; pub(crate) mod numscan; pub(crate) const EINVAL:i32=22;')
    (tmp/'math/mod.rs').write_text('pub(crate) mod fenv;')
    (tmp/'string/mod.rs').write_text('mod errtext; pub(crate) use errtext::error_text;')
    for name in ('support.rs', 'cases.rs'):
        (tmp / name).write_text((root / 'tests/stdio-host' / name).read_text())
    subprocess.run(['rustc','--edition=2021','--test','-O',str(tmp/'support.rs'),'-o',str(tmp/'tests')],cwd=root,check=True)
    subprocess.run([str(tmp/'tests'),'--test-threads=1'],check=True)
