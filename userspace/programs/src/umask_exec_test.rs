//! Fork/exec and spawn/clone credential target for the directories suite.
use libbreenix::{memory, process, syscall::{nr, raw}, time};
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(target_arch = "x86_64")]
const IDS: [u64; 4] = [102, 104, 107, 108];
#[cfg(target_arch = "aarch64")]
const IDS: [u64; 4] = [174, 176, 175, 177];
static RESULT: AtomicU32 = AtomicU32::new(0);
static TID: AtomicU32 = AtomicU32::new(0);

fn valid(uid: u64) -> bool {
    let mut groups = [0u32; 2];
    // SAFETY: the array is writable through the syscall; ID queries take no pointers.
    unsafe {
        raw::syscall1(nr::UMASK, 0o027) == 0o027
            && raw::syscall2(nr::GETGROUPS, 2, groups.as_mut_ptr() as u64) == 2
            && groups == [1001, 2345]
            && IDS.iter().all(|nr| raw::syscall0(*nr) == uid)
    }
}

fn denied() -> bool {
    let root = b"/\0";
    // SAFETY: the root pathname is NUL-terminated through the ownership request.
    unsafe {
        raw::syscall5(nr::FCHOWNAT, (-100i64) as u64, root.as_ptr() as u64,
            1001, u32::MAX as u64, 0) as i64 == -1
            && raw::syscall2(nr::SETGROUPS, 0, 0) as i64 == -1
    }
}

extern "C" fn child(_arg: *mut u8) -> *mut u8 {
    RESULT.store(if valid(1001) && denied() { 1 } else { 2 }, Ordering::Release);
    // SAFETY: exit terminates only this clone thread; it never returns.
    unsafe { raw::syscall1(nr::EXIT, 0); }
    loop { core::hint::spin_loop(); }
}

fn main() {
    let unprivileged = std::env::args().nth(1).as_deref() == Some("unprivileged");
    if !valid(if unprivileged { 1001 } else { 0 }) {
        process::exit(1);
    }
    if unprivileged {
        if !denied() { process::exit(2); }
        let stack = match memory::mmap(core::ptr::null_mut(), 65536, 3, 0x22, -1, 0) {
            Ok(stack) => stack,
            Err(_) => process::exit(3),
        };
        // SAFETY: stack is mapped for 64 KiB, callback uses a shared atomic result,
        // and clear-child-tid remains live until this thread exits.
        let ret = unsafe { raw::syscall5(nr::CLONE, 0x100 | 0x400 | 0x200000 | 0x1000000,
            ((stack as usize + 65536) & !15) as u64, child as *const () as u64, 0,
            TID.as_ptr() as u64) } as i64;
        if ret < 0 { process::exit(4); }
        let start = time::now_monotonic().unwrap().tv_sec;
        while TID.load(Ordering::Acquire) != 0 {
            if time::now_monotonic().unwrap().tv_sec - start > 3 {
                process::exit(5);
            }
            let _ = process::yield_now();
        }
        if RESULT.load(Ordering::Acquire) != 1 { process::exit(6); }
    }
    process::exit(0);
}
