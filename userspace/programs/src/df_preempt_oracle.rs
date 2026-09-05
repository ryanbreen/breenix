//! `df_preempt_oracle` -- a fork-free, deterministic oracle for #737.
//!
//! # What it is for
//!
//! #737's accepted mechanism is that an x86-64 interrupt gate preserves the
//! direction flag. A ring-3 thread that is preempted by the timer while DF=1
//! therefore enters the timer ISR with DF=1, and every `rep`-prefixed string
//! operation the ISR then executes runs backwards. On the from-userspace save
//! path the ISR reaches a `log::trace!` whose `log_impl` calls
//! `log::RecordBuilder::new`; that function returns its 128-byte struct with a
//! `rep movsq` into an out-pointer 0x50 bytes above `log_impl`'s spill of the
//! `&'static Location`. Backwards, that copy overwrites the spill, and
//! `Location::file`'s `mov (%rdi),%rax` then dereferences whatever landed
//! there.
//! claim-lint:ok: "every rep-prefixed string operation runs backwards" is the
//! definition of DF=1 on x86-64, not a survey; the disassembly of the specific
//! path is #737's RCA record and this program does not restate its figures.
//!
//! In the wild that needs the timer to land inside a userspace `memmove`
//! backward arm -- a few-instruction window -- so it shows up rarely. This
//! program removes the lottery: it *stays* in a DF=1 window across many timer
//! ticks, so a from-userspace preempt that lands there is inside it.
//!
//! # Two-sided by construction
//!
//! * On kernel bytes with the defect, this program is not expected to print
//!   its result marker at all: the kernel faults on the save path while this
//!   thread is preempted, and the boot dies with a page fault at
//!   `Location::file`.
//! * On kernel bytes where the timer entry clears DF, the program completes
//!   and prints `df_roundtrip=ok` -- which additionally pins the property the
//!   fix must not break, that a `cld` in the handler does not disturb the
//!   *userspace* direction flag. IRETQ restores RFLAGS from the interrupt
//!   frame, and `save_userspace_context` / `restore_userspace_context`
//!   round-trip `interrupt_frame.cpu_flags` through `thread.context.rflags`,
//!   so a preempted thread resumes with DF still set.
//!
//! # Why the window is one inline-asm block
//!
//! Between `std` and `cld` this program must execute no `rep`-prefixed string
//! operation of its own -- with DF set, its own `memmove` would run backwards
//! over its own stack. So the whole window is a single `asm!` block with no
//! call out of it: `std`, a register-only counting loop, `pushfq` to sample
//! RFLAGS while DF is still set, `cld`, and a second `pushfq`. The clock is
//! read *outside* the window, because a syscall would leave it.
//!
//! Fork-free on purpose: `docker/qemu/run-x86-boot-tests.sh` derives
//! `PRODUCTION_REAPED_ROWS` by counting `fork()` call sites in each
//! RING3_SMOKE roster program, so a program with no such call site contributes
//! 0 to that census. This one moves only `EXPECTED_USERSPACE_EXITS`, by 1.
//! claim-lint:ok: 0 of 0 `fork()` call sites in this file, under the same
//! `grep -cE '(match|=) *([a-zA-Z_]+::)*fork\(\)'` pattern the gate script's
//! own census loop runs; the gate's derivation is at
//! `docker/qemu/run-x86-boot-tests.sh`. #737.
//!
//! # Marker
//!
//! ```text
//! [DF_PREEMPT] ticks_spanned=<n> df_after_cld=<0|1> df_roundtrip=<ok|bad>
//! ```
//!
//! `ticks_spanned` is CLOCK_MONOTONIC milliseconds elapsed across the DF=1
//! window; the timer runs at 1000 Hz, so it is also the number of timer ticks
//! the window spanned. `df_roundtrip` is `ok` when DF was still set at the
//! instant before this program cleared it, i.e. when every preemption inside
//! the window returned the flag unchanged.
//! claim-lint:ok: the marker reports what this program measured on the boot
//! that printed it -- 1 of 1 sampled RFLAGS value per window, sampled by the
//! `pushfq` immediately preceding the `cld` -- and claims nothing about
//! preemptions it did not observe. #737.

