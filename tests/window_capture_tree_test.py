"""Run the kernel's tree-only capture body against counting host stubs."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class CaptureTree(unittest.TestCase):
    def test_tree_capture_avoids_pixels(self):
        source = (ROOT / 'kernel/src/window/ops.rs').read_text()
        marker = 'pub fn capture_tree('
        self.assertIn(marker, source, 'CAPTURE_TREE operation is missing')
        start = source.index(marker)
        pos = source.index('{', start) + 1
        depth = 1
        end = pos
        while depth:
            depth += (source[end] == '{') - (source[end] == '}')
            end += 1
        body = source[start:end]
        harness = r'''
#![allow(dead_code)]
use std::sync::{Mutex,atomic::{AtomicUsize,Ordering}};
const EINVAL:i32=22;
static PIXELS:AtomicUsize=AtomicUsize::new(0);
static TREES:AtomicUsize=AtomicUsize::new(0);
static REGISTRY:Mutex<Registry>=Mutex::new(Registry{window:Window{id:7,tree_kib:1,front:Some(1),tree_pins:[0,0],tree:Mem,front_commit:42}});
struct Caller; struct Mem;
impl Clone for Mem {fn clone(&self)->Self{Self}}
struct Window {id:u64,tree_kib:u32,front:Option<usize>,tree_pins:[usize;2],tree:Mem,front_commit:u64}
struct Registry {window:Window}
impl Registry {fn window(&mut self,_:u64,_:&Caller,_:fn(bool,bool,&Caller)->bool)->Result<&mut Window,i32>{Ok(&mut self.window)}}
mod who {use super::Caller; pub fn owner_compositor_or_root(_:bool,_:bool,_:&Caller)->bool{true}}
struct TreePin {window:u64,slot:usize}
struct Capture {id:u64,dst:u64,dst_len:u64,tree_dst:u64,tree_len:u64,commit_id:u64,width:u32,height:u32}
fn pending_decision(_:u64,_:&Caller,_:fn(bool,bool,&Caller)->bool,_:bool)->Result<(),i32>{Ok(())}
mod mem {pub fn probe_writable(_:u64,_:u64)->Result<(),i32>{Ok(())}}
fn bytes_out(_: &Mem,offset:u64,len:u64,dst:u64)->Result<(),i32>{assert_eq!((offset,len,dst),(1024,1024,5)); TREES.fetch_add(1,Ordering::SeqCst); Ok(())}
fn pixels_out(){PIXELS.fetch_add(1,Ordering::SeqCst);}
'''.replace('REGISTRY.lock()', 'REGISTRY.lock().unwrap()') + body.replace('REGISTRY.lock()', 'REGISTRY.lock().unwrap()') + r'''
#[test] fn captures_front_tree_without_pixels() {
 let mut c=Capture{id:7,dst:0,dst_len:0,tree_dst:5,tree_len:1024,commit_id:0,width:0,height:0};
 capture_tree(&Caller,&mut c).unwrap();
 assert_eq!(PIXELS.load(Ordering::SeqCst),0);
 assert_eq!(TREES.load(Ordering::SeqCst),1);
 assert_eq!(c.commit_id,42);
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'test.rs').write_text(harness)
            subprocess.run((['taskpolicy', '-b'] if sys.platform == 'darwin' else []) +
                           ['rustc', '--edition=2021', '--test', str(path / 'test.rs'), '-o', str(path / 'test')], check=True)
            subprocess.run([str(path / 'test')], check=True)

if __name__ == '__main__':
    unittest.main()
