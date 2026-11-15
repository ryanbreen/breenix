"""Count production canvas hashing and retained-frame copies without wall clocks."""
import os
import subprocess
import unittest
from guskit_new_save_test import ROOT, run_host
from gt710_gr_live_unwind_test import item

class DigestWork(unittest.TestCase):
    def test_metadata_reuses_hashes_and_canvas_snapshots(self):
        revision=os.environ.get('GUSKIT_DIGEST_BASELINE')
        source=(subprocess.check_output(['git','show',f'{revision}:userspace/programs/src/guskit/canvas.rs'],cwd=ROOT,text=True) if revision else (ROOT/'userspace/programs/src/guskit/canvas.rs').read_text()).replace('//!','//')
        dirty=item((ROOT/'userspace/programs/src/guskit.rs').read_text(),'    fn dirty(')
        run_host(self,r'''
#[cfg(test)] mod check {
use std::sync::atomic::{AtomicUsize,Ordering};
static COPIED_BYTES:AtomicUsize=AtomicUsize::new(0);
static HASH_BYTES:AtomicUsize=AtomicUsize::new(0);
struct Allocator;
unsafe impl std::alloc::GlobalAlloc for Allocator {
 unsafe fn alloc(&self,l:std::alloc::Layout)->*mut u8 {if l.size()==1148*454*3 {COPIED_BYTES.fetch_add(l.size(),Ordering::Relaxed);} std::alloc::System.alloc(l)}
 unsafe fn dealloc(&self,p:*mut u8,l:std::alloc::Layout) {std::alloc::System.dealloc(p,l)}
}
#[global_allocator] static ALLOCATOR:Allocator=Allocator;
mod counted_hash {pub fn sha256(b:&[u8])->[u8;32] {super::HASH_BYTES.fetch_add(b.len(),super::Ordering::Relaxed);libpixcheck::sha256(b)}}
#[allow(dead_code)] mod canvas {use super::counted_hash as libpixcheck;CANVAS}
use canvas::{Canvas,History,Radius};
struct App {history:History,clean:[u8;32]}
impl App {fn canvas(&self)->&Canvas {self.history.canvas()} DIRTY}
fn metadata(app:&App) {assert_eq!(app.canvas().digest(),libpixcheck::sha256(app.canvas().pixels()));let _=app.dirty();}
#[test] fn frames_reuse_completed_image() {
 let base=Canvas::new(1148,454);let bytes=base.pixels().len();let clean=libpixcheck::sha256(base.pixels());
 let mut app=App{history:History::new(base),clean};
 HASH_BYTES.store(0,Ordering::Relaxed);metadata(&app);
 assert_eq!(HASH_BYTES.load(Ordering::Relaxed),bytes,"digest and dirty must share one hash");
 let mut previous=app.canvas().clone();COPIED_BYTES.store(0,Ordering::Relaxed);
 for _ in 0..64 {metadata(&app);assert!(!app.canvas().render_changes(&previous,|_,_,_,_|panic!("unchanged frame drew pixels")));previous.clone_from(app.canvas());}
 assert_eq!(HASH_BYTES.load(Ordering::Relaxed),bytes,"unchanged frame rehashed the canvas");
 assert_eq!(COPIED_BYTES.load(Ordering::Relaxed),0,"retained frame copied full pixel buffers");
 let mut next=app.canvas().clone();next.brush(&[(12,14),(16,14)],Radius::Small,[1,2,3]);app.history.push(next);
 metadata(&app);assert!(app.dirty());assert_eq!(HASH_BYTES.load(Ordering::Relaxed),bytes*2);
 assert_eq!(previous.digest(),clean,"editing a shared snapshot changed the previous image");
 assert!(app.history.undo());metadata(&app);assert!(!app.dirty());
 assert!(app.history.redo());metadata(&app);assert!(app.dirty());
 assert_eq!(HASH_BYTES.load(Ordering::Relaxed),bytes*2,"undo/redo rehashed completed states");
 let mut changed=app.canvas().clone();let before=changed.digest();
 changed.pixels_mut()[0]=17;assert_ne!(changed.digest(),before);assert_eq!(changed.digest(),libpixcheck::sha256(changed.pixels()));
 for edit in [0,1,2,3,4] {
  let before=changed.digest();
  match edit {0=>changed.fill((0,0),[5,6,7]),1=>changed.erase(&[(0,0)],Radius::Small),2=>changed.rectangle((0,0),(3,3),[7,8,9]),3=>changed.circle((20,20),(24,20),[7,8,9]),_=>changed.grow(1149,455)};
  assert_ne!(changed.digest(),before);assert_eq!(changed.digest(),libpixcheck::sha256(changed.pixels()));
 }
}
}
'''.replace('CANVAS',source).replace('DIRTY',dirty))

if __name__=='__main__':unittest.main()
