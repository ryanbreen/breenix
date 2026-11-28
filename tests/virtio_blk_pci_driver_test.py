#!/usr/bin/env python3
"""E4.1 review: the x86-64 PCI virtio-blk driver's interleavings, forced.

Compiles the production PCI driver pieces (request gate, slot completions,
ring and DMA access, the sector cache, and the device's transfer, flush,
stop and interrupt methods) with the production blk_queue engine against a
fake device over host memory, then drives interleavings in a fixed order:

- shared_publish: writer W stages its chain; before W publishes, a reader
  on other sectors publishes its own chain, which makes W's visible too.
  W's write completes, so the next flush must reach the device.

VIRTIO_BLK_ROOT points the harness at another tree (an older head) to show a
check red on that code.
"""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

from virtio_blk_mmio_driver_test import item

ROOT = Path(os.environ.get('VIRTIO_BLK_ROOT') or Path(__file__).resolve().parents[1])

ITEMS = [
    'struct BlockRequestGate', "struct BlockRequestGuard<'a>", 'impl BlockRequestGate',
    "impl BlockRequestGuard<'_>", "impl Drop for BlockRequestGuard<'_>", 'mod features',
    'struct CachedSector', 'struct SectorCache', 'impl SectorCache', 'struct PciRing',
    'unsafe impl Send for PciRing', 'impl RingMemory for PciRing', 'struct DirectMap;',
    'impl DmaMemory for DirectMap', 'struct SlotCompletion', 'impl SlotCompletion', "enum Data<'a>",
]
METHODS = [
    'fn next_completion_token(', 'fn wait_for_completion(', 'fn with_engine<R>(', 'fn owner_tags(',
    'fn stop(', 'fn transfer(', 'fn wait_for_slot(', 'fn invalidate_cached(', 'pub fn flush(',
    'pub fn handle_interrupt(',
]
QUEUE_TYPES = ['pub struct VirtqDesc', 'pub struct VirtqAvail', 'pub struct VirtqUsedElem', 'pub struct VirtqUsed {']
CONSTANTS = r'^(?:pub )?const (?:BLOCK_WAIT_SLICE_NS|BLOCK_BOOTSTRAP_WAIT_LIMIT_NS|WEDGED_ERROR|MAX_ADMITTED|CHAIN_FRAMES|SECTOR_SIZE|CACHE_SECTORS|CACHE_EMPTY):[^;]+;'

