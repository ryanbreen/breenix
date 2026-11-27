use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::Relaxed};
use std::sync::{Condvar, Mutex};
extern crate self as libbreenix;

pub mod syscall {
    pub mod nr {
        pub const MADVISE: u64 = 0;
        pub const FUTEX: u64 = 1;
    }
}

static SYSCALLS: AtomicUsize = AtomicUsize::new(0);
static CLEARS: AtomicUsize = AtomicUsize::new(0);
static FUTEX_LOCK: Mutex<()> = Mutex::new(());
static FUTEX_READY: Condvar = Condvar::new();
static CONTENDED: Mutex<Option<std::sync::mpsc::SyncSender<bool>>> = Mutex::new(None);

fn report_contended(waited: bool) {
    if let Some(sender) = CONTENDED.lock().unwrap().take() {
        sender.send(waited).unwrap();
    }
}

pub mod raw {
    use super::*;

    pub unsafe fn syscall6(number: u64, addr: u64, op: u64, val: u64, timeout: u64, _: u64, _: u64) -> u64 {
        assert_eq!((number, timeout), (crate::syscall::nr::FUTEX, 0));
        SYSCALLS.fetch_add(1, Relaxed);
        let word = &*(addr as *const AtomicU32);
        let mut guard = FUTEX_LOCK.lock().unwrap();
        match op & !128 {
            0 => {
                report_contended(true);
                while word.load(Relaxed) == val as u32 {
                    guard = FUTEX_READY.wait(guard).unwrap();
                }
            }
            1 => FUTEX_READY.notify_all(),
            _ => panic!("unexpected futex op {op}"),
        }
        0
    }

    pub unsafe fn syscall3(number: u64, addr: u64, len: u64, advice: u64) -> u64 {
        SYSCALLS.fetch_add(1, Relaxed);
        assert_eq!((number, advice), (crate::syscall::nr::MADVISE, 4));
        assert_eq!((addr % 4096, len % 4096), (0, 0));
        let mappings = crate::MAPPINGS.lock().unwrap();
        assert!(mappings.iter().any(|&(base, requested)| {
            addr as usize >= base
                && (addr + len) as usize <= base + requested.next_multiple_of(4096)
        }));
        crate::DISCARDED
            .lock()
            .unwrap()
            .push((addr as usize, len as usize));
        (addr as *mut u8).write_bytes(0, len as usize);
        0
    }
}

thread_local!(static ERRNO: std::cell::Cell<i32> = const { std::cell::Cell::new(0) });
fn set_errno(e: i32) {
    ERRNO.with(|v| v.set(e));
}
fn errno() -> i32 {
    ERRNO.with(|v| v.get())
}
fn getpid() -> i32 {
    SYSCALLS.fetch_add(1, Relaxed);
    std::process::id() as i32
}
#[allow(dead_code)] // Also drives the old allocator in the check-first run.
fn malloc_thread_id() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    thread_local! { static ID: u32 = NEXT.fetch_add(1, Relaxed); }
    ID.with(|id| *id)
}
const MAP_ANONYMOUS: i32 = 1;
const MAP_PRIVATE: i32 = 2;
const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_FAILED: *mut u8 = usize::MAX as *mut u8;
static MAPPINGS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
static DISCARDED: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
unsafe fn mmap(_: *mut u8, len: usize, _: i32, _: i32, _: i32, _: i64) -> *mut u8 {
    report_contended(false);
    SYSCALLS.fetch_add(1, Relaxed);
    let layout = std::alloc::Layout::from_size_align(len.next_multiple_of(4096), 4096).unwrap();
    let p = std::alloc::alloc_zeroed(layout);
    if p.is_null() {
        return MAP_FAILED;
    }
    MAPPINGS.lock().unwrap().push((p as usize, len));
    p
}
unsafe fn munmap(p: *mut u8, len: usize) -> i32 {
    SYSCALLS.fetch_add(1, Relaxed);
    let mut maps = MAPPINGS.lock().unwrap();
    let i = maps
        .iter()
        .position(|&(base, _)| base == p as usize)
        .unwrap();
    let (_, expected) = maps.remove(i);
    assert_eq!(len, expected, "free must release the full mapping");
    let layout = std::alloc::Layout::from_size_align(len.next_multiple_of(4096), 4096).unwrap();
    std::alloc::dealloc(p, layout);
    0
}
mod allocation_string {
    pub unsafe fn memset(p: *mut u8, c: i32, n: usize) {
        crate::CLEARS.fetch_add(n, crate::Relaxed);
        p.write_bytes(c as u8, n);
    }
    pub unsafe fn memcpy(d: *mut u8, s: *const u8, n: usize) {
        std::ptr::copy_nonoverlapping(s, d, n);
    }
}
