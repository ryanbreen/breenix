//! Kernel random numbers: the pool behind getrandom(2).
//!
//! One generator (`generator.rs`), shared by the CPUs, keeps its key from one
//! read to the next. Its inputs:
//!
//! - At boot, the CPU's random-number instructions where the CPU reports
//!   them (x86-64 RDSEED, else RDRAND; ARMv8.5 RNDRRS, else RNDR), an entropy
//!   device where the machine has one (virtio-rng), and windows of timer
//!   jitter: the counter time of a fixed memory walk (`jitter.rs`).
//! - Device events (`add_event`): the cycle counter at each received network
//!   frame and each terminal input byte, gathered lock-free and folded in at
//!   the next read.
//! - At each read: the cycle counter and, where present, one RDRAND or RNDR
//!   word.
//!
//! The pool is ready once `READY_BITS` have been credited. Credit is
//! conservative: a full word per RDSEED or RNDRRS word, half a word per RDRAND
//! or RNDR word (each is a generator's output, not raw entropy), 8 bits per
//! byte from an entropy device (it exists to supply them). Timer jitter and
//! device events add input but no credit: their entropy is not independently
//! assessed. Passing health diagnostics (`health.rs`) does not establish it.
//! Without any credited source the pool stays unready.

mod blake2s;
mod chacha20;
pub(crate) mod erase;
mod generator;
mod health;
mod jitter;

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use generator::{Generator, MAX_FILL};
use spin::Mutex;

/// Credited bits at which the pool counts as initialized.
pub const READY_BITS: u32 = 256;

/// Most jitter windows boot seeding takes (4096 samples).
const JITTER_MAX_WINDOWS: u32 = 8;

/// The generator's key. A leaf lock, held only with interrupts masked; callers
/// copy to user memory after releasing it.
static GENERATOR: Mutex<Generator> = Mutex::new(Generator::new());

static CREDITED_BITS: AtomicU32 = AtomicU32::new(0);
static READY: AtomicBool = AtomicBool::new(false);

const HW_RDRAND: u8 = 1 << 0;
const HW_RDSEED: u8 = 1 << 1;
const HW_RNDR: u8 = 1 << 2;
/// Which random-number instructions the CPU reported, set by `init`.
static HARDWARE: AtomicU8 = AtomicU8::new(0);

const EVENT_SLOTS: usize = 8;
static EVENTS: [AtomicU64; EVENT_SLOTS] = [const { AtomicU64::new(0) }; EVENT_SLOTS];
static EVENT_COUNT: AtomicU64 = AtomicU64::new(0);

/// A device event whose time feeds the pool.
#[derive(Clone, Copy)]
#[repr(u64)]
pub enum Event {
    NetRx = 1,
    TtyInput = 2,
}

/// What boot seeding found, for the boot log.
pub struct BootReport {
    pub rdrand: bool,
    pub rdseed: bool,
    pub rndr: bool,
    pub hardware_bits: u32,
    pub device_bits: u32,
    pub jitter: JitterReport,
    pub ready: bool,
}

/// The jitter windows boot seeding took, and the worst figures they showed
/// against the health tests' cutoffs.
#[derive(Default)]
pub struct JitterReport {
    pub windows: u32,
    pub passed: u32,
    pub bits: u32,
    pub longest_run: u32,
    pub most_common: u32,
}

impl JitterReport {
    pub const RUN_CUTOFF: u32 = health::RCT_CUTOFF;
    pub const COMMON_CUTOFF: u32 = health::APT_CUTOFF;
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn counter() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn counter() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) value, options(nomem, nostack));
    }
    value
}

#[cfg(target_arch = "x86_64")]
fn probe_hardware() -> u8 {
    use core::arch::x86_64::{__cpuid, __cpuid_count};
    let mut found = 0;
    // CPUID leaf 1 ECX bit 30 is RDRAND; leaf 7 subleaf 0 EBX bit 18 is RDSEED.
    let (max_leaf, leaf1_ecx) = unsafe { (__cpuid(0).eax, __cpuid(1).ecx) };
    if leaf1_ecx & (1 << 30) != 0 {
        found |= HW_RDRAND;
    }
    if max_leaf >= 7 && unsafe { __cpuid_count(7, 0) }.ebx & (1 << 18) != 0 {
        found |= HW_RDSEED;
    }
    found
}

