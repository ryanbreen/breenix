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
            if module == 'locale' and source.name != 'mod.rs':
                continue
            text = source.read_text().replace('#[no_mangle]', '')
            text = text.replace('pub static mut stdin', '#[allow(non_upper_case_globals)]\npub static mut stdin').replace('pub static mut stdout', '#[allow(non_upper_case_globals)]\npub static mut stdout').replace('pub static mut stderr', '#[allow(non_upper_case_globals)]\npub static mut stderr')
            if module == 'stdio' and source.name == 'mod.rs' and '--format-only' in sys.argv:
                text = text.replace('mod scan;', '').replace('mod misc;', '')
            if module == 'locale':
                text = text.replace('mod multibyte;', '')
                text = text.split('#[cfg(test)]')[0]
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
