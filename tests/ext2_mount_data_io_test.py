"""A namespace create can progress while mapped data writeback is in flight."""
from pathlib import Path
import unittest

from file_map_orphan_cache_test import run
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class MountDataIo(unittest.TestCase):
    def test_service_releases_namespace_before_data_write(self):
        service = item((ROOT / 'kernel/src/fs/ext2/writeback.rs').read_text(), 'fn service()')
        ext2 = (ROOT / 'kernel/src/fs/ext2/mod.rs').read_text()
        # The detached service helper is included when present; the old service
        # calls finalize_writeback directly under its mount guard.
        detached = (item(ext2, 'pub(super) fn service_mapped_writeback(')
                    if 'pub(super) fn service_mapped_writeback(' in ext2 else '')
        result = run(r'''
#![allow(dead_code)]
extern crate alloc;
use std::sync::{RwLock, RwLockWriteGuard, atomic::{AtomicBool, Ordering}};
static ROOT: RwLock<Option<Fs>> = RwLock::new(Some(Fs));
static HOME: RwLock<Option<Fs>> = RwLock::new(None);
static CREATED: AtomicBool = AtomicBool::new(false);
static DATA_STARTED: AtomicBool = AtomicBool::new(false);
struct FakeDevice;
impl FakeDevice {
    fn write(&self) {
        DATA_STARTED.store(true, Ordering::SeqCst);
        // The data request is in flight until this callback returns. No
        // scheduler or wall clock is involved: try_write is the ordered
        // concurrent namespace operation's admission point.
        let created = ROOT.try_write().is_ok();
        CREATED.store(created, Ordering::SeqCst);
        assert!(created, "create_new waits for another inode's in-flight data write");
    }
}
struct Fs;
impl Fs {
    fn finalize_writeback(&mut self) -> Finalize { FakeDevice.write(); Finalize::Idle }
    fn finalize_metadata(&mut self) -> Finalize { Finalize::Idle }
    fn finalize_pending_orphans(&mut self) -> Finalize { Finalize::Idle }
    fn mapped_writeback_handles(&self) -> Result<Vec<Handle>, &'static str> { Ok(vec![Handle { object: Object { map: Map } }]) }
}
struct Handle { object: Object }
struct Object { map: Map }
struct Map;
impl Map {
    fn service_writeback(&self, _handle: &Handle, _budget: &mut usize) -> Result<bool, &'static str> {
        FakeDevice.write(); Ok(false)
    }
}
enum Finalize { Idle, More, Retry }
fn fs_write_raw(home: bool) -> Result<RwLockWriteGuard<'static, Option<Fs>>, &'static str> {
    if home { Ok(HOME.write().unwrap()) } else { Ok(ROOT.write().unwrap()) }
}
mod memory { pub mod file_map { pub const EVICT_BUDGET: usize = 64; } }
DETACHED
mod per_cpu { pub fn preempt_disable() {} pub fn preempt_enable() {} }
mod per_cpu_aarch64 { pub fn preempt_disable() {} pub fn preempt_enable() {} }
mod worker {
    use super::*;
    static PENDING: AtomicBool = AtomicBool::new(true);
    static ORPHANS: AtomicBool = AtomicBool::new(false);
    const ORPHAN_COMMIT_NS: u64 = 1;
    static WORK: () = ();
    fn pause(_ns: u64) {}
    fn wait(_w: &(), _deadline: Option<u64>, _condition: impl FnOnce() -> bool) { panic!("finished") }
    SERVICE
}
fn main() {
    let result = std::panic::catch_unwind(worker::run);
    assert!(result.is_err());
    assert!(DATA_STARTED.load(Ordering::SeqCst), "service did not reach device data I/O");
    assert!(CREATED.load(Ordering::SeqCst), "namespace create could not complete during data I/O");
}
'''.replace('DETACHED', detached.replace('pub(super)', 'pub(crate)')).replace('SERVICE', service + '\npub fn run() { service(); }'))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_reserved_blocks_transfer_without_namespace_and_publish_after_data(self):
        from ext2_create_io_test import production, EXT2
        source = production()
        source = source.replace(item(source, 'pub mod file {'),
                                '#[path = "' + str(EXT2 / 'file.rs') + '"] pub mod file;')
        source = source.replace(item(source, 'pub mod block_group {'),
                                '#[path = "' + str(EXT2 / 'block_group.rs') + '"] pub mod block_group;')
        source = source.replace('pub device: Box<dyn BlockDevice>', 'pub device: alloc::sync::Arc<dyn BlockDevice>')
        source = source.replace('fn fail_writes(&mut self) {}', 'fn fail_writes(&mut self) { panic!("unexpected write fence"); }')
        source = source.replace('pub struct Ext2Fs {',
                                'pub struct Ext2Fs {')
        mapped = (EXT2 / 'mapped_io.rs').read_text()
        helpers = r'''
        pub fn data_commit_guard(_mount: usize) -> std::sync::MutexGuard<'static, Option<Ext2Fs>> {
            crate::MOUNT.lock().unwrap()
        }
        impl Ext2Fs { fn durable_epoch(&self) -> u64 { 0 } fn ensure_writable(&self) -> Result<(), &'static str> { Ok(()) } MAX_FILE_SIZE }
        pub mod mapped_io {
            use super::{file, Ext2Fs, Ext2Inode, Ext2Superblock};
            use alloc::{sync::Arc, vec::Vec};
            use crate::block::BlockDevice;
            use std::sync::atomic::{AtomicU64, Ordering};
            pub struct Map { disk_epoch: AtomicU64, disk_write_seq: AtomicU64 }
            impl Map {
                pub fn new() -> Arc<Self> { Arc::new(Self { disk_epoch: AtomicU64::new(0), disk_write_seq: AtomicU64::new(0) }) }
                pub fn complete_writeback(&self, _first: u64, _last: u64) {}
                MAP_METHODS
            }
            pub struct Key { pub inode: u32 }
            pub struct Object { pub mount: usize, pub map: Arc<Map>, pub key: Key }
            pub struct Handle { pub object: Object }
            impl Handle { pub fn verify(&self, _fs: &Ext2Fs) -> Result<u32, &'static str> { Ok(12) } }
            pub struct DataFile {
                pub handle: Handle, pub device: Arc<dyn BlockDevice>,
                pub inode: Ext2Inode, pub superblock: Ext2Superblock,
            }
            impl DataFile {
                fn size(&self) -> u64 { self.inode.size() }
                WRITE
                SYNC
            }
            COUNTS
        }
        fn content_privilege(_mode: u16) -> u16 { 0 }
        '''.replace('WRITE', item(mapped, '    pub(crate) fn write(')) \
             .replace('SYNC', item(mapped, '    pub(crate) fn sync(')) \
             .replace('MAP_METHODS', '\n'.join(item((ROOT / 'kernel/src/memory/file_map.rs').read_text(), marker) for marker in [\
                 '    pub(crate) fn note_disk_write(', '    pub(crate) fn disk_write_seq(',\
                 '    pub(crate) fn disk_epoch(', '    pub(crate) fn record_data_durable('])) \
             .replace('COUNTS', item(mapped, 'fn persist_counts(')) \
             .replace('MAX_FILE_SIZE', item((EXT2 / 'mod.rs').read_text(), '    pub fn max_file_size(')) \
             .replace('fn content_privilege(_mode: u16) -> u16 { 0 }', item((EXT2 / 'mod.rs').read_text(), 'fn content_privilege('))
        # Insert inside ext2 alongside the production create implementation.
        source = source.replace('pub mod directory_io {', helpers + 'pub mod directory_io {')
        fixture = (ROOT / 'tests/fixtures/ext2_create_io.rs').read_text()
        fixture = fixture[:fixture.index('fn main()')]
        fixture = fixture.replace('device:Box::new(disk)', 'device:Arc::new(disk)')
        fixture = fixture.replace('if group==groups-1 {',
                                  'bytes[12..14].copy_from_slice(&7000u16.to_le_bytes()); if group==groups-1 {')
        fixture = fixture.replace('let disk=Disk(state.clone());', r'''
    let disk=Disk(state.clone());
    let mut used=[0u8;1024];
    for block in 1..750 { used[(block-1)/8] |= 1 << ((block-1)%8); }
    file::write_ext2_block(&disk,100,1024,&used).unwrap();
''')
        fixture = fixture.replace('fn flush(&self) -> Result<(), BlockError> { Ok(()) }', r'''
    fn flush(&self) -> Result<(), BlockError> {
        if let Some(map) = FLUSH_MAP.lock().unwrap().take() {
            let _guard=MOUNT.try_lock().expect("device flush holds namespace");
            map.note_disk_write(0); // Metadata written after the barrier started.
        }
        Ok(())
    }
''')
        fixture = fixture.replace('let mut s = self.0.lock().unwrap();\n        s.sectors.insert', r'''
        if bytes[..512].iter().all(|byte| *byte == 0x5a) && !IN_CALLBACK.swap(true, std::sync::atomic::Ordering::SeqCst) {
            // Device data completion is deliberately held until a real
            // production namespace insertion has finished on the same mount.
            let mut guard=MOUNT.try_lock().expect("data write holds namespace");
            let fs=guard.as_mut().unwrap();
            let inode=fs.read_inode(12).unwrap();
            assert!(file::get_block_num(fs.device.as_ref(), &inode, &fs.superblock, 0).unwrap().is_none(),
                    "new data pointer published before initialization completed");
            fs.create_node(2,"concurrent-create",0o100600,0,0,0).unwrap();
            CREATED.store(true,std::sync::atomic::Ordering::SeqCst);
        }
        let mut s = self.0.lock().unwrap();
        s.sectors.insert''')
        result = run(r'''#![allow(dead_code)]
extern crate self as spin;
extern crate self as log;
#[macro_export] macro_rules! warn { ($($t:tt)*) => {{ let _ = format_args!($($t)*); }}; }
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> { pub fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() } }
''' + source + fixture.replace('sync::{Arc, Mutex}', 'sync::{Arc, Mutex as StdMutex}').replace('Mutex<State>', 'StdMutex<State>').replace('Mutex::new(State::default())', 'StdMutex::new(State::default())') + r'''
static MOUNT: std::sync::Mutex<Option<Ext2Fs>> = std::sync::Mutex::new(None);
static FLUSH_MAP: std::sync::Mutex<Option<Arc<fs::ext2::mapped_io::Map>>> = std::sync::Mutex::new(None);
static CREATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static IN_CALLBACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn main() {
    let (mut fs,state)=fixture(1);
    // The fixture reserves inodes 1..11, so create inode 12 for the mapping.
    let ino=fs.create_node(2,"mapped",0o100600,0,0,0).unwrap(); assert_eq!(ino,12);
    let mut inode=fs.read_inode(ino).unwrap(); inode.i_size=16384;
    fs.write_inode(ino,&inode).unwrap();
    let data=fs::ext2::mapped_io::DataFile {
        handle:fs::ext2::mapped_io::Handle { object:fs::ext2::mapped_io::Object { mount:0, map:fs::ext2::mapped_io::Map::new(), key:fs::ext2::mapped_io::Key { inode:12 } } },
        device:fs.device.clone(),inode,superblock:fs.superblock,
    };
    *MOUNT.lock().unwrap()=Some(fs);
    assert_eq!(data.write(0,&[0x5a;16384]).unwrap(),16384);
    assert!(CREATED.load(std::sync::atomic::Ordering::SeqCst));
    let guard=MOUNT.lock().unwrap(); let fs=guard.as_ref().unwrap();
    let inode=fs.read_inode(ino).unwrap();
    let bytes=file::read_file_range(fs.device.as_ref(),&inode,&fs.superblock,0,16384).unwrap();
    assert_eq!(bytes, [0x5a;16384]);
    let blocks=inode.i_blocks; assert_eq!(blocks,34); // 16 data blocks and one indirect block.
    assert!(fs.read_directory(&fs.read_inode(2).unwrap()).unwrap().windows(17).any(|s|s==b"concurrent-create"));
    assert!(!state.lock().unwrap().writes.is_empty());
    drop(guard);
    *FLUSH_MAP.lock().unwrap()=Some(data.handle.object.map.clone());
    data.sync(0,u64::MAX).unwrap();
    assert_eq!(data.handle.object.map.disk_epoch(),0,
               "barrier certified metadata submitted after it started");
    data.sync(0,u64::MAX).unwrap();
    assert_eq!(data.handle.object.map.disk_epoch(),u64::MAX);
    // Repeating a clean snapshot performs reads/comparisons only, including
    // through the indirect root installed by the first detached transfer.
    reset(&state);
    assert_eq!(data.write(0,&[0x5a;16384]).unwrap(),16384);
    assert!(state.lock().unwrap().writes.is_empty(), "identical writeback rewrote clean data or metadata");
    {
        let mut guard=MOUNT.lock().unwrap(); let fs=guard.as_mut().unwrap();
        let mut inode=fs.read_inode(ino).unwrap(); inode.i_mode |= 0o6010;
        fs.write_inode(ino,&inode).unwrap();
    }
    data.write(0,&[0x5a;16384]).unwrap();
    {
        let guard=MOUNT.lock().unwrap(); let inode=guard.as_ref().unwrap().read_inode(ino).unwrap();
        let mode=inode.i_mode; assert_eq!(mode & 0o6000,0, "identical mapped writeback retained set-ID bits");
    }
    // A new inode with a partially completed data request must retain no
    // pointer to that uninitialized block, and its reservations are released.
    let (mut fs,failed_state)=fixture(1);
    let ino=fs.create_node(2,"mapped",0o100600,0,0,0).unwrap();
    let mut inode=fs.read_inode(ino).unwrap(); inode.i_size=4096;
    fs.write_inode(ino,&inode).unwrap();
    let failed=fs::ext2::mapped_io::DataFile {
        handle:fs::ext2::mapped_io::Handle { object:fs::ext2::mapped_io::Object { mount:0, map:fs::ext2::mapped_io::Map::new(), key:fs::ext2::mapped_io::Key { inode:12 } } },
        device:fs.device.clone(),inode,superblock:fs.superblock,
    };
    let free=fs.block_groups[0].bg_free_blocks_count;
    failed_state.lock().unwrap().fail=Some(1500);
    *MOUNT.lock().unwrap()=Some(fs);
    assert!(failed.write(0,&[0x5a;4096]).is_err());
    let guard=MOUNT.lock().unwrap(); let fs=guard.as_ref().unwrap();
    let inode=fs.read_inode(ino).unwrap(); let pointers=inode.i_block; let count=inode.i_blocks;
    assert_eq!(pointers,[0;15]); assert_eq!(count,0);
    let after=fs.block_groups[0].bg_free_blocks_count; assert_eq!(after,free);
    drop(guard);
    assert_eq!(failed.write(0,&[0x5a;4096]).unwrap(),4096);
}
''')
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
