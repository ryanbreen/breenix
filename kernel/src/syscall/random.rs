//! getrandom syscall implementation
//!
//! Provides random bytes to userspace using a TSC-seeded xorshift64* PRNG.
//! This is adequate for a single-user OS running under QEMU. Not
//! cryptographically secure, but sufficient for HashMap seeding and
//! general-purpose randomness.

use super::{ErrorCode, SyscallResult};

/// Read the x86_64 Time Stamp Counter (TSC).
/// Returns a high-resolution, monotonically increasing value.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn read_tsc() -> u64 {
    unsafe {
        core::arch::x86_64::_rdtsc()
    }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn read_tsc() -> u64 {
    let val: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) val);
    }
    val
}

/// xorshift64* PRNG - fast, decent quality for non-crypto use
struct Xorshift64Star {
    state: u64,
}

impl Xorshift64Star {
    fn new(seed: u64) -> Self {
        // Ensure non-zero state
        Self { state: if seed == 0 { 0xdeadbeefcafe1234 } else { seed } }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
}

/// sys_getrandom - fill a userspace buffer with random bytes
///
/// Arguments:
///   buf_ptr: userspace buffer address
///   buflen:  number of bytes to fill
///   flags:   GRND_RANDOM (1), GRND_NONBLOCK (2), GRND_INSECURE (4)
///            (all flags accepted but treated identically)
///
/// Returns: number of bytes written on success, or negative errno
pub fn sys_getrandom(buf_ptr: u64, buflen: u64, _flags: u32) -> SyscallResult {
    if buflen == 0 {
        return SyscallResult::Ok(0);
    }

    if buf_ptr == 0 {
        return SyscallResult::Err(ErrorCode::Fault as u64);
    }

    let len = buflen as usize;

    // Seed from TSC - each call gets a different seed
    let seed = read_tsc();
    let mut rng = Xorshift64Star::new(seed);

    // Write random bytes directly to userspace buffer
    // We write 8 bytes at a time for efficiency, then handle remainder
    let buf = buf_ptr as *mut u8;

    // Validate the userspace pointer range
    if let Err(_) = crate::syscall::userptr::validate_user_ptr_write(buf) {
        return SyscallResult::Err(ErrorCode::Fault as u64);
    }

    unsafe {
        let mut offset = 0usize;

        // Write 8 bytes at a time
        while offset + 8 <= len {
            let val = rng.next_u64();
            core::ptr::write_volatile(buf.add(offset) as *mut u64, val);
            offset += 8;
        }

        // Write remaining bytes
        if offset < len {
            let val = rng.next_u64();
            let bytes = val.to_le_bytes();
            for i in 0..(len - offset) {
                core::ptr::write_volatile(buf.add(offset + i), bytes[i]);
            }
        }
    }

    SyscallResult::Ok(buflen)
}
