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
//! # The window has to outlast a scheduling quantum, not just a tick
//!
//! A timer tick alone is not enough. `kernel/src/interrupts/timer.rs` only
//! sets `need_resched` when the thread's quantum expires (`TIME_QUANTUM` = 10
//! ticks), and it is the reschedule -- not the tick -- that reaches
//! `save_userspace_context` and its `log::trace!`. A DF=1 window shorter than
//! one quantum takes ticks without ever taking the save path. So the window
//! escalates until one of them measurably spans `MIN_WINDOW_MS`, which is set
//! several quanta wide, and then repeats at that size.
//! claim-lint:ok: measured -- a 32 ms window on the x86 gate host took 0
//! from-userspace saves (0 `<S>` markers between this program's own begin and
//! end markers on that boot); the quantum constant is
//! `kernel/src/interrupts/timer.rs`'s `TIME_QUANTUM`. #737.
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
//! # Markers
//!
//! Each window is bracketed, so a reader can count the kernel's own
//! from-userspace save markers between the two lines and know whether the
//! window was actually preempted:
//!
//! ```text
//! [DF_PREEMPT] window <n> begin iterations=<k>
//! [DF_PREEMPT] window <n> end elapsed_ms=<m> rflags_before_cld=0x<..> rflags_after_cld=0x<..>
//! ```
//!
//! and the summary lines are
//!
//! ```text
//! [DF_PREEMPT] windows=<n> long_windows=<n> iterations=<k>
//! [DF_PREEMPT] clock_stalled=<0|1> budget_exhausted=<0|1> spin_ceiling_hit=<0|1>
//! [DF_PREEMPT] ticks_spanned=<n> df_after_cld=<0|1> df_roundtrip=<ok|bad>
//! ```
//!
//! # Why this program terminates
//!
//! The escalation multiplies the spin by 8 per short window, so a window
//! ceiling alone does not bound run time -- window 12 of a run that escalated
//! on each of the 11 preceding windows would spin `10_000_000 * 8^11`
//! register-only passes, which is years of wall clock, not a terminating run.
//! Three bounds close that, and between them they cover 3 of 3 clock
//! behaviours:
//!
//! * **a stopped clock** -- each window measures `elapsed_ms == 0`, and two
//!   consecutive zero-length windows (`STALLED_WINDOWS`) end the run. Cost:
//!   two windows, `10M + 80M` passes.
//! * **the clock advances but no window reaches `MIN_WINDOW_MS`** -- the
//!   run-total wall-clock budget (`TOTAL_BUDGET_MS`, read from the same
//!   CLOCK_MONOTONIC, checked between windows) ends the run.
//! * **either way** -- escalation stops at `MAX_SPIN_ITERATIONS`, so no single
//!   window between two budget checks can cost more than 64x the base window.
//!
//! Whichever bound fires is printed, so a truncated run says so rather than
//! looking like a healthy one. `MAX_WINDOWS` bounds the window count and
//! nothing else; it is not the termination argument.
//! claim-lint:ok: this is a statement about this file's own control flow --
//! the loop condition, the three `break`/ceiling guards and the constants they
//! read are all below in this file -- not a measurement of a host. The costs
//! quoted for the stalled-clock case are `BASE_SPIN_ITERATIONS` and one x8
//! escalation, arithmetic on the constants. #737.
//!
//! `ticks_spanned` is the longest window's CLOCK_MONOTONIC milliseconds; the
//! timer runs at a fixed rate, so it is also the number of timer ticks that
//! window spanned. `df_roundtrip` is `ok` when DF was still set at the instant
//! before this program cleared it in each window, i.e. when the preemptions it
//! did take returned the flag unchanged.
//! claim-lint:ok: the marker reports what this program measured on the boot
//! that printed it -- 1 of 1 sampled RFLAGS value per window, sampled by the
//! `pushfq` immediately preceding that window's `cld` -- and claims nothing
//! about preemptions it did not observe. #737.

use std::process;

/// RFLAGS bit 10.
#[cfg(target_arch = "x86_64")]
const DF_BIT: u64 = 1 << 10;

/// Loop iterations in the first DF=1 window. Escalated below until a window
/// measurably spans `MIN_WINDOW_MS`, so no calibration constant is
/// load-bearing.
#[cfg(target_arch = "x86_64")]
const BASE_SPIN_ITERATIONS: u64 = 10_000_000;

