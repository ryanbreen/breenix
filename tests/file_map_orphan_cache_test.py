"""Run orphan cache retirement before deferred disk reclamation, with bounded work."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


def run(source):
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory)
        (path / 'check.rs').write_text(source)
        command = ['rustc', '--edition=2021', '-Dwarnings', str(path / 'check.rs'), '-o', str(path / 'check')]
        if sys.platform == 'darwin':
            command = ['taskpolicy', '-b', *command]
        build = subprocess.run(command, capture_output=True, text=True)
        if build.returncode:
            raise AssertionError(build.stderr)
        return subprocess.run([str(path / 'check')], capture_output=True, text=True)


class OrphanCache(unittest.TestCase):
    def test_cache_drops_dirty_frames_with_a_budget_and_refuses_bindings(self):
        text = (ROOT / 'kernel/src/memory/file_map.rs').read_text()
        methods = item(text, '    pub(crate) fn discard_unobserved(') + item(text, '    fn refresh(')
        result = run(r'''
use std::{cell::Cell,rc::Rc,sync::{Mutex,MutexGuard,atomic::{AtomicBool,Ordering}}};
struct Lock<T>(Mutex<T>);
impl<T> Lock<T>{fn lock(&self)->MutexGuard<'_,T>{self.0.lock().unwrap()}}
struct Page{dirty:bool,released:Rc<Cell<usize>>}
impl Drop for Page{fn drop(&mut self){self.released.set(self.released.get()+1);}}
struct PageVec<T>{entries:Vec<(u64,T)>}
impl<T> PageVec<T>{
fn new()->Self{Self{entries:vec![]}}
fn len(&self)->usize{self.entries.len()}
fn is_empty(&self)->bool{self.entries.is_empty()}
}
struct MapInner{pages:PageVec<Page>,poisoned:PageVec<()>,bindings:Vec<u64>}
struct Map{inner:Lock<MapInner>,resident:AtomicBool,dirty:AtomicBool}
impl Map{METHODS}
fn main(){
    let released=Rc::new(Cell::new(0));
    let map=Map{inner:Lock(Mutex::new(MapInner{
        pages:PageVec{entries:(0..67).map(|i|(i,Page{dirty:i!=0,released:released.clone()})).collect()},
        poisoned:PageVec{entries:vec![(68,())]},bindings:vec![1]})),
        resident:AtomicBool::new(true),dirty:AtomicBool::new(true)};
    let mut budget=64;assert!(map.discard_unobserved(&mut budget));
    assert_eq!(budget,64);assert_eq!(released.get(),0,"live mapping lost its bytes");
    map.inner.lock().bindings.clear();
    assert!(map.discard_unobserved(&mut budget));assert_eq!(budget,0);assert_eq!(released.get(),64);
    assert!(map.resident.load(Ordering::Acquire));assert!(map.dirty.load(Ordering::Acquire));
    assert!(map.discard_unobserved(&mut budget));assert_eq!(released.get(),64);
    budget=64;assert!(!map.discard_unobserved(&mut budget));assert_eq!(budget,61);
    assert_eq!(released.get(),67);assert!(map.inner.lock().poisoned.is_empty());
    assert!(!map.resident.load(Ordering::Acquire));assert!(!map.dirty.load(Ordering::Acquire));
}
'''.replace('METHODS', methods))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_last_observer_cache_retires_before_disk_io(self):
        production = item((ROOT / 'kernel/src/fs/ext2/mod.rs').read_text(), '    fn finalize_pending_orphans(')
        source = r'''
use std::{cell::Cell,rc::Rc};
const RECLAIM_BUDGET:usize=128;
mod memory { pub mod file_map { pub const EVICT_BUDGET:usize=64; } }
#[derive(Debug,PartialEq)]enum Finalize{Idle,More,Retry}
struct Map{pages:Cell<usize>}
impl Map{fn discard_unobserved(&self,budget:&mut usize)->bool{
    let count=self.pages.get().min(*budget);self.pages.set(self.pages.get()-count);*budget-=count;self.pages.get()!=0
}}
struct Object{map:Map}
impl Object{fn unused(&self)->bool{true}}
struct Registry{object:Rc<Object>,pending:Cell<bool>}
impl Registry{fn pending(&self,_cursor:u32)->Vec<Rc<Object>>{
    if self.pending.get(){vec![self.object.clone()]}else{vec![]}
}}
struct Fs{live_inodes:Registry,finalization_cursor:u32,fail_disk:bool,disk_calls:usize,read_only:bool}
impl Fs{
fn is_read_only(&self)->bool{self.read_only}
fn finalize_orphans(&mut self,objects:&[Rc<Object>],budget:&mut usize)->Finalize{
    assert_eq!(*budget,RECLAIM_BUDGET,"cache retirement spent disk custody budget");
    self.disk_calls+=1;
    assert!(objects.iter().all(|o|o.map.pages.get()==0),"unobserved cache retained until disk reclamation");
    if self.fail_disk{Finalize::Retry}else{self.live_inodes.pending.set(false);Finalize::Idle}
}
PRODUCTION
}
fn main(){
    assert_eq!(memory::file_map::EVICT_BUDGET,64);
    let empty=Object{map:Map{pages:Cell::new(0)}};
    assert!(empty.unused());assert!(!empty.map.discard_unobserved(&mut 64));
    let object=Rc::new(Object{map:Map{pages:Cell::new(4003)}});
    let mut fs=Fs{live_inodes:Registry{object:object.clone(),pending:Cell::new(true)},finalization_cursor:0,fail_disk:true,disk_calls:0,read_only:false};
    for _ in 0..62{
        let before=object.map.pages.get();assert_eq!(fs.finalize_pending_orphans(),Finalize::More);
        assert_eq!(before-object.map.pages.get(),64);assert_eq!(fs.disk_calls,0);
    }
    assert_eq!(object.map.pages.get(),35);
    assert_eq!(fs.finalize_pending_orphans(),Finalize::Retry);
    assert_eq!(object.map.pages.get(),0);assert!(fs.live_inodes.pending.get(),"failed disk custody lost");
    fs.fail_disk=false;assert_eq!(fs.finalize_pending_orphans(),Finalize::Idle);assert!(!fs.live_inodes.pending.get());
    object.map.pages.set(3);fs.read_only=true;
    assert_eq!(fs.finalize_pending_orphans(),Finalize::Idle);assert_eq!(object.map.pages.get(),3);
}
'''.replace('PRODUCTION', production)
        result = run(source)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
