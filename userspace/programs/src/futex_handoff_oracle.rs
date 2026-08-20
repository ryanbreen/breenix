//! Deterministic userspace driver for the #584 futex handoff oracle.
//!
//! `init` launches this program on every aarch64 image, but the kernel seam that
//! drives the handoff races is `boot_tests`-only. On a production kernel `val3`
//! carries no meaning and no thread in the system would ever wake these waits,
//! so the driver has to be self-limiting or it wedges `init` before `bsshd`.
//!
//! Stage 0 is the arming handshake. An armed kernel answers the probe sentinel
//! with `PROBE_ACK`, a success value no ordinary `FUTEX_WAIT` can return. An
//! unarmed kernel ignores `val3`, sees a matching word, honours the probe's
//! timeout and returns `-ETIMEDOUT`. Anything other than the ack makes the
//! driver print a skip marker and exit 0 without ever blocking.
//!
//! The blocking stages additionally carry their own timeout, longer than the
//! kernel oracle's 1 s backstop so an armed run's verdict is still decided by
//! that backstop, exactly as before. That bound only ever takes effect if the
//! handshake sentinel and the seam ever drift apart.

use libbreenix::memory;
use libbreenix::process;
use libbreenix::syscall::{nr, raw};
use libbreenix::types::Timespec;

const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;
const PROBE: u64 = 0x4655_5850;
const STAGE1: u64 = 0x4655_5831;
const STAGE2: u64 = 0x4655_5832;
const STAGE3: u64 = 0x4655_5833;
const REPORT: u64 = 0x4655_5852;

/// Success value an armed kernel returns from the stage-0 probe.
const PROBE_ACK: i64 = 0x4655_5850;
/// Word value the probe waits on. Only this program touches the page, so an
/// unarmed kernel always finds a match and always reaches its timeout.
const PROBE_VALUE: u32 = 0x5850;
/// Probe timeout on an unarmed kernel: the whole cost of the seam-absent path.
const PROBE_TIMEOUT_NS: i64 = 20_000_000;
/// Self-limiting bound for the two handoff stages, in seconds. Must stay above
/// the kernel oracle's 1 s backstop so the backstop, not this bound, ends an
/// armed run's wait and the RESCUED regression signature is preserved.
const STAGE_BACKSTOP_SECS: i64 = 5;

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
    let probe_word = page as *mut u32;
    let word0 = unsafe { page.add(4) as *mut u32 };
    let word1 = unsafe { page.add(8) as *mut u32 };
    let word2 = unsafe { page.add(12) as *mut u32 };

    unsafe {
        // Stage 0: arming handshake. It must precede every blocking stage, and
        // anything but the ack must leave before another wait is issued.
        let probe_timeout = Timespec {
            tv_sec: 0,
            tv_nsec: PROBE_TIMEOUT_NS,
        };
        core::ptr::write_volatile(probe_word, PROBE_VALUE);
        let probe = futex(
            probe_word,
            FUTEX_WAIT,
            PROBE_VALUE as u64,
            &probe_timeout as *const Timespec as u64,
            PROBE,
        );
        if probe != PROBE_ACK {
            println!("[FUTEX_HANDOFF_ORACLE_DRIVER:seam_absent:probe={}]", probe);
            process::exit(0);
        }

        let backstop = Timespec {
            tv_sec: STAGE_BACKSTOP_SECS,
            tv_nsec: 0,
        };
        let backstop_ptr = &backstop as *const Timespec as u64;

        core::ptr::write_volatile(word0, 42);
        let stage1 = futex(word0, FUTEX_WAIT, 42, backstop_ptr, STAGE1);

        core::ptr::write_volatile(word1, 7);
        let stage2 = futex(word1, FUTEX_WAIT, 7, backstop_ptr, STAGE2);

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
