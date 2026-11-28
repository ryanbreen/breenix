#!/usr/bin/env python3
"""E4.1 review: the ARM64 virtio-blk driver's interleavings, forced.

Compiles the production MMIO driver pieces (request gate, slot completions,
engine lock and drain, transfer, flush, the queue report and the interrupt
handler) with the production blk_queue engine against a fake device over host
memory, then drives interleavings in a fixed order:

- report: an interrupt arrives while the slow-request report holds the
  engine lock; the completion must still reach its waiter.
- late_clear: owner P takes its result on slot 0; owner X reserves slot 0
  before P's code has finished; P then finishes. X's chain completion must
  still wake X.

VIRTIO_BLK_ROOT points the harness at another tree (an older head) to show a
check red on that code.
"""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(os.environ.get('VIRTIO_BLK_ROOT') or Path(__file__).resolve().parents[1])


def block(source, start):
    at = source.index('{', start)
    depth = 1
    end = at + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


def item(source, head, optional=False):
    at = source.find(head)
    if at < 0:
        if optional:
            return ''
        raise AssertionError('production item missing: ' + head)
    # Unit structs end at their semicolon.
    if head.startswith('struct ') and source[at:].split('\n', 1)[0].rstrip().endswith(';'):
        return source[at:].split('\n', 1)[0]
    return block(source, at)


ITEMS = [
    'struct BlockMmioRequestGate', "struct BlockMmioRequestGuard<'a>", 'impl BlockMmioRequestGate',
    "impl BlockMmioRequestGuard<'_>", "impl Drop for BlockMmioRequestGuard<'_>",
    'fn block_mmio_request_gate_can_sleep(',
    'struct SlotCompletion', 'impl SlotCompletion', 'struct BlockMmioCompletion', 'impl BlockMmioCompletion',
    'mod features', 'struct BlockDeviceState', 'struct VirtqDesc', 'struct MmioRing', 'unsafe impl Send for MmioRing',
    'impl RingMemory for MmioRing',
    'struct DirectMap;', 'impl DmaMemory for DirectMap', 'struct DeviceQueue', 'impl DeviceQueue',
    'fn with_engine<R>(', 'fn drain_device(', 'fn stop_device(', 'struct TransferFailure',
    'fn request_error_reason(', "enum Data<'a>", 'fn transfer(', 'fn wait_for_slot(',
    'pub fn write_sectors(', 'pub fn flush(', 'fn report_queue_state(', 'pub fn handle_interrupt(',
]
# Present on some heads only.
OPTIONAL = ['fn owner_tags(', 'fn drain_after_release(', 'fn complete_token(', 'fn wake_all_slots(']
CONSTANTS = r'^(?:pub )?const (?:BLOCK_MMIO_WEDGED_ERROR|BLOCK_MMIO_WAIT_SLICE_NS|BLOCK_MMIO_BOOTSTRAP_WAIT_LIMIT_NS|MAX_ADMITTED|CHAIN_FRAMES|SECTOR_SIZE):[^;]+;'

