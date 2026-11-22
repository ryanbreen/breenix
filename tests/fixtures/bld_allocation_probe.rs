// Host-only allocator adapter for the same bld::run entry point as the shipped CLI.
use std::alloc::{GlobalAlloc, Layout};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
extern crate self as libbreenix;

#[allow(dead_code)] // The probe uses only the C entry points Rust's allocator needs.
mod stdlib {
    pub(crate) use crate::{errno, set_errno};

    pub const EINVAL: i32 = 22;
    pub const ENOMEM: i32 = 12;

    pub mod alloc_cache;
    pub mod malloc;
}

const MAP_PRIVATE: i32 = 0x02;
#[cfg(target_os = "macos")]
const MAP_ANONYMOUS: i32 = 0x1000;
#[cfg(target_os = "linux")]
const MAP_ANONYMOUS: i32 = 0x20;
const PROT_READ: i32 = 0x01;
const PROT_WRITE: i32 = 0x02;
const MAP_FAILED: *mut u8 = usize::MAX as *mut u8;

mod native {
    unsafe extern "C" {
        pub fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64)
            -> *mut u8;
        pub fn munmap(addr: *mut u8, len: usize) -> i32;
        pub fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
        pub fn getpid() -> i32;
    }
}

pub mod syscall {
    pub mod nr {
        pub const MADVISE: u64 = 0;
    }
}

pub mod raw {
    pub unsafe fn syscall3(number: u64, addr: u64, len: u64, advice: u64) -> u64 {
        assert_eq!(number, crate::syscall::nr::MADVISE);
        crate::native::madvise(addr as *mut u8, len as usize, advice as i32) as u64
    }
}

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static MAPS: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

thread_local!(static ERRNO: std::cell::Cell<i32> = const { std::cell::Cell::new(0) });
fn set_errno(value: i32) {
    ERRNO.with(|errno| errno.set(value));
}
fn errno() -> i32 {
    ERRNO.with(|errno| errno.get())
}
fn getpid() -> i32 {
    unsafe { native::getpid() }
}

unsafe fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8 {
    MAPS.fetch_add(1, Relaxed);
    let base = native::mmap(addr, len, prot, flags, fd, off);
    if base != MAP_FAILED {
        PEAK.fetch_max(LIVE.fetch_add(1, Relaxed) + 1, Relaxed);
    }
    base
}

unsafe fn munmap(addr: *mut u8, len: usize) -> i32 {
    let result = native::munmap(addr, len);
    if result == 0 {
        LIVE.fetch_sub(1, Relaxed);
    }
    result
}

mod string {
    pub unsafe fn memset(dst: *mut u8, byte: i32, len: usize) {
        dst.write_bytes(byte as u8, len);
    }
    pub unsafe fn memcpy(dst: *mut u8, src: *const u8, len: usize) {
        std::ptr::copy_nonoverlapping(src, dst, len);
    }
}

struct BreenixAllocator;

#[global_allocator]
static ALLOCATOR: BreenixAllocator = BreenixAllocator;

unsafe fn allocate(size: usize, alignment: usize) -> *mut u8 {
    use stdlib::malloc::{malloc, posix_memalign};
    if alignment <= 16 {
        return malloc(size);
    }
    let mut block = ptr::null_mut();
    posix_memalign(&mut block, alignment, size);
    block
}

unsafe impl GlobalAlloc for BreenixAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        allocate(layout.size(), layout.align())
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        if layout.align() <= 16 {
            return stdlib::malloc::calloc(1, layout.size());
        }
        let block = allocate(layout.size(), layout.align());
        if !block.is_null() {
            block.write_bytes(0, layout.size());
        }
        block
    }

    unsafe fn dealloc(&self, block: *mut u8, _: Layout) {
        stdlib::malloc::free(block);
    }

    unsafe fn realloc(&self, block: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        if layout.align() <= 16 {
            return stdlib::malloc::realloc(block, size);
        }
        let new = allocate(size, layout.align());
        if !new.is_null() {
            ptr::copy_nonoverlapping(block, new, size.min(layout.size()));
            stdlib::malloc::free(block);
        }
        new
    }
}

fn main() -> std::process::ExitCode {
    let status = bld::run(std::env::args_os().skip(1));
    eprintln!(
        "{} {} {}",
        ALLOCS.load(Relaxed),
        MAPS.load(Relaxed),
        PEAK.load(Relaxed),
    );
    status
}
