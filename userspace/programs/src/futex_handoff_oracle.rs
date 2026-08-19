//! Deterministic userspace driver for the #584 futex handoff oracle.

use libbreenix::memory;
use libbreenix::process;
use libbreenix::syscall::{nr, raw};
use libbreenix::types::Timespec;

const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;
const STAGE1: u64 = 0x4655_5831;
const STAGE2: u64 = 0x4655_5832;
const STAGE3: u64 = 0x4655_5833;
const REPORT: u64 = 0x4655_5852;

unsafe fn map_region() -> *mut u8 {
    match memory::mmap(core::ptr::null_mut(), 4096, 3, 0x22, -1, 0) {
        Ok(mapped) => mapped,
        Err(error) => {
            println!("[FUTEX_HANDOFF_ORACLE_DRIVER:mmap_error={} ]", error);
            process::exit(1);
        }
    }
}

unsafe fn futex(
    address: *mut u32,
    operation: u64,
    value: u64,
    timeout: u64,
    val3: u64,
) -> i64 {
    raw::syscall6(
        nr::FUTEX,
        address as u64,
        operation,
        value,
        timeout,
        0,
        val3,
    ) as i64
}

fn main() {
    let page = unsafe { map_region() };
    let word0 = page as *mut u32;
    let word1 = unsafe { page.add(4) as *mut u32 };
    let word2 = unsafe { page.add(8) as *mut u32 };

    unsafe {
        core::ptr::write_volatile(word0, 42);
        let stage1 = futex(word0, FUTEX_WAIT, 42, 0, STAGE1);

        core::ptr::write_volatile(word1, 7);
        let stage2 = futex(word1, FUTEX_WAIT, 7, 0, STAGE2);

        core::ptr::write_volatile(word2, 9);
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: 50_000_000,
        };
        let stage3 = futex(word2, FUTEX_WAIT, 9, &timeout as *const Timespec as u64, STAGE3);

        let _report = futex(word0, FUTEX_WAKE, 0, 0, REPORT);
        println!(
            "[FUTEX_HANDOFF_ORACLE_DRIVER:s1={}:s2={}:s3={}]",
            stage1, stage2, stage3
        );
    }

    process::exit(0);
}
