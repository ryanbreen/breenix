"""Execute bwm's input drain and count page-backed scratch allocations."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class InputBuffer(unittest.TestCase):
    def test_ready_batches_reuse_the_full_reader_buffer(self):
        source = (ROOT / 'userspace/programs/src/bwm/input.rs').read_text()
        method = item(source, '    pub(super) fn read(').replace('pub(super)', 'pub(crate)')
        fixture = r'''
#![allow(dead_code)] // The old production drain does not use the persistent field.
use std::alloc::{GlobalAlloc,Layout,System};
use std::sync::{OnceLock,atomic::{AtomicUsize,Ordering}};
static LARGE:AtomicUsize=AtomicUsize::new(0);
struct Allocator;
unsafe impl GlobalAlloc for Allocator {
 unsafe fn alloc(&self,l:Layout)->*mut u8{if l.size()>=BUFFER{LARGE.fetch_add(1,Ordering::Relaxed);}unsafe{System.alloc(l)}}
 unsafe fn alloc_zeroed(&self,l:Layout)->*mut u8{if l.size()>=BUFFER{LARGE.fetch_add(1,Ordering::Relaxed);}unsafe{System.alloc_zeroed(l)}}
 unsafe fn dealloc(&self,p:*mut u8,l:Layout){unsafe{System.dealloc(p,l)}}
}
#[global_allocator]static ALLOC:Allocator=Allocator;
mod libbreenix {
 pub mod errno {#[derive(Debug)]pub enum Errno{EAGAIN,EIO}}
 pub mod error {#[derive(Debug)]pub enum Error{Os(super::errno::Errno)}
 impl std::fmt::Display for Error{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"{self:?}")}}}
}
mod core_input {pub mod abi{pub const READER_EVENTS:usize=1024;}}
const BUFFER:usize=core_input::abi::READER_EVENTS*(32+256);
type Event=u64;
struct Record<'a>{event:Event,payload:&'a[u8]}
struct Reader;
impl Reader {
 fn read<'a>(&self,b:&'a mut[u8])->Result<std::iter::Once<Result<Record<'a>,libbreenix::error::Error>>,libbreenix::error::Error>{
  assert_eq!(b.len(),BUFFER);b[..3].copy_from_slice(b"abc");Ok(std::iter::once(Ok(Record{event:17,payload:&b[..3]})))
 }
}
static READER:OnceLock<Reader>=OnceLock::new();
struct Stream{read_buffer:Vec<u8>}
impl Stream { METHOD }
fn main(){
 assert!(READER.set(Reader).is_ok());let mut stream=Stream{read_buffer:vec![0;BUFFER]};
 LARGE.store(0,Ordering::Relaxed);
 for _ in 0..200{assert_eq!((&mut stream).read(),vec![(17,b"abc".to_vec())]);}
 assert_eq!(LARGE.load(Ordering::Relaxed),0,"steady input drains remap a full 288KiB scratch buffer");
}
'''.replace('METHOD', method)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'input.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'input.rs'), '-o', str(p / 'input')], check=True)
            subprocess.run([str(p / 'input')], check=True)


if __name__ == '__main__':
    unittest.main()