HARNESS = r'''
#![allow(dead_code, unused_imports, unused_variables, unused_mut, unused_unsafe, static_mut_refs)]
extern crate alloc;
use alloc::vec::Vec;
use std::cell::Cell;
use std::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex as StdMutex, OnceLock};
use std::time::Duration;
use core::ptr::{read_volatile, write_volatile};
#[path = "@BLK_QUEUE@"] mod blk_queue;
use blk_queue::*;
use spin::Mutex;

const SAFETY: Duration = Duration::from_secs(10);

mod spin {
    use std::cell::UnsafeCell;
    use std::sync::atomic::{AtomicBool, Ordering};
    pub struct Mutex<T> { locked: AtomicBool, value: UnsafeCell<T> }
    unsafe impl<T: Send> Sync for Mutex<T> {}
    pub struct MutexGuard<'a, T> { lock: &'a Mutex<T> }
    impl<T> Mutex<T> {
        pub const fn new(value: T) -> Self { Self { locked: AtomicBool::new(false), value: UnsafeCell::new(value) } }
        pub fn lock(&self) -> MutexGuard<'_, T> {
            while self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
                std::thread::yield_now();
            }
            MutexGuard { lock: self }
        }
    }
    impl<T> core::ops::Deref for MutexGuard<'_, T> { type Target = T; fn deref(&self) -> &T { unsafe { &*self.lock.value.get() } } }
    impl<T> core::ops::DerefMut for MutexGuard<'_, T> { fn deref_mut(&mut self) -> &mut T { unsafe { &mut *self.lock.value.get() } } }
    impl<T> Drop for MutexGuard<'_, T> { fn drop(&mut self) { self.lock.locked.store(false, Ordering::Release); } }
}

mod memory {
    pub struct Offset;
    impl Offset { pub fn as_u64(&self) -> u64 { 0 } }
    pub fn physical_memory_offset() -> Offset { Offset }
}

static CAN_SLEEP: AtomicBool = AtomicBool::new(true);
static ABORT: AtomicBool = AtomicBool::new(false);
static CLOCK: AtomicU64 = AtomicU64::new(1);
static SHARED_PUBLISH: AtomicBool = AtomicBool::new(false);

mod x86_64 { pub mod instructions { pub mod interrupts {
    pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
        crate::before_masked_section();
        f()
    }
} } }

mod task {
    pub mod thread { #[derive(PartialEq, Eq, Clone, Copy)] pub enum ThreadState { BlockedOnIO } }
    pub mod waitqueue {
        pub use crate::WaitQueueHead;
        pub fn schedule_current_wait() { crate::park() }
    }
    pub mod completion { pub fn wait_sleeps() -> bool { true } }
}

mod block {
    pub enum HardwareDisk { Virtio(usize) }
    pub fn hardware_disk_read_only(_: HardwareDisk) -> bool { false }
}

fn block_request_gate_can_sleep() -> bool { CAN_SLEEP.load(Ordering::SeqCst) }

thread_local! { static SEEN: Cell<u64> = Cell::new(0); }
static WAIT_GEN: StdMutex<u64> = StdMutex::new(0);
static WAIT_CV: Condvar = Condvar::new();
static PARK_HOOK: StdMutex<Option<fn()>> = StdMutex::new(None);

pub struct WaitQueueHead;
impl WaitQueueHead {
    pub const fn new() -> Self { Self }
    pub fn prepare_to_wait(&self, _: task::thread::ThreadState) -> Option<u64> {
        let generation = *WAIT_GEN.lock().unwrap();
        SEEN.with(|seen| seen.set(generation));
        Some(73)
    }
    pub fn finish_wait(&self) {}
    pub fn wake_up(&self) {
        *WAIT_GEN.lock().unwrap() += 1;
        WAIT_CV.notify_all();
    }
}

fn park() {
    let hook = *PARK_HOOK.lock().unwrap();
    if let Some(hook) = hook {
        hook();
        return;
    }
    panic!("a caller parked with nothing to wake it");
}

/// Completions are delivered by running the device and its interrupt
/// handler when a waiter finds its token missing.
pub struct Completion { token: StdMutex<u32> }
impl Completion {
    pub const fn new() -> Self { Self { token: StdMutex::new(0) } }
    pub fn reset(&self) { *self.token.lock().unwrap() = 0; }
    pub fn complete(&self, token: u32) { *self.token.lock().unwrap() = token; }
    pub fn completed_token(&self) -> u32 { *self.token.lock().unwrap() }
    pub fn wait_timeout_uninterruptible(&self, token: u32, _: u64) -> Result<bool, i32> {
        if self.completed_token() != token {
            device_complete_all();
            dev().handle_interrupt();
        }
        Ok(self.completed_token() == token)
    }
}

fn monotonic_now_ns() -> u64 { CLOCK.fetch_add(1, Ordering::SeqCst) }
fn report_slow_request(_: &VirtioBlockDevice, _: u32, _: u64) {}

struct Transport;
impl Transport {
    fn notify_queue(&self, _: u16) { device_take() }
    fn read_isr(&self) -> u8 { 1 }
    fn has_failed(&self) -> bool { ABORT.load(Ordering::SeqCst) }
}

/// The production struct's fields that the methods under test use.
pub struct VirtioBlockDevice {
    device: Transport,
    interrupt_line: u8,
    engine: Mutex<BlkQueue<PciRing>>,
    request_gate: BlockRequestGate,
    completions: [SlotCompletion; MAX_CHAINS],
    next_token: AtomicU32,
    capacity: u64,
    flush_supported: bool,
    read_only: bool,
    chain_bytes: usize,
    writes_pending: AtomicBool,
    ops_completed: AtomicU64,
    cache: Option<SectorCache>,
}

impl VirtioBlockDevice {
    fn irq_completion_available(&self) -> bool { true }
}

static DEV: OnceLock<&'static VirtioBlockDevice> = OnceLock::new();
fn dev() -> &'static VirtioBlockDevice { DEV.get().unwrap() }

struct Device { desc: usize, avail: usize, used: usize, size: u16, seen: u16, taken: Vec<u16>, used_idx: u16, disk: Vec<u8>, log: Vec<(u32, u64)> }
static DEVICE: StdMutex<Option<Device>> = StdMutex::new(None);

fn rd<T: Copy>(addr: usize) -> T { unsafe { read_volatile(addr as *const T) } }
fn wr<T: Copy>(addr: usize, value: T) { unsafe { write_volatile(addr as *mut T, value) } }

fn device_take() {
    let mut guard = DEVICE.lock().unwrap();
    let dev = guard.as_mut().unwrap();
    let idx: u16 = rd(dev.avail + 2);
    while dev.seen != idx {
        let head: u16 = rd(dev.avail + 4 + 2 * (dev.seen % dev.size) as usize);
        dev.taken.push(head);
        dev.seen = dev.seen.wrapping_add(1);
    }
}

fn device_complete_all() {
    device_take();
    let mut guard = DEVICE.lock().unwrap();
    let dev = guard.as_mut().unwrap();
    for head in core::mem::take(&mut dev.taken) {
        let desc = |i: u16| { let a = dev.desc + 16 * i as usize; (rd::<u64>(a) as usize, rd::<u32>(a + 8) as usize, rd::<u16>(a + 12), rd::<u16>(a + 14)) };
        let mut chain = Vec::new();
        let mut index = head;
        loop {
            let d = desc(index);
            chain.push(d);
            if d.2 & 1 == 0 { break; }
            index = d.3;
        }
        let kind: u32 = rd(chain[0].0);
        let sector: u64 = rd(chain[0].0 + 8);
        let mut offset = sector as usize * 512;
        let mut written = 1u32;
        for &(addr, len, _, _) in &chain[1..chain.len() - 1] {
            let bytes = unsafe { std::slice::from_raw_parts_mut(addr as *mut u8, len) };
            if kind == request_type::IN { bytes.copy_from_slice(&dev.disk[offset..offset + len]); written += len as u32; }
            else { dev.disk[offset..offset + len].copy_from_slice(bytes); }
            offset += len;
        }
        wr::<u8>(chain[chain.len() - 1].0, status::OK);
        dev.log.push((kind, sector));
        let pos = dev.used + 4 + 8 * (dev.used_idx % dev.size) as usize;
        wr::<u32>(pos, u32::from(head));
        wr::<u32>(pos + 4, written);
        fence(Ordering::SeqCst);
        dev.used_idx = dev.used_idx.wrapping_add(1);
        wr::<u16>(dev.used + 2, dev.used_idx);
    }
}

fn device_log() -> Vec<(u32, u64)> { DEVICE.lock().unwrap().as_ref().unwrap().log.clone() }

fn leak(bytes: usize) -> u64 {
    let layout = std::alloc::Layout::from_size_align(bytes, 4096).unwrap();
    unsafe { std::alloc::alloc_zeroed(layout) as u64 }
}

/// One modern device with FLUSH: a 16-entry ring and two one-frame chains.
fn setup() {
    const SIZE: u16 = 16;
    let desc = leak(16 * 256) as *mut VirtqDesc;
    let avail = leak(core::mem::size_of::<VirtqAvail>()) as *mut VirtqAvail;
    let used = leak(core::mem::size_of::<VirtqUsed>()) as *mut VirtqUsed;
    let mut storage = Vec::new();
    for _ in 0..2 {
        let mut chain = ChainStorage::EMPTY;
        chain.header = leak(16);
        chain.status = leak(16);
        chain.frames[0] = leak(4096);
        chain.frame_count = 1;
        storage.push(chain);
    }
    let limits = Limits { seg_max: 0, size_max: 0, exact_used_len: true };
    let engine = BlkQueue::new(PciRing { desc, avail, used }, SIZE, limits, &storage).expect("engine");
    let chain_bytes = engine.chain_bytes();
    let device = Box::leak(Box::new(VirtioBlockDevice {
        device: Transport,
        interrupt_line: 11,
        engine: Mutex::new(engine),
        request_gate: BlockRequestGate::new(),
        completions: [const { SlotCompletion::new() }; MAX_CHAINS],
        next_token: AtomicU32::new(0),
        capacity: 4096,
        flush_supported: true,
        read_only: false,
        chain_bytes,
        writes_pending: AtomicBool::new(true),
        ops_completed: AtomicU64::new(0),
        cache: None,
    }));
    let _ = DEV.set(device);
    *DEVICE.lock().unwrap() = Some(Device { desc: desc as usize, avail: avail as usize, used: used as usize, size: SIZE, seen: 0, taken: Vec::new(), used_idx: 0, disk: vec![0; 4096 * 512], log: Vec::new() });
}

/// shared_publish: just before writer W's own publish, with W's chain
/// staged, a reader on sectors 200..208 stages and publishes its chain.
fn before_masked_section() {
    if !SHARED_PUBLISH.load(Ordering::SeqCst) {
        return;
    }
    let staged = {
        let engine = dev().engine.lock();
        engine.outstanding() > engine.published_idx().wrapping_sub(engine.last_used())
    };
    if !staged || !SHARED_PUBLISH.swap(false, Ordering::SeqCst) {
        return;
    }
    let device = dev();
    let entry = device.request_gate.try_admit(200, 208, false).expect("the reader's range");
    device.with_engine(|engine| {
        let slot = engine.reserve(0x99).expect("a slot for the reader");
        device.completions[slot].completion.reset();
        engine.submit(&mut DirectMap, slot, request_type::IN, 200, None, 512).unwrap();
    });
    assert!(device.with_engine(|engine| engine.publish()), "the reader published nothing");
    device.device.notify_queue(0);
    device.request_gate.release(entry);
}

fn shared_publish() {
    setup();
    let device = dev();
    // The device starts clean: the first flush covers nothing yet written.
    assert_eq!(device.flush(), Ok(()));
    assert_eq!(device_log(), [(request_type::FLUSH, 0)]);
    let guard = device.request_gate.admit(0, 8, true).unwrap();
    SHARED_PUBLISH.store(true, Ordering::SeqCst);
    assert_eq!(device.transfer(guard, 0, Data::Write(&[0x33; 512])), Ok(()));
    assert!(!SHARED_PUBLISH.load(Ordering::SeqCst), "the reader never ran");
    let log = device_log();
    assert!(log.contains(&(request_type::OUT, 0)), "the write did not reach the device: {log:?}");
    // fsync after the completed write.
    assert_eq!(device.flush(), Ok(()));
    let flushes = device_log().iter().filter(|(kind, _)| *kind == request_type::FLUSH).count();
    assert_eq!(flushes, 2, "the flush after a completed write returned without sending FLUSH");
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("shared_publish") => shared_publish(),
        other => panic!("unknown scenario {other:?}"),
    }
}
'''


