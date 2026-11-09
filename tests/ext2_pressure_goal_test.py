"""Allocation pressure needs restored headroom rather than a complete accounting cohort."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from ui_idle_capture_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class PressureGoal(unittest.TestCase):
    def test_headroom_stops_early_but_accounting_drains_its_snapshot(self):
        text = (ROOT / 'kernel/src/fs/ext2/mod.rs').read_text()
        methods = '\n'.join(item(text, marker) for marker in [
            'fn allocation_pressure(', 'pub(crate) fn accounting_fs_write(', 'fn reclaim_fs_write('])
        result = run_rust(r'''
use std::sync::{Mutex,atomic::{AtomicUsize,Ordering}};
const RECLAIM_BUDGET:usize=128;
static HELD:AtomicUsize=AtomicUsize::new(0);
struct State{free:u64,inodes:u64,starts:Vec<usize>,snapshots:usize,id:u64,change:bool,retry:bool,more:bool,requested:bool,homes:Vec<bool>}
impl State{const fn new()->Self{Self{free:999,inodes:32,starts:Vec::new(),snapshots:0,id:1,change:false,retry:false,more:false,requested:false,homes:Vec::new()}}}
static STATE:Mutex<State>=Mutex::new(State::new());
struct Stats{free_blocks:u64,blocks:u64,free_inodes:u64}
#[derive(Clone,Copy)]struct Pin(u64);
impl Pin{fn verify(self,fs:&Ext2Fs)->Result<(),&'static str>{if self.0==fs.mount_pin.0{Ok(())}else{Err("changed mount")}}}
struct LiveInodes;
impl LiveInodes{fn pending_snapshot(&self)->Result<Vec<usize>,&'static str>{STATE.lock().unwrap().snapshots+=1;Ok((0..96).collect())}}
enum Finalize{Idle,More,Retry}
struct Ext2Fs{mount_pin:Pin,live_inodes:LiveInodes}
impl Ext2Fs{
 fn statfs(&self)->Stats{let s=STATE.lock().unwrap();Stats{free_blocks:s.free,blocks:16000,free_inodes:s.inodes}}
 fn finalize_orphans(&mut self,objects:&[usize],budget:&mut usize)->Finalize{
  assert_eq!(*budget,128);*budget=0;
  let mut s=STATE.lock().unwrap();s.starts.push(objects[0]);
  if s.retry{return Finalize::Retry;}
  s.free+=128;s.inodes+=32;
  if std::mem::take(&mut s.more){Finalize::More}else{Finalize::Idle}
 }
}
struct Ext2WriteGuard(Option<Ext2Fs>);
impl Ext2WriteGuard{
 fn as_ref(&self)->Option<&Ext2Fs>{self.0.as_ref()}
 fn as_mut(&mut self)->Option<&mut Ext2Fs>{self.0.as_mut()}
}
impl Drop for Ext2WriteGuard{fn drop(&mut self){HELD.fetch_sub(1,Ordering::Relaxed);}}
fn fs_write_raw(home:bool)->Result<Ext2WriteGuard,&'static str>{
 let mut s=STATE.lock().unwrap();s.homes.push(home);HELD.fetch_add(1,Ordering::Relaxed);
 Ok(Ext2WriteGuard(Some(Ext2Fs{mount_pin:Pin(s.id),live_inodes:LiveInodes})))
}
mod writeback{
 pub fn pause(ns:u64){assert_eq!(ns,1_000_000);assert_eq!(crate::HELD.load(crate::Ordering::Relaxed),0);let mut s=crate::STATE.lock().unwrap();if s.change{s.id=2;}}
 pub fn request(){crate::STATE.lock().unwrap().requested=true;}
}
METHODS
fn reset(){assert_eq!(HELD.load(Ordering::Relaxed),0);*STATE.lock().unwrap()=State::new();}
fn main(){
 reset();drop(reclaim_fs_write(true,true).unwrap());
 {let s=STATE.lock().unwrap();assert_eq!(s.starts,[0]);assert_eq!(s.snapshots,1);assert_eq!(s.homes,[true,true]);assert!(s.free>1000&&s.inodes>32);}
 reset();drop(accounting_fs_write(false).unwrap());
 {let s=STATE.lock().unwrap();assert_eq!(s.starts,[0,32,64]);assert_eq!(s.snapshots,1);assert_eq!(s.homes,[false,false,false]);}
 reset();STATE.lock().unwrap().more=true;drop(accounting_fs_write(false).unwrap());
 assert_eq!(STATE.lock().unwrap().starts,[0,0,32,64]);
 reset();STATE.lock().unwrap().retry=true;assert!(reclaim_fs_write(false,true).is_err());
 assert!(STATE.lock().unwrap().requested);
 reset();STATE.lock().unwrap().change=true;assert!(reclaim_fs_write(false,true).is_err(),"headroom return bypassed mount identity");
 reset();{let mut s=STATE.lock().unwrap();s.free=1001;s.inodes=33;}drop(reclaim_fs_write(false,true).unwrap());
 assert!(STATE.lock().unwrap().starts.is_empty());
 reset();{let mut s=STATE.lock().unwrap();s.free=1001;}drop(reclaim_fs_write(false,true).unwrap());
 assert_eq!(STATE.lock().unwrap().starts,[0],"inode pressure was ignored");
}
'''.replace('METHODS', methods))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_allocating_writer_selects_headroom_not_full_accounting(self):
        body = item((ROOT / 'kernel/src/fs/ext2/mod.rs').read_text(), 'fn fs_write(home: bool)')
        result = run_rust(r'''
use std::sync::atomic::{AtomicUsize,Ordering};
static FULL:AtomicUsize=AtomicUsize::new(0);
static HEADROOM:AtomicUsize=AtomicUsize::new(0);
struct Stats{free_blocks:u64,blocks:u64,free_inodes:u64}
struct Ext2Fs;
impl Ext2Fs{
 fn ensure_writable(&self)->Result<(),&'static str>{Ok(())}
 fn statfs(&self)->Stats{Stats{free_blocks:999,blocks:16000,free_inodes:100}}
}
struct Ext2WriteGuard(Option<Ext2Fs>);
impl Ext2WriteGuard{
 fn as_ref(&self)->Option<&Ext2Fs>{self.0.as_ref()}
 fn wait_for_shrink_writer(self)->Result<Self,&'static str>{Ok(self)}
}
fn fs_write_raw(_:bool)->Result<Ext2WriteGuard,&'static str>{Ok(Ext2WriteGuard(Some(Ext2Fs)))}
fn accounting_fs_write(home:bool)->Result<Ext2WriteGuard,&'static str>{FULL.fetch_add(1,Ordering::Relaxed);fs_write_raw(home)}
fn reclaim_fs_write(home:bool,pressure_only:bool)->Result<Ext2WriteGuard,&'static str>{assert!(pressure_only);HEADROOM.fetch_add(1,Ordering::Relaxed);fs_write_raw(home)}
fn allocation_pressure(fs:&Ext2Fs)->bool{let stats=fs.statfs();stats.free_blocks<=(stats.blocks/16).max(128)||stats.free_inodes<=32}
BODY
fn main(){
 assert!(allocation_pressure(&Ext2Fs));
 assert!(accounting_fs_write(false).is_ok());assert!(reclaim_fs_write(false,true).is_ok());
 FULL.store(0,Ordering::Relaxed);HEADROOM.store(0,Ordering::Relaxed);
 for home in [false,true]{assert!(fs_write(home).is_ok());}
 assert_eq!(FULL.load(Ordering::Relaxed),0,"allocation waits for unrelated accounting cohort");
 assert_eq!(HEADROOM.load(Ordering::Relaxed),2);
}
'''.replace('BODY', body))
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
