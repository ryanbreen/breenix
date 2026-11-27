//! What a TCP connection is told to start from: its initial sequence number
//! (RFC 6528) and, for a socket that did not bind one, its local port (RFC 6056).
//!
//! Pure computation, so the host tests compile this file as is.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::random::blake2s::Blake2s;

/// Bytes in the ISN key, drawn once from the kernel's random pool.
pub const ISN_SECRET_LEN: usize = 32;

/// RFC 6528 section 3: ISN = M + F(localip, localport, remoteip, remoteport,
/// secretkey). M is a timer that ticks once per 4 microseconds, so one 4-tuple's
/// ISNs advance with time; F is a keyed pseudorandom function, here BLAKE2s
/// keyed with the boot-time secret (RFC 7693 section 2.5), so the ISNs of
/// different 4-tuples are unrelated: one connection's ISN does not predict
/// another's without the key.
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

/// The kernel's random pool, as connection setup draws from it.
pub trait Pool {
    /// Fill `out` from the pool, whether or not it is ready, and return
    /// whether it was ready before the draw: credited with the entropy it
    /// needs to be unpredictable (`random::is_ready`, which once true stays
    /// true).
    fn fill(&self, out: &mut [u8]) -> bool;
}

/// `IsnKey` states.
const KEY_UNSET: u8 = 0;
const KEY_WRITING: u8 = 1;
const KEY_SET: u8 = 2;

/// The ISN key (RFC 6528 section 3: a secret the attacker cannot guess). It is
/// drawn the first time a connection needs one once the pool is ready, and no
/// context ever waits for it:
/// - Until the pool is ready there is no key: each ISN's offset is a draw of
///   its own, so nothing taken from an unready pool outlives one connection.
/// - The context that claims the key draws and publishes it. A context that
///   asks meanwhile, including a handshake that interrupted the claimer on its
///   own CPU, takes a draw of its own instead.
pub struct IsnKey {
    state: AtomicU8,
    key: UnsafeCell<[u8; ISN_SECRET_LEN]>,
}

// SAFETY: `key` is written only by the one context that moved `state` from
// KEY_UNSET to KEY_WRITING, and read only after an Acquire load sees KEY_SET,
// which that context stores with Release once the key is written.
unsafe impl Sync for IsnKey {}

impl IsnKey {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(KEY_UNSET),
            key: UnsafeCell::new([0; ISN_SECRET_LEN]),
        }
    }

    /// The ISN for a connection on this 4-tuple at `now_us`: `isn` under the
    /// key once it is set, and before that the 4-microsecond clock plus a
    /// fresh draw.
    pub fn isn(
        &self,
        pool: &impl Pool,
        local_ip: [u8; 4],
        local_port: u16,
        remote_ip: [u8; 4],
        remote_port: u16,
        now_us: u64,
    ) -> u32 {
        match self.key(pool) {
            Ok(secret) => isn(secret, local_ip, local_port, remote_ip, remote_port, now_us),
            Err(offset) => ((now_us / 4) as u32).wrapping_add(offset),
        }
    }

    /// The key, or a fresh offset from the pool when there is none to use yet.
    fn key(&self, pool: &impl Pool) -> Result<&[u8; ISN_SECRET_LEN], u32> {
        if self.state.load(Ordering::Acquire) == KEY_SET {
            // SAFETY: KEY_SET is stored after the key is written, and it is
            // not written again.
            return Ok(unsafe { &*self.key.get() });
        }
        let mut offset = [0u8; 4];
        let ready = pool.fill(&mut offset);
        if ready
            && self
                .state
                .compare_exchange(KEY_UNSET, KEY_WRITING, Ordering::Acquire, Ordering::Acquire)
                .is_ok()
        {
            // SAFETY: this context holds KEY_WRITING, so no other writes the
            // key or reads it until KEY_SET. The pool was ready before the
            // offset's draw and stays ready, so the key is drawn from it ready.
            pool.fill(unsafe { &mut *self.key.get() });
            self.state.store(KEY_SET, Ordering::Release);
            // SAFETY: as above; this context has just set it.
            return Ok(unsafe { &*self.key.get() });
        }
        if self.state.load(Ordering::Acquire) == KEY_SET {
            // SAFETY: as above.
            return Ok(unsafe { &*self.key.get() });
        }
        Err(u32::from_le_bytes(offset))
    }
}

/// Bind a socket that has no port to the first free one of a search that
/// starts at a fresh draw from `pool` (`ephemeral_candidates`). `bind(port)`
/// takes the port: true if it did, false if another socket holds it. The draw
/// is used for this search only and is not kept, and the search does not wait
/// for the pool to be ready: port randomization (RFC 6056 section 1) makes a
/// port harder to guess, and a connection must not block on it.
/// `Ok(None)` when the range has no free port; `bind`'s error otherwise.
pub fn bind_ephemeral(
    pool: &impl Pool,
    mut bind: impl FnMut(u16) -> Result<bool, i32>,
) -> Result<Option<u16>, i32> {
    let mut draw = [0u8; 8];
    pool.fill(&mut draw);
    for candidate in ephemeral_candidates(u64::from_le_bytes(draw)) {
        if bind(candidate)? {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
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