class VirtioBlkPciDriver(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        source = (ROOT / 'kernel/src/drivers/virtio/block.rs').read_text()
        queue = (ROOT / 'kernel/src/drivers/virtio/queue.rs').read_text()
        parts = ['#[repr(C)]\n#[derive(Clone, Copy)]\n' + item(queue, head) for head in QUEUE_TYPES]
        parts += [item(source, head) for head in ITEMS]
        parts.append('impl VirtioBlockDevice {\n' + '\n\n'.join(item(source, head) for head in METHODS) + '\n}')
        constants = '\n'.join(re.findall(CONSTANTS, source, re.M))
        harness = HARNESS.replace('@BLK_QUEUE@', str(ROOT / 'kernel/src/drivers/virtio/blk_queue.rs'))
        cls.tmp = tempfile.TemporaryDirectory()
        path = Path(cls.tmp.name)
        (path / 'model.rs').write_text(harness + '\n' + constants + '\n' + '\n\n'.join(parts))
        subprocess.run(['rustc', '--edition=2021', '-O', str(path / 'model.rs'), '-o', str(path / 'model')],
                       check=True, timeout=120)
        cls.model = path / 'model'

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def run_case(self, name):
        result = subprocess.run([str(self.model), name], capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_fsync_after_a_write_another_caller_published(self):
        self.run_case('shared_publish')


if __name__ == '__main__':
    unittest.main()