HARNESS = r'''
#![allow(dead_code, unused_imports, unused_variables, unused_mut, unused_unsafe, static_mut_refs)]
extern crate alloc;
use std::cell::{Cell, UnsafeCell};
use std::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex as StdMutex, OnceLock};
use std::time::Duration;
use core::ptr::{read_volatile, write_volatile};
#[path = "@BLK_QUEUE@"] mod blk_queue;
use blk_queue::*;

const MAX_BLOCK_DEVICES: usize = 1;
const SAFETY: Duration = Duration::from_secs(10);

mod log {
    macro_rules! warn_report { ($($t:tt)*) => {{ let _ = format!($($t)*); }} }
    pub(crate) use warn_report as warn;
}

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
        pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
            self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).ok().map(|_| MutexGuard { lock: self })
        }
        pub fn is_locked(&self) -> bool { self.locked.load(Ordering::SeqCst) }
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
static WAIT_SLEEPS: AtomicBool = AtomicBool::new(true);
static AUTO_DEVICE: AtomicBool = AtomicBool::new(false);
static ABORT: AtomicBool = AtomicBool::new(false);
static IRQ_IN_BARRIER: AtomicBool = AtomicBool::new(false);
static WAITING: AtomicUsize = AtomicUsize::new(0);
static CLOCK: AtomicU64 = AtomicU64::new(1);

mod per_cpu_aarch64 {
    pub fn preempt_count() -> u32 { if crate::CAN_SLEEP.load(crate::Ordering::SeqCst) { 1 } else { 0 } }
}
mod arch_impl { pub mod aarch64 {
    pub mod timer_interrupt { pub fn is_initialized() -> bool { true } }
    pub mod mmio {
        pub unsafe fn read32(_: *const u32) -> u32 { 1 }
        pub unsafe fn write32(_: *mut u32, _: u32) {}
    }
} }
mod task {
    pub mod thread {
        pub struct KillCustody;
        impl KillCustody { pub fn try_enter() -> Option<Self> { Some(KillCustody) } }
        #[derive(PartialEq, Eq, Clone, Copy)] pub enum ThreadState { BlockedOnIO }
    }
    pub mod scheduler { pub fn current_thread_id() -> Option<u64> { Some(73) } }
    pub mod waitqueue { pub fn schedule_current_wait() { crate::park() } }
    pub mod completion { pub fn wait_sleeps() -> bool { crate::WAIT_SLEEPS.load(crate::Ordering::SeqCst) } }
}

fn dsb_sy() {
    // An interrupt from another CPU, delivered at the first barrier after
    // the test arms it: the device returns its chains and the handler runs.
    if IRQ_IN_BARRIER.swap(false, Ordering::SeqCst) {
        device_complete_all();
        handle_interrupt(0);
    }
}

thread_local! { static ROLE: Cell<u8> = Cell::new(0); static SEEN: Cell<u64> = Cell::new(0); }
const ROLE_P: u8 = 1;
static P_PAUSED: AtomicBool = AtomicBool::new(false);
static P_RESUME: AtomicBool = AtomicBool::new(false);
static P_HOOK_DONE: AtomicBool = AtomicBool::new(false);

fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let result = f();
    after_masked_section();
    result
}

/// late_clear: owner P stops just after the engine call that takes its
/// result (slot 0 free again, P's completion delivered), before the rest of
/// P's code runs.
fn after_masked_section() {
    if ROLE.with(|r| r.get()) != ROLE_P || P_HOOK_DONE.load(Ordering::SeqCst) {
        return;
    }
    let free = QUEUES[0].engine.lock().as_ref().map_or(false, |engine| engine.slot_state(0) == 'F');
    if free && COMPLETIONS[0].slots[0].completion.completed_token() != 0 {
        P_HOOK_DONE.store(true, Ordering::SeqCst);
        P_PAUSED.store(true, Ordering::SeqCst);
        until("P resumed", || P_RESUME.load(Ordering::SeqCst));
    }
}

fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !condition() {
        assert!(start.elapsed() < SAFETY, "harness timed out waiting for: {what}");
        std::thread::yield_now();
    }
}

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
    pub fn wake_up_one(&self) { self.wake_up() }
}

/// A parked caller: a scenario's hook stands in for the rest of the system,
/// or the thread sleeps until a wake after its prepare_to_wait.
fn park() {
    let hook = *PARK_HOOK.lock().unwrap();
    if let Some(hook) = hook {
        hook();
        return;
    }
    let seen = SEEN.with(|seen| seen.get());
    let mut generation = WAIT_GEN.lock().unwrap();
    while *generation == seen {
        let (next, timeout) = WAIT_CV.wait_timeout(generation, SAFETY).unwrap();
        generation = next;
        assert!(!timeout.timed_out(), "a parked caller was never woken");
    }
}

pub struct Completion { token: StdMutex<u32>, cv: Condvar }
impl Completion {
    pub const fn new() -> Self { Self { token: StdMutex::new(0), cv: Condvar::new() } }
    pub fn reset(&self) { *self.token.lock().unwrap() = 0; }
    pub fn complete(&self, token: u32) {
        *self.token.lock().unwrap() = token;
        self.cv.notify_all();
    }
    pub fn completed_token(&self) -> u32 { *self.token.lock().unwrap() }
    pub fn waiter_tid(&self) -> u64 { 0 }
    pub fn wait_timeout_uninterruptible(&self, token: u32, _: u64) -> Result<bool, i32> {
        if AUTO_DEVICE.load(Ordering::SeqCst) {
            if self.completed_token() != token {
                device_complete_all();
                handle_interrupt(0);
            }
            return Ok(self.completed_token() == token);
        }
        WAITING.fetch_add(1, Ordering::SeqCst);
        let mut held = self.token.lock().unwrap();
        while *held != token && !ABORT.load(Ordering::SeqCst) {
            held = self.cv.wait_timeout(held, Duration::from_millis(20)).unwrap().0;
        }
        let done = *held == token;
        drop(held);
        WAITING.fetch_sub(1, Ordering::SeqCst);
        Ok(done)
    }
}

fn device_has_failed(_: &BlockDeviceState) -> bool { ABORT.load(Ordering::SeqCst) }
fn monotonic_now_ns() -> u64 { CLOCK.fetch_add(1, Ordering::SeqCst) }
fn report_slow_request(_: u32, _: u64) {}
fn device_count() -> usize { 1 }
fn irq_completion_available() -> bool { true }
fn block_mmio_virt_base(_: &BlockDeviceState) -> u64 { 0x1000 }
static STATE: OnceLock<BlockDeviceState> = OnceLock::new();
fn block_device_state(_: usize) -> Result<&'static BlockDeviceState, &'static str> { STATE.get().ok_or("no device") }

struct VirtioMmioDevice;
impl VirtioMmioDevice {
    fn probe(_: u64) -> Option<Self> { Some(Self) }
    fn notify_queue(&self, _: u32) { device_take() }
}

/// The device: it reads the ring the engine wrote, through host memory.
struct Device { base: usize, size: u16, avail: usize, used: usize, seen: u16, taken: Vec<u16>, used_idx: u16, disk: Vec<u8>, log: Vec<(u32, u64)> }
static DEVICE: StdMutex<Option<Device>> = StdMutex::new(None);

fn rd<T: Copy>(addr: usize) -> T { unsafe { read_volatile(addr as *const T) } }
fn wr<T: Copy>(addr: usize, value: T) { unsafe { write_volatile(addr as *mut T, value) } }

fn device_take() {
    let mut guard = DEVICE.lock().unwrap();
    let dev = guard.as_mut().unwrap();
    let idx: u16 = rd(dev.base + dev.avail + 2);
    while dev.seen != idx {
        let head: u16 = rd(dev.base + dev.avail + 4 + 2 * (dev.seen % dev.size) as usize);
        dev.taken.push(head);
        dev.seen = dev.seen.wrapping_add(1);
    }
}

fn device_complete(head: u16) {
    let mut guard = DEVICE.lock().unwrap();
    let dev = guard.as_mut().unwrap();
    let at = dev.taken.iter().position(|h| *h == head).expect("the device holds the chain");
    dev.taken.remove(at);
    let desc = |i: u16| { let a = dev.base + 16 * i as usize; (rd::<u64>(a) as usize, rd::<u32>(a + 8) as usize, rd::<u16>(a + 12), rd::<u16>(a + 14)) };
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
    let pos = dev.base + dev.used + 4 + 8 * (dev.used_idx % dev.size) as usize;
    wr::<u32>(pos, u32::from(head));
    wr::<u32>(pos + 4, written);
    fence(Ordering::SeqCst);
    dev.used_idx = dev.used_idx.wrapping_add(1);
    wr::<u16>(dev.base + dev.used + 2, dev.used_idx);
}

fn device_complete_all() {
    device_take();
    let heads = DEVICE.lock().unwrap().as_ref().unwrap().taken.clone();
    for head in heads { device_complete(head); }
}

fn taken() -> usize { DEVICE.lock().unwrap().as_ref().unwrap().taken.len() }
fn device_log() -> Vec<(u32, u64)> { DEVICE.lock().unwrap().as_ref().unwrap().log.clone() }

fn leak(bytes: usize) -> u64 {
    let layout = std::alloc::Layout::from_size_align(bytes, 4096).unwrap();
    unsafe { std::alloc::alloc_zeroed(layout) as u64 }
}

/// One device: a 16-entry ring and two one-frame chains.
fn setup(device_features: u64) {
    const SIZE: u16 = 16;
    let layout = split_layout(SIZE, 4096);
    let base = leak(layout.bytes);
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
    let ring = MmioRing { base: base as *mut u8, avail: layout.avail, used: layout.used };
    let engine = BlkQueue::new(ring, SIZE, limits, &storage).expect("engine");
    let chain_bytes = engine.chain_bytes();
    *QUEUES[0].engine.lock() = Some(engine);
    let _ = STATE.set(BlockDeviceState { base: 0x1000, capacity: 4096, device_features, slot: 0, chain_bytes });
    *DEVICE.lock().unwrap() = Some(Device { base: base as usize, size: SIZE, avail: layout.avail, used: layout.used, seen: 0, taken: Vec::new(), used_idx: 0, disk: vec![0; 4096 * 512], log: Vec::new() });
}

fn report() {
    setup(features::FLUSH);
    let token = 0x51;
    let slot = with_engine(0, |engine| {
        let slot = engine.@RESERVE@.unwrap();
        COMPLETIONS[0].slots[slot].completion.reset();
        engine.submit(&mut DirectMap, slot, request_type::IN, 0, None, 512).unwrap();
        engine.publish();
        slot
    }).unwrap();
    device_take();
    // The report samples the queue; the device's interrupt lands on another
    // CPU while the report holds the engine lock.
    IRQ_IN_BARRIER.store(true, Ordering::SeqCst);
    report_queue_state(0);
    assert!(!IRQ_IN_BARRIER.load(Ordering::SeqCst), "the interrupt did not arrive during the report");
    assert_eq!(COMPLETIONS[0].slots[slot].completion.completed_token(), token,
        "the report released the engine and left the completed chain's waiter stranded");
}

fn late_clear() {
    setup(features::FLUSH);
    // Owner P writes sector 0 on slot 0.
    let p = std::thread::spawn(|| {
        ROLE.with(|r| r.set(ROLE_P));
        write_sectors(0, 0, &[0x11; 512])
    });
    until("P's chain on the device", || { device_take(); taken() == 1 });
    until("P waiting", || WAITING.load(Ordering::SeqCst) == 1);
    let p_head = DEVICE.lock().unwrap().as_ref().unwrap().taken[0];
    device_complete(p_head);
    handle_interrupt(0);
    // P has taken its result and slot 0 is free; P's code has not finished.
    until("P past its finish", || P_PAUSED.load(Ordering::SeqCst));
    // Owner X writes sector 64 and gets slot 0.
    let x = std::thread::spawn(|| write_sectors(0, 64, &[0x22; 512]));
    until("X's chain on the device", || { device_take(); taken() == 1 });
    until("X waiting", || WAITING.load(Ordering::SeqCst) == 1);
    assert_eq!(QUEUES[0].engine.lock().as_ref().unwrap().slot_state(0), 'I', "X reuses slot 0");
    P_RESUME.store(true, Ordering::SeqCst);
    assert_eq!(p.join().unwrap(), Ok(()));
    // The device returns X's chain and the handler drains it.
    let x_head = DEVICE.lock().unwrap().as_ref().unwrap().taken[0];
    device_complete(x_head);
    handle_interrupt(0);
    let delivered = COMPLETIONS[0].slots[0].completion.completed_token() != 0;
    if !delivered {
        ABORT.store(true, Ordering::SeqCst);
        let _ = x.join();
        panic!("X's chain was drained but X's completion was never delivered");
    }
    assert_eq!(x.join().unwrap(), Ok(()));
    assert_eq!(device_log(), [(request_type::OUT, 0), (request_type::OUT, 64)]);
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("report") => report(),
        Some("late_clear") => late_clear(),
        other => panic!("unknown scenario {other:?}"),
    }
}
'''


