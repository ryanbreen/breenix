"""Replay the shipped font layout through the production ext2 reader and cache."""
from pathlib import Path
import unittest

from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class FontTrace(unittest.TestCase):
    def test_font_read_requests_and_immediate_reread(self):
        source = (ROOT / 'kernel/src/fs/ext2/file.rs').read_text()
        reader = '\n'.join(item(source, name) for name in [
            'pub fn read_ext2_block<', 'pub fn read_ext2_blocks<',
            'pub fn get_block_num<', 'pub fn read_file_range<', 'fn read_indirect_block<',
        ])
        cache = (ROOT / 'kernel/src/fs/ext2/sector_cache.rs').read_text().split('#[cfg(test)]')[0]
        # The fixture exercises reads only; omit the unused write API, preserving
        # production lookup, fill, set selection and replacement without edits.
        cache = cache.replace('#[derive(Clone, Copy)]\npub(super) struct Store(Option<u64>);', '')
        for name in ['pub(super) fn begin_write(', 'pub(super) fn end_write(']:
            cache = cache.replace(item(cache, name), '')
        cache = cache.replace('///', '//').replace('//!', '//')
        cache = cache.replace('use alloc::', 'use std::').replace('use spin::Mutex;', 'use crate::HostMutex as Mutex;')
        run_rust(self, r'''
extern crate alloc;
struct HostMutex<T>(std::sync::Mutex<T>);
impl<T> HostMutex<T> {
    fn new(value:T)->Self { Self(std::sync::Mutex::new(value)) }
    fn lock(&self)->std::sync::MutexGuard<'_,T> { self.0.lock().unwrap() }
}
fn arch_without_interrupts<R>(f:impl FnOnce()->R)->R { f() }
mod cache { @CACHE@ }
#[derive(Debug)] pub enum BlockError { IoError, OutOfMemory }
pub trait BlockDevice {
    fn block_size(&self)->usize { 512 }
    fn read_blocks(&self, start:u64, count:usize, buf:&mut[u8])->Result<(),BlockError>;
}
pub struct Ext2Inode { i_block:[u32;15] }
impl Ext2Inode { fn size(&self)->u64 { 757076 } }
pub struct Ext2Superblock { s_blocks_count:u32 }
impl Ext2Superblock { fn block_size(&self)->usize { 1024 } }
const DIRECT_BLOCKS:u32=12;
const SINGLE_INDIRECT:usize=12;
const DOUBLE_INDIRECT:usize=13;
const TRIPLE_INDIRECT:usize=14;
@READER@
struct Disk { cache:cache::SectorCache, requests:std::cell::RefCell<Vec<(u64,usize)>> }
impl BlockDevice for Disk {
 fn read_blocks(&self,start:u64,count:usize,buf:&mut[u8])->Result<(),BlockError> {
    let buf=&mut buf[..count*512];
    let fill=match self.cache.lookup(start,buf) { Ok(())=>return Ok(()), Err(fill)=>fill };
    // Model the ARM DMA chunk boundary; no timing or completion is simulated.
    for (i,chunk) in buf.chunks_mut(4096).enumerate() {
        self.requests.borrow_mut().push((start+i as u64*8,chunk.len()));
    }
    buf.fill(0);
    for (sector,bytes) in (start..).zip(buf.chunks_exact_mut(512)) {
        let block=sector/2;
        for (i,word) in bytes.chunks_exact_mut(4).enumerate() {
            let index=(sector%2) as usize*128+i;
            let pointer=match block {
                304220=>physical(12+index),
                304221=>if index<2 {304222+index as u32} else {0},
                304222=>physical(268+index),
                304223=>if index<216 {physical(524+index)} else {0},
                _=>0,
            };
            word.copy_from_slice(&pointer.to_le_bytes());
        }
    }
    self.cache.fill(fill,start,buf);
    Ok(())
 }
}
// debugfs stat, shipped image SHA256 feac6807...: seven contiguous data extents.
fn physical(logical:usize)->u32 {
 for (first,count,block) in [(0,16,231425),(16,16,307057),(32,32,307105),
    (64,64,181473),(128,128,304385),(256,256,182474),(512,228,182785)] {
    if (first..first+count).contains(&logical) { return block+(logical-first) as u32; }
 }
 panic!("outside font layout: {logical}")
}
#[test] fn cold_then_warm() {
 let disk=Disk{cache:cache::SectorCache::new(512).unwrap(),requests:Default::default()};
 let mut inode=Ext2Inode{i_block:[0;15]};
 for i in 0..12 {inode.i_block[i]=physical(i);}
 inode.i_block[12]=304220;inode.i_block[13]=304221;
 let sb=Ext2Superblock{s_blocks_count:307200};
 for (label,expected) in [("cold",189),("immediate reread",188)] {
    disk.requests.borrow_mut().clear();
    assert_eq!(read_file_range(&disk,&inode,&sb,0,757076).unwrap().len(),757076);
    let requests=disk.requests.borrow();
    assert_eq!(requests.len(),expected,"{label}");
    let data:Vec<_>=requests.iter().filter(|(s,_)| ![304220,304221,304222,304223].contains(&(*s/2))).collect();
    assert_eq!(data.len(),185,"{label}");
    assert_eq!(data.iter().map(|(_,n)| n).sum::<usize>(),740*1024);
 }
 disk.requests.borrow_mut().clear();
 assert!(read_file_range(&disk,&inode,&sb,757076,32).unwrap().is_empty());
 assert!(disk.requests.borrow().is_empty(),"EOF has no content I/O");
}
'''.replace('@CACHE@', cache).replace('@READER@', reader))


if __name__ == '__main__':
    unittest.main()