use std::process;

/// RFLAGS bit 10.
#[cfg(target_arch = "x86_64")]
const DF_BIT: u64 = 1 << 10;

/// Loop iterations inside one DF=1 window. Sized to span far more than the
/// several timer ticks the oracle needs on a TCG-emulated qemu64, and
/// escalated below when a measurement says otherwise, so no calibration
/// constant is load-bearing.
#[cfg(target_arch = "x86_64")]
const BASE_SPIN_ITERATIONS: u64 = 10_000_000;

/// Minimum milliseconds a window has to span before the oracle stops
/// escalating.
#[cfg(target_arch = "x86_64")]
const MIN_WINDOW_MS: i64 = 10;

/// CLOCK_MONOTONIC in milliseconds.
#[cfg(target_arch = "x86_64")]
fn monotonic_ms() -> i64 {
    let ts = libbreenix::time::now_monotonic().expect("clock_gettime(CLOCK_MONOTONIC)");
    ts.tv_sec as i64 * 1000 + ts.tv_nsec as i64 / 1_000_000
}

/// Run one DF=1 window of `iterations` register-only loop passes.
///
/// Returns `(rflags_before_cld, rflags_after_cld)`.
///
/// Everything between `std` and `cld` is inside this one asm block: no call,
/// no string instruction, and no memory operand other than the two `pushfq`
/// samples (an ordinary push is not a string operation and does not consult
/// DF).
#[cfg(target_arch = "x86_64")]
fn df_window(iterations: u64) -> (u64, u64) {
    let before: u64;
    let after: u64;
    unsafe {
        core::arch::asm!(
            "std",
            "2:",
            "sub {counter}, 1",
            "jnz 2b",
            "pushfq",
            "pop {before}",
            "cld",
            "pushfq",
            "pop {after}",
            counter = inout(reg) iterations => _,
            before = out(reg) before,
            after = out(reg) after,
        );
    }
    (before, after)
}

#[cfg(target_arch = "x86_64")]
fn main() {
    println!("[DF_PREEMPT] start: entering DF=1 windows, no fork, no sleep");

    let mut iterations = BASE_SPIN_ITERATIONS;
    let mut windows = 0u32;
    let mut roundtrip_ok = true;
    let mut ticks_spanned: i64 = 0;
    let mut df_after_cld: u64 = 1;

    // Escalate the window size until one window measurably spans at least
    // MIN_WINDOW_MS. An early attempt that turns out to be too short is still
    // a real exposure, not a wasted one.
    for _ in 0..4 {
        let start_ms = monotonic_ms();
        let (before, after) = df_window(iterations);
        let end_ms = monotonic_ms();
        windows += 1;

        if before & DF_BIT == 0 {
            roundtrip_ok = false;
        }
        df_after_cld = (after & DF_BIT) >> 10;
        ticks_spanned = end_ms - start_ms;

        println!(
            "[DF_PREEMPT] window {}: iterations={} elapsed_ms={} rflags_before_cld=0x{:x} rflags_after_cld=0x{:x}",
            windows, iterations, ticks_spanned, before, after
        );

        if ticks_spanned >= MIN_WINDOW_MS {
            break;
        }
        iterations = iterations.saturating_mul(8);
    }

    println!(
        "[DF_PREEMPT] ticks_spanned={} df_after_cld={} df_roundtrip={}",
        ticks_spanned,
        df_after_cld,
        if roundtrip_ok { "ok" } else { "bad" }
    );

    process::exit(0);
}

#[cfg(not(target_arch = "x86_64"))]
fn main() {
    // The direction flag is an x86 concept; #737 is an x86-only defect.
    println!("[DF_PREEMPT] SKIP: df_preempt_oracle is x86_64-only");
    process::exit(0);
}
