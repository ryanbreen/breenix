"""Allocation pressure needs restored headroom rather than a complete accounting cohort."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from ui_idle_capture_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class PressureGoal(unittest.TestCase):
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