class VirtioBlkMmioDriver(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        source = (ROOT / 'kernel/src/drivers/virtio/block_mmio.rs').read_text()
        parts = [('#[repr(C)]\n#[derive(Clone, Copy)]\n' if head == 'struct VirtqDesc' else '') + item(source, head) for head in ITEMS]
        parts += [item(source, head, optional=True) for head in OPTIONAL]
        for name in ('REQUEST_GATES', 'COMPLETIONS', 'QUEUES'):
            match = re.search(r'^static ' + name + r':.*?;\n', source, re.M | re.S)
            assert match, name
            parts.append(match.group(0))
        constants = '\n'.join(re.findall(CONSTANTS, source, re.M))
        engine = (ROOT / 'kernel/src/drivers/virtio/blk_queue.rs').read_text()
        # Heads before the slot tag reserve without one.
        reserve = 'reserve(token)' if 'pub fn reserve(&mut self, tag: u32)' in engine else 'reserve()'
        harness = HARNESS.replace('@BLK_QUEUE@', str(ROOT / 'kernel/src/drivers/virtio/blk_queue.rs')).replace('@RESERVE@', reserve)
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

    def test_report_keeps_an_interrupt_completion(self):
        self.run_case('report')

    def test_a_late_owner_does_not_clear_the_next_owner(self):
        self.run_case('late_clear')


if __name__ == '__main__':
    unittest.main()
