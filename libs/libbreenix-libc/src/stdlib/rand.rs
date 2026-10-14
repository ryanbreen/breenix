//! Pseudo-random numbers: rand, srand and rand_r (ISO C17 7.22.2, POSIX.1-2024),
//! random and srandom (XSI), and the drand48 family, whose generator POSIX defines
//! exactly: X(n+1) = (a X(n) + c) mod 2^48.
//!
//! RAND_MAX is 2^31 - 1. rand and random use a 64-bit linear congruential generator with Knuth's MMIX
//! constants (TAOCP vol. 2, 3.3.4) and return its top 31 bits. rand_r has only the
//! caller's 32 bits of state, so it uses O'Neill's PCG-RXS-M-XS-32 output permutation
//! ("PCG: A Family of Simple Fast Space-Efficient Statistically Good Algorithms for
//! Random Number Generation", 2014) on a 32-bit LCG step.

use core::sync::atomic::{AtomicU64, Ordering};

/// Calling rand before srand behaves as if srand(1) had been called.
static RAND_STATE: AtomicU64 = AtomicU64::new(1);
static RANDOM_STATE: AtomicU64 = AtomicU64::new(1);

fn step64(x: u64) -> u64 {
    x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)
}

fn next31(state: &AtomicU64) -> i32 {
    let mut old = state.load(Ordering::Relaxed);
    loop {
        let new = step64(old);
        match state.compare_exchange_weak(old, new, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return (new >> 33) as i32,
            Err(seen) => old = seen,
        }
    }
}

#[no_mangle]
pub extern "C" fn rand() -> i32 {
    next31(&RAND_STATE)
}

#[no_mangle]
pub extern "C" fn srand(seed: u32) {
    RAND_STATE.store(seed as u64, Ordering::Relaxed);
}

#[no_mangle]
pub extern "C" fn random() -> i64 {
    next31(&RANDOM_STATE) as i64
}

#[no_mangle]
pub extern "C" fn srandom(seed: u32) {
    RANDOM_STATE.store(seed as u64, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn rand_r(seed: *mut u32) -> i32 {
    let x = (*seed).wrapping_mul(747796405).wrapping_add(2891336453);
    *seed = x;
    let word = ((x >> ((x >> 28) + 4)) ^ x).wrapping_mul(277803737);
    (((word >> 22) ^ word) >> 1) as i32
}

// ----------------------------------------------------------------------------
// drand48 and relatives: 48-bit state as three 16-bit words, least significant first.
// ----------------------------------------------------------------------------

const MASK48: u64 = (1 << 48) - 1;
const A_DEFAULT: u64 = 0x5_DEEC_E66D;
const C_DEFAULT: u16 = 0xB;

struct Lcg48 {
    x: [u16; 3],
    a: [u16; 3],
    c: u16,
    seed_out: [u16; 3],
}

static mut LCG48: Lcg48 = Lcg48 {
    x: [0x330E, 0, 0],
    a: [A_DEFAULT as u16, (A_DEFAULT >> 16) as u16, (A_DEFAULT >> 32) as u16],
    c: C_DEFAULT,
    seed_out: [0; 3],
};

fn join(w: &[u16; 3]) -> u64 {
    w[0] as u64 | (w[1] as u64) << 16 | (w[2] as u64) << 32
}

fn split(v: u64) -> [u16; 3] {
    [v as u16, (v >> 16) as u16, (v >> 32) as u16]
}

/// Advances xsubi and returns the new 48-bit value.
unsafe fn next48(xsubi: *mut u16) -> u64 {
    let g = &*core::ptr::addr_of!(LCG48);
    let x = join(&*(xsubi as *const [u16; 3]));
    let v = (join(&g.a).wrapping_mul(x).wrapping_add(g.c as u64)) & MASK48;
    *(xsubi as *mut [u16; 3]) = split(v);
    v
}

unsafe fn state() -> *mut u16 {
    core::ptr::addr_of_mut!(LCG48.x) as *mut u16
}

#[no_mangle]
pub unsafe extern "C" fn erand48(xsubi: *mut u16) -> f64 {
    next48(xsubi) as f64 / (1u64 << 48) as f64
}

#[no_mangle]
pub unsafe extern "C" fn drand48() -> f64 {
    erand48(state())
}

#[no_mangle]
pub unsafe extern "C" fn nrand48(xsubi: *mut u16) -> i64 {
    (next48(xsubi) >> 17) as i64
}

#[no_mangle]
pub unsafe extern "C" fn lrand48() -> i64 {
    nrand48(state())
}

#[no_mangle]
pub unsafe extern "C" fn jrand48(xsubi: *mut u16) -> i64 {
    (next48(xsubi) >> 16) as u32 as i32 as i64
}

#[no_mangle]
pub unsafe extern "C" fn mrand48() -> i64 {
    jrand48(state())
}

unsafe fn reset_multiplier() {
    let g = &mut *core::ptr::addr_of_mut!(LCG48);
    g.a = split(A_DEFAULT);
    g.c = C_DEFAULT;
}

#[no_mangle]
pub unsafe extern "C" fn srand48(seed: i64) {
    reset_multiplier();
    (*core::ptr::addr_of_mut!(LCG48)).x = [0x330E, seed as u16, (seed >> 16) as u16];
}

#[no_mangle]
pub unsafe extern "C" fn seed48(seed16v: *mut u16) -> *mut u16 {
    reset_multiplier();
    let g = &mut *core::ptr::addr_of_mut!(LCG48);
    g.seed_out = g.x;
    g.x = *(seed16v as *const [u16; 3]);
    g.seed_out.as_mut_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn lcong48(param: *mut u16) {
    let p = &*(param as *const [u16; 7]);
    let g = &mut *core::ptr::addr_of_mut!(LCG48);
    g.x = [p[0], p[1], p[2]];
    g.a = [p[3], p[4], p[5]];
    g.c = p[6];
}
