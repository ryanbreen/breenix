"""Force claim loss before first composite through the real readback/descriptor path."""
from pathlib import Path
import unittest

from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class ClaimReadback(unittest.TestCase):
    def test_pending_read_retries_claim_loss_instead_of_timing_out(self):
        ops = (ROOT / 'kernel/src/window/ops.rs').read_text()
        events = (ROOT / 'kernel/src/window/events.rs').read_text()
        readback = item((ROOT / 'kernel/src/syscall/window.rs').read_text(), 'fn readback(')
        gate = item(readback, 'if reg.present.busy')
        helper = item((ROOT / 'libs/libbreenix/src/window.rs').read_text(),
                      'pub fn readback_until(')
        code = r'''
#![feature(allocator_api)]
use std::{collections::BTreeMap, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}}};
use ipc::object::IpcFile;
pub mod spin {
    pub struct Once<T>(std::sync::OnceLock<T>);
    impl<T> Once<T> {
        pub const fn new() -> Self { Self(std::sync::OnceLock::new()) }
        pub fn get(&self) -> Option<&T> { self.0.get() }
        pub fn call_once(&self, f: impl FnOnce() -> T) -> &T { self.0.get_or_init(f) }
    }
}
pub mod breengel_abi { pub mod errno { pub const ENOMEM: i32=12; pub const EINVAL: i32=22; pub const EFAULT: i32=14; pub const EAGAIN: i32=11; } }
pub mod errno { #[derive(Debug, PartialEq)] pub enum Errno { EAGAIN, EINTR, ETIMEDOUT } }
pub mod error { #[derive(Debug, PartialEq)] pub enum Error { Os(super::errno::Errno) } }
pub struct WaitQueueHead(AtomicUsize);
impl WaitQueueHead {
    pub fn new() -> Self { Self(AtomicUsize::new(0)) }
    pub fn wake_up(&self) {
        // A wake while holding the registry is a publication/lock-order defect.
        assert!(REGISTRY.0.try_lock().unwrap().claim.is_none());
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
pub mod ipc {
    pub mod poll { pub mod events { pub const POLLIN: i16=1; } }
    pub mod object { pub trait IpcFile {
        fn as_any(&self) -> &dyn core::any::Any;
        fn fill_stat(&self, st: &mut crate::syscall::fs::Stat);
        fn poll_events(&self) -> i16;
        fn poll_queue(&self) -> Option<std::sync::Arc<crate::WaitQueueHead>>;
        fn read(&self, buf: u64, count: usize) -> Result<u64,u64>;
    } }
}
pub mod syscall {
    pub mod fs { pub struct Stat { pub st_mode: u32, pub st_nlink: u32, pub st_blksize: u32 } }
    pub mod userptr { pub fn copy_to_user(dst: *mut u64, src: &u64) -> Result<(),u64> {
        unsafe { dst.write(*src); } Ok(())
    } }
}
pub mod events { use super::*; @DESCRIPTOR@ }
#[derive(Clone, Copy)] pub struct Held { pub first_serial: u64 }
pub struct Present { pub serial: u64, pub busy: bool }
pub struct Event { pub kind: u16, pub epoch: u64 }
pub mod event_kind { pub const COMPOSITOR_CHANGED: u16=13; }
fn kernel_event(kind: u16) -> Event { Event { kind, epoch: 0 } }
pub struct Ring(Vec<Event>);
impl Ring { fn push(&mut self, event: Event) { self.0.push(event); } }
pub struct Waits { pub events: WaitQueueHead }
pub struct Window { pub ring: Ring, pub waits: Arc<Waits> }
pub struct Registry {
    pub claim: Option<Held>, pub present: Present, pub compositor_tree: Option<()>,
    pub epoch: u64, pub windows: BTreeMap<u64,Window>,
}
pub struct Lock(Mutex<Registry>);
impl Lock { fn lock(&self) -> std::sync::MutexGuard<'_,Registry> { self.0.lock().unwrap() } }
static REGISTRY: Lock = Lock(Mutex::new(Registry { claim: None,
    present: Present { serial: 7, busy: false }, compositor_tree: None,
    epoch: 0, windows: BTreeMap::new() }));
mod ops {
    use super::*;
    @END@
    @WAITS@
    @WAKE@
    pub fn lose_claim() {
        let readers = { let mut reg=REGISTRY.lock(); end_claim(&mut reg) };
        claim_ended(readers);
    }
}
fn observe() -> Result<u64,i32> {
    let reg = REGISTRY.lock();
    @GATE@
    Ok(reg.present.serial)
}
static FILE: Mutex<Option<events::PresentationFile>> = Mutex::new(None);
static NOW: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static POLLS: AtomicUsize = AtomicUsize::new(0);
static BEFORE_POLL: AtomicBool = AtomicBool::new(false);
static CLOSED: AtomicUsize = AtomicUsize::new(0);
pub mod types {
    #[derive(Clone, Copy)] pub struct Fd(u64);
    impl Fd { pub fn from_raw(raw: u64) -> Self { Self(raw) } pub fn raw(self) -> u64 { self.0 } }
    pub struct OwnedFd(Fd);
    impl OwnedFd { pub fn new(fd: Fd) -> Self { Self(fd) } pub fn fd(&self) -> Fd { self.0 } }
    impl Drop for OwnedFd { fn drop(&mut self) {
        super::FILE.lock().unwrap().take(); super::CLOSED.fetch_add(1, super::Ordering::SeqCst);
    } }
}
pub mod time {
    pub struct Stamp(u64);
    impl Stamp { pub fn as_nanos(&self) -> u64 { self.0 } }
    pub fn now_monotonic() -> Result<Stamp,crate::error::Error> { Ok(Stamp(crate::NOW.load(crate::Ordering::SeqCst))) }
}
pub mod io {
    use super::*;
    pub mod poll_events { pub const POLLIN: i16=1; }
    pub struct PollFd { pub fd: i32, pub events: i16, pub revents: i16 }
    pub fn read(_: types::Fd, bytes: &mut [u8]) -> Result<usize,error::Error> {
        let mut generation=0u64;
        let n=FILE.lock().unwrap().as_ref().unwrap().read(&mut generation as *mut u64 as u64,8).unwrap();
        bytes.copy_from_slice(&generation.to_ne_bytes()); Ok(n as usize)
    }
    pub fn poll_ns(_: &mut [PollFd], left: Option<u64>) -> Result<usize,error::Error> {
        POLLS.fetch_add(1,Ordering::SeqCst);
        let queue=FILE.lock().unwrap().as_ref().unwrap().poll_queue().unwrap();
        let wakes=queue.0.load(Ordering::SeqCst);
        if !BEFORE_POLL.load(Ordering::SeqCst) {
            assert_eq!(FILE.lock().unwrap().as_ref().unwrap().poll_events(),0);
            ops::lose_claim(); // Forced pending-poll transition, no composite or host timing.
            if queue.0.load(Ordering::SeqCst)==wakes { NOW.fetch_add(left.unwrap(),Ordering::SeqCst); return Ok(0); }
        }
        if FILE.lock().unwrap().as_ref().unwrap().poll_events()==0 {
            NOW.fetch_add(left.unwrap(),Ordering::SeqCst); return Ok(0);
        }
        NOW.fetch_add(10,Ordering::SeqCst); Ok(1)
    }
}
pub mod window {
    use super::*;
    pub mod abi {
        pub mod op { pub const PRESENTATION_FD: u32=36; pub const SCANOUT_READBACK: u32=22; }
        pub struct WinId { pub id: u64 }
        #[derive(Default)] pub struct Readback { pub serial: u64 }
    }
    pub fn call<T>(op: u32, _: &mut T) -> Result<u64,error::Error> {
        match op {
            abi::op::PRESENTATION_FD => { *FILE.lock().unwrap()=Some(events::PresentationFile::new().unwrap()); Ok(7) }
            abi::op::SCANOUT_READBACK => {
                CALLS.fetch_add(1,Ordering::SeqCst);
                match observe() {
                    Ok(serial) => Ok(serial),
                    Err(11) => {
                        if BEFORE_POLL.load(Ordering::SeqCst) { ops::lose_claim(); }
                        Err(error::Error::Os(errno::Errno::EAGAIN))
                    }
                    Err(e) => panic!("unexpected gate error: {e}"),
                }
            }
            _ => panic!("unexpected window op"),
        }
    }
    @HELPER@
}
#[test]
fn claim_loss_wakes_and_retries_before_the_original_deadline() {
    for before_poll in [false,true] {
        for windows in [0,1] {
            { let mut reg=REGISTRY.lock(); reg.claim=Some(Held { first_serial: 7 });
              reg.windows.clear(); if windows==1 { reg.windows.insert(1,Window {
                ring: Ring(vec![]), waits: Arc::new(Waits { events: WaitQueueHead::new() }) }); } }
            assert_eq!(observe(),Err(11),"claim has not completed its first composite");
            NOW.store(0,Ordering::SeqCst); CALLS.store(0,Ordering::SeqCst);
            POLLS.store(0,Ordering::SeqCst); CLOSED.store(0,Ordering::SeqCst);
            BEFORE_POLL.store(before_poll,Ordering::SeqCst);
            assert_eq!(window::readback_until(&mut window::abi::Readback::default(),100),Ok(()),
                "pending read slept to ETIMEDOUT after claim loss; before_poll={before_poll} windows={windows}");
            assert_eq!(CALLS.load(Ordering::SeqCst),2);
            assert_eq!(POLLS.load(Ordering::SeqCst),1);
            assert_eq!(CLOSED.load(Ordering::SeqCst),1);
            assert_eq!(NOW.load(Ordering::SeqCst),10);
            assert_eq!(REGISTRY.lock().present.serial,7,"no composite completed to wake this read");
        }
    }
}
'''
        code = (code.replace('@DESCRIPTOR@', events[events.index('static COMPLETIONS:'):])
                .replace('@END@', item(ops, 'fn end_claim('))
                .replace('@WAITS@', item(ops, 'fn all_waits('))
                .replace('@WAKE@', item(ops, 'fn claim_ended('))
                .replace('@GATE@', gate).replace('@HELPER@', helper))
        run_rust(self, code)


if __name__ == '__main__':
    unittest.main()
