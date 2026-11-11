"""Run smbfsd's production request loop against libc's real small-block cache."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class IdleMemory(unittest.TestCase):
    def test_idle_daemon_returns_freed_request_pages(self):
        source = (ROOT / 'userspace/programs/src/smbfsd.rs').read_text()
        loop = item(source, 'loop {')
        run_rust(self, r'''
use std::io::{self, Write};
use std::ops::Deref;
mod alloc_cache { CACHE }
fn getpid() -> i32 { 1 }
#[no_mangle]
unsafe extern "C" fn __breenix_malloc_release_cached() -> usize {
    alloc_cache::drain(|_, ptr| { drop(Box::from_raw(ptr.cast::<[usize; 256]>())); })
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
        let retained=unsafe { __breenix_malloc_release_cached() };
        assert_eq!(retained,0,"idle smbfsd retained freed request allocations");
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
        for class in 0..3 {
            assert_eq!(alloc_cache::marked_class(alloc_cache::marker(class)),Some(class));
            assert!(alloc_cache::capacity(class)>=16);
            assert_eq!(alloc_cache::class(alloc_cache::capacity(class)),Some(class));
            for _ in 0..16 {
                let p=Box::into_raw(Box::new([0usize;256])).cast();
                assert!(unsafe { alloc_cache::put(class,p) });
            }
        }
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
    let live=Box::new([0xa5u8;256]);
    run().unwrap();
    assert!(live.deref().iter().all(|b| *b==0xa5));
}
'''.replace('CACHE', (ROOT / 'libs/libbreenix-libc/src/stdlib/alloc_cache.rs').read_text().split('#[cfg(test)]')[0].replace('//!', '//'))
            .replace('LOOP', loop))


if __name__ == '__main__':
    unittest.main()
