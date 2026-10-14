#!/usr/bin/env python3
"""Exercise this lane's wide I/O against Breenix FILE/format on host adapters.

Only export attributes are removed from production sources so the host's FILE
symbols cannot be interposed. Guest acceptance remains the unchanged C suite.
"""
from pathlib import Path
import subprocess
import tempfile
import re

root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='breenix-locale-io-') as directory:
    tmp = Path(directory)
    for module in ('stdio', 'locale'):
        dest = tmp / module
        dest.mkdir()
        for source in (root / 'libs/libbreenix-libc/src' / module).glob('*.rs'):
            text = source.read_text().replace('#[no_mangle]', '')
            text = text.replace('#[cfg(not(test))]', '')
            for name in ('stdin', 'stdout', 'stderr', 'timezone', 'daylight', 'tzname'):
                text = text.replace('pub static mut ' + name, '#[allow(non_upper_case_globals)]\npub static mut ' + name)
            if module == 'locale' and source.name == 'mod.rs':
                # Include only the I/O host cases, not the algorithm test set.
                text = text.split('#[cfg(test)]')[0]
                text += '\n#[cfg(test)] mod io_cases;\n'
            (dest / source.name).write_text(text)
    (tmp / 'locale/io_cases.rs').write_text((root / 'tests/locale-host/cases.rs').read_text())
    support = (root / 'tests/stdio-host/support.rs').read_text()
    support = re.sub(r'#\[cfg\(test\)\]\s*mod cases;', '', support)
    support = support.replace('mod pthread {', '''mod pthread {
 std::thread_local! {static LOCALE:std::cell::UnsafeCell<*mut core::ffi::c_void>=const{std::cell::UnsafeCell::new(core::ptr::null_mut())};}
 pub unsafe fn locale_slot()->*mut *mut core::ffi::c_void{LOCALE.with(|s|s.get())}
''')
    support += '''
extern "C" { #[link_name="getenv"] fn host_getenv(p:*const u8)->*mut u8; #[link_name="clock_gettime"] fn host_clock_gettime(c:i32,p:*mut u8)->i32; }
pub unsafe fn clock_gettime(c:i32,p:*mut u8)->i32 { host_clock_gettime(c,p) }
mod env {pub fn getenv_bytes(name:&[u8])->Option<&'static [u8]> {let mut key=name.to_vec();key.push(0);unsafe{let p=super::host_getenv(key.as_ptr());if p.is_null(){return None;}let mut n=0;while *p.add(n)!=0{n+=1;}Some(core::slice::from_raw_parts(p,n))}}}
'''
    (tmp / 'support.rs').write_text(support)
    subprocess.run(['rustc', '--edition=2021', '--test', '-O', str(tmp / 'support.rs'), '-o', str(tmp / 'tests')], cwd=root, check=True)
    subprocess.run([str(tmp / 'tests'), '--test-threads=1'], check=True)