#[cfg(target_arch = "aarch64")]
fn probe_hardware() -> u8 {
    let isar0: u64;
    unsafe {
        core::arch::asm!("mrs {}, id_aa64isar0_el1", out(reg) isar0, options(nomem, nostack));
    }
    // ID_AA64ISAR0_EL1.RNDR, bits 63:60: 1 means RNDR and RNDRRS are implemented.
    if (isar0 >> 60) & 0xF >= 1 {
        HW_RNDR
    } else {
        0
    }
}

/// One RDRAND (`reseeded == false`) or RDSEED word. Both can fail transiently,
/// so each is retried a few times. A word of 1 bits (u64::MAX) is refused: it
/// is the value a faulty RDRAND returns with the success flag set.
#[cfg(target_arch = "x86_64")]
fn hardware_word(reseeded: bool) -> Option<u64> {
    let wanted = if reseeded { HW_RDSEED } else { HW_RDRAND };
    if HARDWARE.load(Ordering::Relaxed) & wanted == 0 {
        return None;
    }
    for _ in 0..16 {
        let value: u64;
        let ok: u8;
        unsafe {
            if reseeded {
                core::arch::asm!("rdseed {v}", "setc {ok}", v = out(reg) value, ok = out(reg_byte) ok,
                    options(nomem, nostack));
            } else {
                core::arch::asm!("rdrand {v}", "setc {ok}", v = out(reg) value, ok = out(reg_byte) ok,
                    options(nomem, nostack));
            }
        }
        if ok != 0 && value != u64::MAX {
            return Some(value);
        }
        core::hint::spin_loop();
    }
    None
}

/// One RNDR (`reseeded == false`) or RNDRRS word; the instruction sets Z on
/// failure, and failures are transient, so each is retried a few times.
#[cfg(target_arch = "aarch64")]
fn hardware_word(reseeded: bool) -> Option<u64> {
    if HARDWARE.load(Ordering::Relaxed) & HW_RNDR == 0 {
        return None;
    }
    for _ in 0..16 {
        let value: u64;
        let ok: u64;
        unsafe {
            // RNDR is s3_3_c2_c4_0 and RNDRRS is s3_3_c2_c4_1.
            if reseeded {
                core::arch::asm!("mrs {v}, s3_3_c2_c4_1", "cset {ok}, ne", v = out(reg) value,
                    ok = out(reg) ok, options(nomem, nostack));
            } else {
                core::arch::asm!("mrs {v}, s3_3_c2_c4_0", "cset {ok}, ne", v = out(reg) value,
                    ok = out(reg) ok, options(nomem, nostack));
            }
        }
        if ok != 0 {
            return Some(value);
        }
        core::hint::spin_loop();
    }
    None
}

fn with_generator<R>(f: impl FnOnce(&mut Generator) -> R) -> R {
    crate::arch_without_interrupts(|| f(&mut GENERATOR.lock()))
}

fn credit(bits: u32) {
    let total = CREDITED_BITS
        .fetch_add(bits, Ordering::AcqRel)
        .saturating_add(bits);
    if total >= READY_BITS {
        READY.store(true, Ordering::Release);
    }
}

/// Whether the pool has been credited with `READY_BITS`.
pub fn is_ready() -> bool {
    READY.load(Ordering::Acquire)
}

/// Record the time of a device event, with a value that came with it.
/// Lock-free and allocation-free: interrupt and softirq context may call it.
#[inline]
pub fn add_event(event: Event, value: u64) {
    let n = EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
    let sample = counter() ^ value.rotate_left(32) ^ ((event as u64) << 59);
    let rotation = ((n / EVENT_SLOTS as u64).wrapping_mul(7) % 64) as u32;
    EVENTS[n as usize % EVENT_SLOTS].fetch_xor(sample.rotate_left(rotation), Ordering::Relaxed);
}

/// Take jitter windows: the start-up windows first, then more until the pool
/// is ready, up to `JITTER_MAX_WINDOWS`, stopping at the first failure. Each
/// window is mixed in without entropy credit; health tests are diagnostics.
fn collect_jitter() -> JitterReport {
    let mut report = JitterReport::default();
    let Some(mut source) = jitter::Jitter::new() else {
        return report;
    };
    let startup = health::STARTUP_SAMPLES / health::WINDOW;
    let mut earned = health::Credit::new();
    let mut digest = [0u8; 32];
    while report.windows < JITTER_MAX_WINDOWS
        && !earned.failed()
        && (report.windows < startup || !is_ready())
    {
        let verdict = source.window(&mut digest);
        with_generator(|g| g.mix(&[b"jitter", &digest]));
        report.windows += 1;
        report.passed += u32::from(verdict.passed());
        report.longest_run = report.longest_run.max(verdict.longest_run);
        report.most_common = report.most_common.max(verdict.most_common);
        let bits = earned.record(&verdict);
        report.bits += bits;
    }
    erase::wipe(&mut digest);
    report
}

