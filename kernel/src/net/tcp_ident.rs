//! What a TCP connection is told to start from: its initial sequence number
//! (RFC 6528) and, for a socket that never bound one, its local port (RFC 6056).
//!
//! Pure computation, so the host tests compile this file as is.

use crate::random::blake2s::Blake2s;

/// Bytes in the ISN key, drawn once from the kernel's random pool.
pub const ISN_SECRET_LEN: usize = 32;

/// RFC 6528 section 3: ISN = M + F(localip, localport, remoteip, remoteport,
/// secretkey). M is a timer that ticks every 4 microseconds, so one 4-tuple's
/// ISNs advance with time; F is a keyed pseudorandom function, here BLAKE2s
/// keyed with the boot-time secret (RFC 7693 section 2.5), so the ISNs of
/// different 4-tuples are unrelated and an observer of one connection learns
/// nothing about another's.
pub fn isn(
    secret: &[u8; ISN_SECRET_LEN],
    local_ip: [u8; 4],
    local_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    now_us: u64,
) -> u32 {
    let mut tuple = [0u8; 12];
    tuple[..4].copy_from_slice(&local_ip);
    tuple[4..6].copy_from_slice(&local_port.to_be_bytes());
    tuple[6..10].copy_from_slice(&remote_ip);
    tuple[10..].copy_from_slice(&remote_port.to_be_bytes());
    let mut hash = Blake2s::new(4, secret);
    hash.update(&tuple);
    let mut f = [0u8; 4];
    hash.finalize(&mut f);
    // M wraps with the 32-bit sequence space.
    let m = (now_us / 4) as u32;
    m.wrapping_add(u32::from_le_bytes(f))
}

/// The ephemeral range, 32768-60999, unchanged by the move to random selection.
pub const EPHEMERAL_FIRST: u16 = 32768;
pub const EPHEMERAL_LAST: u16 = 60999;

/// RFC 6056 section 3.3.1, Algorithm 1 ("Simple Port Randomization"): start
/// at a random port in the range and step by one, wrapping, through the whole
/// range. The caller takes the first port that is free. `random` is a fresh
/// value from the kernel's random pool for each search.
pub fn ephemeral_candidates(random: u64) -> impl Iterator<Item = u16> {
    let span = u32::from(EPHEMERAL_LAST - EPHEMERAL_FIRST) + 1;
    let start = (random % u64::from(span)) as u32;
    (0..span).map(move |n| EPHEMERAL_FIRST + ((start + n) % span) as u16)
}
