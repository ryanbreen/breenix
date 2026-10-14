//! ISO C17 and POSIX.1-2024 pseudo-random generators.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::cell::UnsafeCell;

struct Locked<T> { busy: AtomicBool, data: UnsafeCell<T> }
unsafe impl<T: Send> Sync for Locked<T> {}
impl<T> Locked<T> {
    const fn new(data: T) -> Self { Self { busy: AtomicBool::new(false), data: UnsafeCell::new(data) } }
    fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        while self.busy.swap(true, Ordering::Acquire) { core::hint::spin_loop(); }
        let r = f(unsafe { &mut *self.data.get() });
        self.busy.store(false, Ordering::Release);
        r
    }
}

/// Calling rand before srand behaves as if srand(1) had been called.
static RAND_STATE: AtomicU64 = AtomicU64::new(1);
// POSIX requires a nonlinear additive-feedback generator with a default 31-long
// state. We choose the recurrence X[n] = X[n-31] + X[n-3] modulo 2^32,
// discarding its low bit (the discarded carry makes the output nonlinear).
// The taps and initialization below are implementation choices, not libc data.
struct Feedback { words: [u32; 31], index: usize, initialized: bool }
static RANDOM_STATE: Locked<Feedback> = Locked::new(Feedback {
    words: [0; 31], index: 0, initialized: false,
});
impl Feedback {
    fn next(&mut self) -> i64 {
        let j = (self.index + 31 - 3) % 31;
        let x = self.words[self.index].wrapping_add(self.words[j]);
        self.words[self.index] = x;
        self.index = (self.index + 1) % 31;
        (x >> 1) as i64
    }
    fn seed(&mut self, seed: u32) {
        let mut x = seed as u64;
        for w in &mut self.words { x = step64(x); *w = (x >> 16) as u32; }
        self.index = 0;
        self.initialized = true;
        // Ten complete passes mix every seeded word into the recurrence.
        for _ in 0..310 { self.next(); }
    }
}

fn step64(x: u64) -> u64 {
    // ISO C leaves rand's algorithm to the implementation. Choose multiplier
    // 2^32+1 (1 modulo 4) and odd increment 1 for a full-period modulo-2^64 LCG.
    x.wrapping_mul((1u64 << 32) + 1).wrapping_add(1)
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
    RANDOM_STATE.with(|g| { if !g.initialized { g.seed(1); } g.next() })
}

#[no_mangle]
pub extern "C" fn srandom(seed: u32) {
    RANDOM_STATE.with(|g| g.seed(seed));
}

#[no_mangle]
pub unsafe extern "C" fn rand_r(seed: *mut u32) -> i32 {
    // Full-period 32-bit LCG: multiplier 1 modulo 4, odd increment.
    let x = (*seed).wrapping_mul((1 << 16) + 1).wrapping_add(1);
    *seed = x;
    (x >> 1) as i32
}

// ----------------------------------------------------------------------------
// drand48 and relatives: 48-bit state as three 16-bit words, least significant first.
// ----------------------------------------------------------------------------

const MASK48: u64 = (1 << 48) - 1;
// POSIX.1-2024 drand48 specifies a=0x5DEECE66D and c=0xB.
const A_DEFAULT: u64 = 0x5_DEEC_E66D;
const C_DEFAULT: u16 = 0xB;

struct Lcg48 {
    x: [u16; 3],
    a: [u16; 3],
    c: u16,
    seed_out: [u16; 3],
}

static LCG48: Locked<Lcg48> = Locked::new(Lcg48 {
    x: [0x330E, 0, 0],
    a: [A_DEFAULT as u16, (A_DEFAULT >> 16) as u16, (A_DEFAULT >> 32) as u16],
    c: C_DEFAULT,
    seed_out: [0; 3],
});

fn join(w: &[u16; 3]) -> u64 {
    w[0] as u64 | (w[1] as u64) << 16 | (w[2] as u64) << 32
}

fn split(v: u64) -> [u16; 3] {
    [v as u16, (v >> 16) as u16, (v >> 32) as u16]
}

/// Advances xsubi and returns the new 48-bit value.
unsafe fn next48(g: &Lcg48, xsubi: *mut u16) -> u64 {
    let x = join(&*(xsubi as *const [u16; 3]));
    let v = (join(&g.a).wrapping_mul(x).wrapping_add(g.c as u64)) & MASK48;
    *(xsubi as *mut [u16; 3]) = split(v);
    v
}

unsafe fn next_global(g: &mut Lcg48) -> u64 {
    let v = (join(&g.a).wrapping_mul(join(&g.x)).wrapping_add(g.c as u64)) & MASK48;
    g.x = split(v);
    v
}

#[no_mangle]
pub unsafe extern "C" fn erand48(xsubi: *mut u16) -> f64 {
    LCG48.with(|g| next48(g, xsubi)) as f64 / (1u64 << 48) as f64
}
#[no_mangle]
pub unsafe extern "C" fn drand48() -> f64 {
    LCG48.with(|g| next_global(g)) as f64 / (1u64 << 48) as f64
}
#[no_mangle]
pub unsafe extern "C" fn nrand48(xsubi: *mut u16) -> i64 {
    (LCG48.with(|g| next48(g, xsubi)) >> 17) as i64
}
#[no_mangle]
pub unsafe extern "C" fn lrand48() -> i64 {
    (LCG48.with(|g| next_global(g)) >> 17) as i64
}
#[no_mangle]
pub unsafe extern "C" fn jrand48(xsubi: *mut u16) -> i64 {
    (LCG48.with(|g| next48(g, xsubi)) >> 16) as u32 as i32 as i64
}
#[no_mangle]
pub unsafe extern "C" fn mrand48() -> i64 {
    (LCG48.with(|g| next_global(g)) >> 16) as u32 as i32 as i64
}
fn reset_multiplier(g: &mut Lcg48) { g.a = split(A_DEFAULT); g.c = C_DEFAULT; }
#[no_mangle]
pub unsafe extern "C" fn srand48(seed: i64) {
    LCG48.with(|g| {
        reset_multiplier(g);
        // POSIX specifies the low 16 seed bits as 0x330E.
        g.x = [0x330E, seed as u16, (seed >> 16) as u16];
    });
}
#[no_mangle]
pub unsafe extern "C" fn seed48(seed16v: *mut u16) -> *mut u16 {
    LCG48.with(|g| {
        reset_multiplier(g);
        g.seed_out = g.x;
        g.x = *(seed16v as *const [u16; 3]);
        g.seed_out.as_mut_ptr()
    })
}
#[no_mangle]
pub unsafe extern "C" fn lcong48(param: *mut u16) {
    let p = &*(param as *const [u16; 7]);
    LCG48.with(|g| {
        g.x = [p[0], p[1], p[2]];
        g.a = [p[3], p[4], p[5]];
        g.c = p[6];
    });
}