/// Bytes from the machine's entropy device into `out`; 0 without one.
#[cfg(target_arch = "aarch64")]
fn device_read(out: &mut [u8]) -> usize {
    crate::drivers::virtio::rng::read(out)
}

#[cfg(not(target_arch = "aarch64"))]
fn device_read(_out: &mut [u8]) -> usize {
    0
}

/// Ask the entropy device for a seed; each byte it returns earns 8 bits.
fn reseed_device() -> u32 {
    let mut seed = [0u8; 64];
    let got = device_read(&mut seed);
    if got > 0 {
        with_generator(|g| g.mix(&[b"device", &seed[..got]]));
    }
    erase::wipe(&mut seed);
    let bits = got as u32 * 8;
    credit(bits);
    bits
}

/// Ask the sources that can be asked again: the CPU's instructions and the
/// entropy device. Returns the bits credited.
pub fn reseed() -> u32 {
    reseed_hardware() + reseed_device()
}

/// Retry the CPU RNG after transient failures; only successful words earn credit.
fn reseed_hardware() -> u32 {
    let mut seed = [0u8; 64];
    let mut hardware_bits = 0;
    for chunk in seed.chunks_exact_mut(8) {
        let word = match hardware_word(true) {
            Some(word) => {
                hardware_bits += 64;
                Some(word)
            }
            None => hardware_word(false).inspect(|_| hardware_bits += 32),
        };
        if let Some(word) = word {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
    }
    with_generator(|g| g.mix(&[b"hardware", &seed]));
    erase::wipe(&mut seed);
    credit(hardware_bits);
    hardware_bits
}

/// Seed the pool at boot. Runs once, before the network stack and before any
/// user process.
pub fn init() -> BootReport {
    let hardware = probe_hardware();
    HARDWARE.store(hardware, Ordering::Relaxed);

    let hardware_bits = reseed_hardware();
    let device_bits = reseed_device();
    let jitter = collect_jitter();
    BootReport {
        rdrand: hardware & HW_RDRAND != 0,
        rdseed: hardware & HW_RDSEED != 0,
        rndr: hardware & HW_RNDR != 0,
        hardware_bits,
        device_bits,
        jitter,
        ready: is_ready(),
    }
}

/// Fold the inputs gathered since the last read into the key.
fn mix_fresh(g: &mut Generator) {
    let mut events = [0u8; 8 * (EVENT_SLOTS + 1)];
    for (slot, out) in EVENTS.iter().zip(events.chunks_exact_mut(8)) {
        out.copy_from_slice(&slot.swap(0, Ordering::Relaxed).to_le_bytes());
    }
    events[8 * EVENT_SLOTS..].copy_from_slice(&EVENT_COUNT.load(Ordering::Relaxed).to_le_bytes());
    let hardware = hardware_word(false).unwrap_or(0);
    g.mix(&[
        b"read",
        &counter().to_le_bytes(),
        &hardware.to_le_bytes(),
        &events,
    ]);
    erase::wipe(&mut events);
}

/// Fill `out` (at most 256 bytes) from the pool. The first chunk of a read
/// folds in fresh inputs first; later chunks only advance the key.
pub fn fill_chunk(out: &mut [u8], fresh: bool) {
    with_generator(|g| {
        if fresh {
            mix_fresh(g);
        }
        g.fill(out);
    });
}

/// Longest chunk `fill_chunk` takes.
pub const CHUNK: usize = MAX_FILL;

/// Fill `out` from the pool, whether or not it is ready yet. For kernel users
/// that cannot wait (port numbers, IDs). Before readiness, unpredictability
/// may be absent.
pub fn fill(out: &mut [u8]) {
    for (i, chunk) in out.chunks_mut(CHUNK).enumerate() {
        fill_chunk(chunk, i == 0);
    }
}

/// One `u64` from the pool (see `fill` for readiness limits).
pub fn next_u64() -> u64 {
    let mut bytes = [0u8; 8];
    fill(&mut bytes);
    u64::from_le_bytes(bytes)
}
