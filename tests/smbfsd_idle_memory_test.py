"""Run smbfsd's production request loop against libc's real small-block cache."""
from pathlib import Path
import tempfile
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class IdleMemory(unittest.TestCase):
    def test_idle_daemon_returns_freed_request_pages(self):
        source = (ROOT / 'userspace/programs/src/smbfsd.rs').read_text()
        loop = item(source, 'loop {')
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            for name in ('alloc_cache.rs', 'malloc.rs'):
                original = ROOT / 'libs/libbreenix-libc/src/stdlib' / name
                code = original.read_text().split('#[cfg(test)]', 1)[0]
                (directory / name).write_text(code.replace('#[no_mangle]', ''))
            self.check_idle(loop, directory)

    def check_idle(self, loop, directory):
        run_rust(self, r'''
use std::io::{self, Write};
include!("HOST");
use allocation_string as string;
#[allow(dead_code)] // Published C APIs outside the idle-path check.
mod stdlib {
    pub(crate) use crate::{errno, set_errno};
    pub const EINVAL: i32 = 22;
    pub const ENOMEM: i32 = 12;
    #[path="TMP/alloc_cache.rs"] pub mod alloc_cache;
    #[path="TMP/malloc.rs"] pub mod malloc;
}
use stdlib::malloc::{malloc, free};
#[no_mangle]
unsafe extern "C" fn __breenix_malloc_release_cached() -> usize {
    stdlib::malloc::__breenix_malloc_release_cached()
}

struct Request<'a> { id:u64, bytes:&'a [u8] }
impl<'a> Request<'a> {
    fn decode(bytes:&'a [u8]) -> Result<Self,()> { Ok(Self { id:1, bytes }) }
}
struct Reply<'a> { id:u64, errno:i32, payload:&'a [u8] }
impl Reply<'_> {
    fn encode(&self, output:&mut [u8]) -> Result<usize,()> {
        assert_eq!((self.id,self.errno),(1,0)); output[0]=self.payload[0]; Ok(1)
    }
}
struct Device { reads:usize }
impl Device {
    fn read(&mut self, input:&mut [u8]) -> io::Result<usize> {
        self.reads+=1;
        if self.reads==1 { input[0]=42; return Ok(1); }
        // This is the point where the real daemon blocks indefinitely in /dev/ufs.
        assert_eq!(MAPPINGS.lock().unwrap().len(),1,"idle smbfsd retained freed request mappings");
        assert_eq!(unsafe { __breenix_malloc_release_cached() },0);
        Ok(0)
    }
    fn write(&mut self, output:&[u8]) -> io::Result<usize> {
        assert_eq!(output,[42]); Ok(output.len())
    }
}
struct Fs;
impl Fs {
    fn serve(&mut self, req:&Request<'_>, _:u64) -> Result<Vec<u8>,i32> {
        // Transient protocol/name blocks follow the actual libc free path.
        for size in [16, 32, 64] {
            let blocks: Vec<_> = (0..16).map(|_| unsafe { malloc(size) }).collect();
            for p in blocks {
                assert!(!p.is_null());
                unsafe { free(p) };
            }
        }
        assert!(MAPPINGS.lock().unwrap().len() > 1,"request did not seed the cache");
        Ok(req.bytes.to_vec())
    }
    fn recovery_count(&self) -> u64 { 0 }
}
fn run() -> Result<(),String> {
    let start=std::time::Instant::now();
    let mut dev=Device { reads:0 }; let mut fs=Fs;
    let mut input=vec![0;64]; let mut output=vec![0;64];
    let mut reported_recoveries=0;
    LOOP
}
#[test] fn idle_returns_freed_pages() {
    // A live object must survive cache retirement.
    let live=unsafe { malloc(256) };
    assert!(!live.is_null());
    unsafe { live.write_bytes(0xa5,256) };
    run().unwrap();
    assert!(unsafe { std::slice::from_raw_parts(live,256) }.iter().all(|b| *b==0xa5));
    unsafe { free(live); __breenix_malloc_release_cached(); }
    assert!(MAPPINGS.lock().unwrap().is_empty());
}
'''.replace('HOST', str(ROOT / 'tests/fixtures/libc_malloc_host.rs'))
            .replace('TMP', str(directory)).replace('LOOP', loop))


if __name__ == '__main__':
    unittest.main()