/// Milliseconds a window has to span before the oracle stops escalating.
/// Several scheduling quanta wide, so quantum expiry lands inside it.
#[cfg(target_arch = "x86_64")]
const MIN_WINDOW_MS: i64 = 200;

/// How many windows at the escalated size to run once the size is found.
#[cfg(target_arch = "x86_64")]
const LONG_WINDOWS_WANTED: u32 = 3;

/// Ceiling on total windows. This bounds the window *count* only -- on its own
/// it does not bound run time, because each escalation multiplies the spin by
/// 8, so a run that escalated all the way to window 12 would spin
/// `BASE_SPIN_ITERATIONS * 8^11` times, which is not a run anybody sees end.
/// The three bounds below are what actually make this program terminate.
#[cfg(target_arch = "x86_64")]
const MAX_WINDOWS: u32 = 12;

/// Ceiling on the escalated spin size, so no single window can cost more than
/// 64x the base window. Escalation stops here; the loop then keeps sampling at
/// this size until one of the other bounds fires.
#[cfg(target_arch = "x86_64")]
const MAX_SPIN_ITERATIONS: u64 = BASE_SPIN_ITERATIONS * 64;

/// Wall-clock budget across the run's windows, read from the same
/// CLOCK_MONOTONIC and checked between windows. This is the bound that fires
/// on a host whose clock advances but where no window reaches
/// `MIN_WINDOW_MS`.
#[cfg(target_arch = "x86_64")]
const TOTAL_BUDGET_MS: i64 = 30_000;

/// Consecutive windows reporting `elapsed_ms == 0` that end the run. A stopped
/// clock reports 0 on each window, so the run stops here after two base-sized
/// windows instead of escalating into a spin no one will see end. This is the
/// bound `MAX_WINDOWS` was wrongly documented as providing.
#[cfg(target_arch = "x86_64")]
const STALLED_WINDOWS: u32 = 2;

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
    let mut long_windows = 0u32;
    let mut roundtrip_ok = true;
    let mut ticks_spanned: i64 = 0;
    let mut df_after_cld: u64 = 1;
    let mut stalled_windows = 0u32;
    let mut clock_stalled = false;
    let mut budget_exhausted = false;
    let run_start_ms = monotonic_ms();

    while windows < MAX_WINDOWS && long_windows < LONG_WINDOWS_WANTED {
        windows += 1;
        println!("[DF_PREEMPT] window {} begin iterations={}", windows, iterations);

        let start_ms = monotonic_ms();
        let (before, after) = df_window(iterations);
        let end_ms = monotonic_ms();
        let elapsed = end_ms - start_ms;

        println!(
            "[DF_PREEMPT] window {} end elapsed_ms={} rflags_before_cld=0x{:x} rflags_after_cld=0x{:x}",
            windows, elapsed, before, after
        );

        if before & DF_BIT == 0 {
            roundtrip_ok = false;
        }
        df_after_cld = (after & DF_BIT) >> 10;
        if elapsed > ticks_spanned {
            ticks_spanned = elapsed;
        }

        // A stopped clock makes each window measure 0 and each escalation
        // meaningless. Stop rather than multiply the spin by 8 again -- that
        // is the difference between terminating and appearing to hang for the
        // rest of the boot's timeout.
        if elapsed == 0 {
            stalled_windows += 1;
            if stalled_windows >= STALLED_WINDOWS {
                clock_stalled = true;
                break;
            }
        } else {
            stalled_windows = 0;
        }

        if elapsed >= MIN_WINDOW_MS {
            long_windows += 1;
        } else if iterations < MAX_SPIN_ITERATIONS {
            iterations = iterations.saturating_mul(8).min(MAX_SPIN_ITERATIONS);
        }

        if monotonic_ms() - run_start_ms >= TOTAL_BUDGET_MS {
            budget_exhausted = true;
            break;
        }
    }

    println!(
        "[DF_PREEMPT] windows={} long_windows={} iterations={}",
        windows, long_windows, iterations
    );
    println!(
        "[DF_PREEMPT] clock_stalled={} budget_exhausted={} spin_ceiling_hit={}",
        clock_stalled as u32,
        budget_exhausted as u32,
        (iterations >= MAX_SPIN_ITERATIONS) as u32
    );
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
