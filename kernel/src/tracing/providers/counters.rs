//! Built-in trace counters for kernel statistics.
//!
//! This module defines the core kernel counters that track fundamental
//! operations like syscalls, interrupts, and context switches.
//!
//! # Available Counters
//!
//! - `SYSCALL_TOTAL`: Total syscall invocations across all CPUs
//! - `IRQ_TOTAL`: Total interrupt invocations
//! - `CTX_SWITCH_TOTAL`: Total context switches
//! - `TIMER_TICK_TOTAL`: Total timer tick interrupts
//!
//! # Usage
//!
//! ```rust,ignore
//! use kernel::tracing::providers::counters::{SYSCALL_TOTAL, trace_count};
//!
//! // Increment in hot path (compiles to single atomic add)
//! trace_count!(SYSCALL_TOTAL);
//!
//! // Query aggregated value
//! let total = SYSCALL_TOTAL.aggregate();
//! ```
//!
//! # GDB Inspection
//!
//! ```gdb
//! # View all counter values
//! print SYSCALL_TOTAL
//! print SYSCALL_TOTAL.per_cpu[0].value
//!
//! # View aggregated total (requires helper script)
//! # Or manually sum: p SYSCALL_TOTAL.per_cpu[0].value + SYSCALL_TOTAL.per_cpu[1].value + ...
//! ```

use crate::tracing::counter::{register_counter, TraceCounter};
use core::sync::atomic::{AtomicU64, Ordering};

// =============================================================================
// Built-in Counter Definitions
// =============================================================================

/// Total syscall invocations across all CPUs.
///
/// Incremented at syscall entry, before dispatching to the handler.
/// Use `aggregate()` to get the total count across all CPUs.
///
/// GDB: `print SYSCALL_TOTAL`
#[no_mangle]
pub static SYSCALL_TOTAL: TraceCounter =
    TraceCounter::new("SYSCALL_TOTAL", "Total syscall invocations");

/// Total interrupt invocations across all CPUs.
///
/// Incremented at interrupt entry, for all interrupt types.
/// Includes timer, keyboard, disk, network, etc.
///
/// GDB: `print IRQ_TOTAL`
#[no_mangle]
pub static IRQ_TOTAL: TraceCounter = TraceCounter::new("IRQ_TOTAL", "Total interrupt invocations");

/// Total context switches across all CPUs.
///
/// Incremented when switching from one thread/process to another.
/// Does not count switches to/from idle.
///
/// GDB: `print CTX_SWITCH_TOTAL`
#[no_mangle]
pub static CTX_SWITCH_TOTAL: TraceCounter =
    TraceCounter::new("CTX_SWITCH_TOTAL", "Total context switches");

/// Total timer tick interrupts across all CPUs.
///
/// Incremented in the timer interrupt handler.
/// Useful for measuring uptime and verifying timer frequency.
///
/// GDB: `print TIMER_TICK_TOTAL`
#[no_mangle]
pub static TIMER_TICK_TOTAL: TraceCounter =
    TraceCounter::new("TIMER_TICK_TOTAL", "Total timer tick interrupts");

/// Total fork operations across all CPUs.
///
/// Incremented at fork entry, before the fork is performed.
/// Use `aggregate()` to get the total count across all CPUs.
///
/// GDB: `print FORK_TOTAL`
#[no_mangle]
pub static FORK_TOTAL: TraceCounter = TraceCounter::new("FORK_TOTAL", "Total fork operations");

/// Total exec operations across all CPUs.
///
/// Incremented at exec entry, before the exec is performed.
/// Use `aggregate()` to get the total count across all CPUs.
///
/// GDB: `print EXEC_TOTAL`
#[no_mangle]
pub static EXEC_TOTAL: TraceCounter = TraceCounter::new("EXEC_TOTAL", "Total exec operations");

/// Total CoW fault operations across all CPUs.
///
/// Incremented when a copy-on-write fault is triggered.
/// Use `aggregate()` to get the total count across all CPUs.
///
/// GDB: `print COW_FAULT_TOTAL`
#[no_mangle]
pub static COW_FAULT_TOTAL: TraceCounter =
    TraceCounter::new("COW_FAULT_TOTAL", "Total CoW fault operations");

/// Total idle timer ticks across all CPUs.
///
/// Incremented in the timer interrupt handler when the CPU is running
/// its idle thread. Per-CPU utilization = 1 - (idle_ticks / timer_ticks).
///
/// GDB: `print IDLE_TICK_TOTAL`
#[no_mangle]
pub static IDLE_TICK_TOTAL: TraceCounter =
    TraceCounter::new("IDLE_TICK_TOTAL", "Total idle timer ticks");

/// GPU compositor: total bytes uploaded to VRAM.
#[no_mangle]
pub static GPU_BYTES_UPLOADED: TraceCounter =
    TraceCounter::new("GPU_BYTES_UPLOADED", "GPU bytes uploaded to VRAM");

/// GPU compositor: full-screen uploads (4.9MB each).
#[no_mangle]
pub static GPU_FULL_UPLOADS: TraceCounter =
    TraceCounter::new("GPU_FULL_UPLOADS", "Full-screen GPU uploads");

/// GPU compositor: partial rect uploads.
#[no_mangle]
pub static GPU_PARTIAL_UPLOADS: TraceCounter =
    TraceCounter::new("GPU_PARTIAL_UPLOADS", "Partial rect GPU uploads");

/// Network RX softirq polls that exhausted their packet budget.
#[no_mangle]
pub static NET_RX_BUDGET_EXHAUSTED: TraceCounter =
    TraceCounter::new("NET_RX_BUDGET_EXHAUSTED", "NetRx softirq budget exhausted");

/// VirtIO PCI net IRQs that raised NetRx softirq work.
#[no_mangle]
pub static NET_PCI_IRQ_RAISED_NETRX: TraceCounter =
    TraceCounter::new("NET_PCI_IRQ_RAISED_NETRX", "PCI net IRQ raised NetRx");

/// GIC acknowledgements for VirtIO-net's GICv2m MSI-X SPI 55.
#[no_mangle]
pub static GIC_SPI55_ACK_TOTAL: TraceCounter =
    TraceCounter::new("GIC_SPI55_ACK_TOTAL", "GIC acknowledged SPI 55");

// =============================================================================
// Socket recv wait-loop wasted-turn counters (#772 instrumentation)
// =============================================================================

/// TCP recv wait loop (`sys_read`, `FdKind::Socket` blocking path in
/// `kernel/src/syscall/handlers.rs`) observed `ThreadState::Blocked` after a
/// dispatch + `yield_current()` + halt cycle and went back to sleep without
/// making progress.
///
/// This is the direct signal for issue #772's "wasted turn": the reader was
/// scheduled, ran the loop body, and found itself still `Blocked` even though
/// it had been given a CPU turn. A nonzero count here, correlated with
/// `unblock()` having already run `set_ready()` for the same thread before
/// this check executed, supports RCA reading (a) (the loop observes a state
/// that is stale relative to `unblock()`). Per-CPU aggregate only — see
/// `RECV_WAIT_STILL_BLOCKED_FALSE` doc comment for the per-tid caveat.
///
/// GDB: `print RECV_WAIT_STILL_BLOCKED_TRUE`
#[no_mangle]
pub static RECV_WAIT_STILL_BLOCKED_TRUE: TraceCounter = TraceCounter::new(
    "RECV_WAIT_STILL_BLOCKED_TRUE",
    "recv wait loop observed Blocked, looped back to sleep (#772)",
);

/// TCP recv wait loop observed a state other than `ThreadState::Blocked`
/// (i.e. `unblock()`'s `set_ready()` was visible to this thread) and
/// proceeded to clear `blocked_in_syscall` and return data to the caller.
///
/// Comparing this counter's growth against `RECV_WAIT_STILL_BLOCKED_TRUE`
/// and against `CTX_SWITCH_TOTAL` / `SCHED_PICK` occurrences for the same
/// thread distinguishes RCA reading (a) — the loop re-observes Blocked one
/// or more times before this fires — from reading (b) — the thread is
/// dispatched but this check (and everything else in the loop body) does
/// not run before being switched out again, so that turn increments
/// neither branch of this pair.
///
/// Per-tid tagging: `TraceCounter` (`kernel/src/tracing/counter.rs`) only
/// carries a per-CPU dimension (`per_cpu: [CpuCounterSlot; MAX_CPUS]`); there
/// is no per-tid slot in the counter primitive, and adding one would require
/// either a lock or a fixed-size tid-indexed array allocated statically (the
/// same pattern the scheduler's separate, non-tracing-framework
/// `WAKE_LAST_READY_SITE: [AtomicU64; WAKE_ATTRIB_MAX_TIDS]` in
/// `kernel/src/task/scheduler.rs` already uses for wake-site attribution).
/// That mechanism exists but is outside `kernel/src/tracing/`'s counter
/// registration, so these two counters stay global per-CPU aggregates as
/// instructed when no cheap per-tid tag is available in the framework itself.
///
/// GDB: `print RECV_WAIT_STILL_BLOCKED_FALSE`
#[no_mangle]
pub static RECV_WAIT_STILL_BLOCKED_FALSE: TraceCounter = TraceCounter::new(
    "RECV_WAIT_STILL_BLOCKED_FALSE",
    "recv wait loop observed not-Blocked, proceeded (#772)",
);

// =============================================================================
// Dispatch no-progress counters (#772)
// =============================================================================
//
// x86 only. The two counters below are written from
// `kernel/src/interrupts/context_switch.rs`, whose parent module
// `kernel/src/interrupts.rs` carries the `#![cfg(target_arch = "x86_64")]`;
// on aarch64 they register and stay 0.
// #772's spec (section 3) records why the aarch64 dispatch path does not carry
// the defect today and asks for a separate report-only census there rather
// than a silent extension.

/// An identical-frame preemption was observed at the `need_resched` gate:
/// a REVISIT census, not a defect census (R113).
///
/// Incremented in `check_need_resched_and_switch` when the frame the interrupt
/// was entered with is byte-identical (RIP *and* RSP) to the frame the last
/// completed dispatch installed for the same thread, and that thread is
/// neither blocked nor terminated.
///
// claim-lint:ok: 186 of 226 frame-changed pairs re-park on the two symbolised
// wait sites -- docs/planning/green-program/sockets/772-DIAG-2026-09-03.md
/// What it does NOT mean: that the dispatch retired no instruction.
/// `docs/planning/green-program/sockets/772-DIAG-2026-09-03.md` symbolised the
/// two addresses this population sits on -- the `ret` after `sti; hlt` inside
/// `enable_and_hlt`, and the `jmp` after `call interrupts::enable` inside
/// `without_interrupts` -- as the park points of the blocked-in-syscall wait
/// loops. A thread that goes once around its wait loop, finds its condition
/// still unmet and re-parks is saved at a byte-identical frame, and this
/// counter cannot tell it apart from one that never ran
/// (docs/planning/green-program/sockets/772-DIAG-2026-09-03.md). R113 therefore
/// retired the predicate as #772's oracle and R115 removed the refusal that
/// used to act on it; the counter itself stays, as the cheapest available
/// measure of how often the wait loops re-park.
///
/// GDB: `print DISPATCH_NO_PROGRESS`
#[no_mangle]
pub static DISPATCH_NO_PROGRESS: TraceCounter = TraceCounter::new(
    "DISPATCH_NO_PROGRESS",
    "identical-frame preemption observed at the need_resched gate (#772)",
);

/// Completed switches into a blocked-in-syscall kernel context.
///
/// Incremented once per kernel-context restore in `switch_to_thread`'s
/// blocked-in-syscall arm -- the same event the serial record "Restored kernel
/// context for thread N" names. This is the denominator `DISPATCH_NO_PROGRESS`
/// is read against.
///
/// GDB: `print DISPATCH_KERNEL_RESTORE_TOTAL`
#[no_mangle]
pub static DISPATCH_KERNEL_RESTORE_TOTAL: TraceCounter = TraceCounter::new(
    "DISPATCH_KERNEL_RESTORE_TOTAL",
    "completed switches into a blocked-in-syscall kernel context (#772)",
);

// =============================================================================
// Dispatch save-path census (#772 diagnostics, R111/R112)
// =============================================================================
//
// x86 only, like the two counters above: written from
// `kernel/src/interrupts/context_switch.rs`, whose parent module
// `kernel/src/interrupts.rs` carries the `#![cfg(target_arch = "x86_64")]`.
// They register on both arches and stay 0 on aarch64.
//
// R111/R112 ruled the candidate-A mechanism model incomplete: its refusal
// moved neither the identical-RIP/RSP proxy nor the latency. These counters
// were added to answer WHICH path produces each save, by splitting the
// dispatch path's context saves on (save flavour x which gate admitted the
// switch) and counting the identical-frame subset of each reason separately.
// The 40-boot battery they were built for is reported in
// docs/planning/green-program/sockets/772-DIAG-2026-09-03.md: the
// blocked/terminated arm produces 84.7-97.6% of the kernel-blocked
// identical-frame saves across its 4 arms, which is why the refusal -- whose
// `!current_thread_blocked_or_terminated` conjunct excluded exactly that arm
// -- could not move the aggregate. R115 withdrew the refusal; these counters
// land as instrumentation.
//
// Reading them:
//
// * The 6 `DISPATCH_SAVE_REASON_*` counters cover the 3 save arms the
//   interrupt-return path has, crossed with the 2 admitting gates. The 2
//   `DISPATCH_SAVE_REASON_KERNEL_BLOCKED_*` counters sum to the count of
//   "Saved kernel context for blocked thread N" serial records, because the
//   increment sits beside that record inside the same `if let`.
// * `DISPATCH_NOPROGRESS_SAVE_<X> <= DISPATCH_SAVE_REASON_<X>` by
//   construction: the no-progress counter is incremented only when the save
//   counter is, and only when the frame being saved is byte-identical (RIP
//   AND RSP) to the frame the last completed dispatch installed for the same
//   thread.
// * `_MANDATORY` names the gate that admits the switch because the current
//   thread is blocked or terminated -- a switch the scheduler must perform,
//   and the arm that carries the great majority of the identical-frame saves.
//   `_PREEMPT` names the `need_resched` arm, where the thread was running and
//   the switch is discretionary. The split is read off the same boolean the
//   gate itself computes, not a second predicate.

/// Userspace-frame saves admitted by the `need_resched` arm.
///
/// GDB: `print DISPATCH_SAVE_REASON_USER_PREEMPT`
#[no_mangle]
pub static DISPATCH_SAVE_REASON_USER_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_SAVE_REASON_USER_PREEMPT",
    "userspace context save, need_resched arm (#772)",
);

/// Userspace-frame saves admitted by the blocked/terminated arm.
///
/// GDB: `print DISPATCH_SAVE_REASON_USER_MANDATORY`
#[no_mangle]
pub static DISPATCH_SAVE_REASON_USER_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_SAVE_REASON_USER_MANDATORY",
    "userspace context save, blocked/terminated arm (#772)",
);

/// Blocked-in-syscall kernel-frame saves admitted by the `need_resched` arm.
///
/// This is the save side of the identical-RIP/RSP proxy, on the arm the
/// refusal guards.
///
/// GDB: `print DISPATCH_SAVE_REASON_KERNEL_BLOCKED_PREEMPT`
#[no_mangle]
pub static DISPATCH_SAVE_REASON_KERNEL_BLOCKED_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_SAVE_REASON_KERNEL_BLOCKED_PREEMPT",
    "blocked-in-syscall kernel context save, need_resched arm (#772)",
);

/// Blocked-in-syscall kernel-frame saves admitted by the blocked/terminated
/// arm.
///
/// This is the save side of the identical-RIP/RSP proxy, on the arm the
/// refusal is conjoined out of.
///
/// GDB: `print DISPATCH_SAVE_REASON_KERNEL_BLOCKED_MANDATORY`
#[no_mangle]
pub static DISPATCH_SAVE_REASON_KERNEL_BLOCKED_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_SAVE_REASON_KERNEL_BLOCKED_MANDATORY",
    "blocked-in-syscall kernel context save, blocked/terminated arm (#772)",
);

/// Pure-kthread saves admitted by the `need_resched` arm.
///
/// GDB: `print DISPATCH_SAVE_REASON_KTHREAD_PREEMPT`
#[no_mangle]
pub static DISPATCH_SAVE_REASON_KTHREAD_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_SAVE_REASON_KTHREAD_PREEMPT",
    "kthread context save, need_resched arm (#772)",
);

/// Pure-kthread saves admitted by the blocked/terminated arm.
///
/// GDB: `print DISPATCH_SAVE_REASON_KTHREAD_MANDATORY`
#[no_mangle]
pub static DISPATCH_SAVE_REASON_KTHREAD_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_SAVE_REASON_KTHREAD_MANDATORY",
    "kthread context save, blocked/terminated arm (#772)",
);

/// No-progress subset of `DISPATCH_SAVE_REASON_USER_PREEMPT`.
///
/// GDB: `print DISPATCH_NOPROGRESS_SAVE_USER_PREEMPT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_SAVE_USER_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_SAVE_USER_PREEMPT",
    "no-progress userspace save, need_resched arm (#772)",
);

/// No-progress subset of `DISPATCH_SAVE_REASON_USER_MANDATORY`.
///
/// GDB: `print DISPATCH_NOPROGRESS_SAVE_USER_MANDATORY`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_SAVE_USER_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_SAVE_USER_MANDATORY",
    "no-progress userspace save, blocked/terminated arm (#772)",
);

/// No-progress subset of `DISPATCH_SAVE_REASON_KERNEL_BLOCKED_PREEMPT`.
///
/// GDB: `print DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_PREEMPT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_PREEMPT",
    "no-progress blocked-in-syscall save, need_resched arm (#772)",
);

/// No-progress subset of `DISPATCH_SAVE_REASON_KERNEL_BLOCKED_MANDATORY`.
///
/// GDB: `print DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_MANDATORY`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_MANDATORY",
    "no-progress blocked-in-syscall save, blocked/terminated arm (#772)",
);

/// No-progress subset of `DISPATCH_SAVE_REASON_KTHREAD_PREEMPT`.
///
/// GDB: `print DISPATCH_NOPROGRESS_SAVE_KTHREAD_PREEMPT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_SAVE_KTHREAD_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_SAVE_KTHREAD_PREEMPT",
    "no-progress kthread save, need_resched arm (#772)",
);

/// No-progress subset of `DISPATCH_SAVE_REASON_KTHREAD_MANDATORY`.
///
/// GDB: `print DISPATCH_NOPROGRESS_SAVE_KTHREAD_MANDATORY`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_SAVE_KTHREAD_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_SAVE_KTHREAD_MANDATORY",
    "no-progress kthread save, blocked/terminated arm (#772)",
);

/// Dispatches `switch_to_thread` rolled back after the save had already run.
///
/// Four arms reach this: the userspace-save failure in
/// `check_need_resched_and_switch`, the TLS-switch failure, the
/// first-userspace-entry abort, and the blocked-in-syscall arm that cannot
/// take the process-manager guard. Each one calls
/// `abort_dispatch_and_resume`, so the outgoing thread was saved and then
/// resumed without ever leaving the CPU; the dispatch mark written when
/// `switch_to_thread` returns then names that thread at the frame it is about
/// to re-enter. The per-site breakdown is in the trace event's flags.
///
/// GDB: `print DISPATCH_SWITCH_ROLLED_BACK`
#[no_mangle]
pub static DISPATCH_SWITCH_ROLLED_BACK: TraceCounter = TraceCounter::new(
    "DISPATCH_SWITCH_ROLLED_BACK",
    "dispatch rolled back inside switch_to_thread (#772)",
);

/// Dispatches `switch_to_thread` redirected to the idle loop.
///
/// Ten arms reach this: the unpublished-dispatch refusal, the missing-CR3
/// refusal and the signal-termination arm on both the blocked-in-syscall and
/// the userspace restore paths, the userspace restore error, the
/// already-terminated-after-delivery arm, and the two signal-termination arms
/// in `check_and_deliver_signals_for_current_thread`, which the no-switch
/// return arm at gate 4 reaches. The per-site breakdown is in the trace
/// event's flags.
///
/// GDB: `print DISPATCH_SWITCH_IDLE_REDIRECT`
#[no_mangle]
pub static DISPATCH_SWITCH_IDLE_REDIRECT: TraceCounter = TraceCounter::new(
    "DISPATCH_SWITCH_IDLE_REDIRECT",
    "dispatch redirected to idle inside switch_to_thread (#772)",
);

/// Exception handlers that redirected the faulting thread to the idle loop.
///
/// The page-fault and general-protection-fault handlers
/// (`kernel/src/interrupts.rs`) call `switch_to_idle()` and rewrite the
/// exception frame themselves, outside `check_need_resched_and_switch`. They
/// replace the per-CPU current thread without saving a context and without
/// touching the dispatch mark.
///
// claim-lint:ok: 17 of 17 abandon sites enumerated by grep in this slot --
// 13 switch_to_idle() call sites on x86 plus 4 abort_dispatch_and_resume().
/// One further site does the same and is NOT instrumented:
/// `kernel/src/syscall/handler.rs:689`, which on
/// `SignalDeliveryResult::Terminated` calls `set_need_resched()` then
/// `switch_to_idle()` on the syscall vector. `syscall/handler.rs` is a Tier-1
/// file, so it is deliberately left alone (round-2 review, m3). The census is
/// therefore 17 abandon sites, 16 of them instrumented -- 13 `switch_to_idle()`
/// call sites on x86 plus 4 `abort_dispatch_and_resume()` sites, of which
/// handler.rs:689 is the one exception. Its consequence is bounded rather than
/// measured: after it the CPU runs idle, whose tid does not match the mark, so
/// the next `classify_dispatch_progress` returns `Advanced`. The gap can only
/// under-count identical frames, never invent one.
///
/// GDB: `print DISPATCH_EXC_IDLE_REDIRECT`
#[no_mangle]
pub static DISPATCH_EXC_IDLE_REDIRECT: TraceCounter = TraceCounter::new(
    "DISPATCH_EXC_IDLE_REDIRECT",
    "exception handler redirected a thread to idle (#772)",
);

/// Interrupt-return calls that returned at the `PREEMPT_ACTIVE` gate.
///
/// `kernel/src/syscall/entry.asm` sets `PREEMPT_ACTIVE` (bit 28 of `gs:[32]`)
/// at `:110`, two instructions before its
/// `call check_need_resched_and_switch` at `:124`, and clears it again at
/// `:223`, after that call has returned. The 1 syscall-return call site
/// therefore finds the bit set and returns at this gate without reaching
/// `schedule()`. This counter is what makes the claim measurable rather than
/// merely read off the assembly.
///
/// GDB: `print DISPATCH_GATE_PREEMPT_ACTIVE`
#[no_mangle]
pub static DISPATCH_GATE_PREEMPT_ACTIVE: TraceCounter = TraceCounter::new(
    "DISPATCH_GATE_PREEMPT_ACTIVE",
    "interrupt-return call returned at the PREEMPT_ACTIVE gate (#772)",
);

// =============================================================================
// Boot Test Counters (BTRT feature)
// =============================================================================

/// Total boot tests recorded.
///
/// GDB: `print BOOT_TEST_TOTAL`
#[cfg(feature = "btrt")]
#[no_mangle]
pub static BOOT_TEST_TOTAL: TraceCounter =
    TraceCounter::new("BOOT_TEST_TOTAL", "Total boot tests recorded");

/// Total boot tests passed.
///
/// GDB: `print BOOT_TEST_PASS_TOTAL`
#[cfg(feature = "btrt")]
#[no_mangle]
pub static BOOT_TEST_PASS_TOTAL: TraceCounter =
    TraceCounter::new("BOOT_TEST_PASS_TOTAL", "Total boot tests passed");

/// Total boot tests failed.
///
/// GDB: `print BOOT_TEST_FAIL_TOTAL`
#[cfg(feature = "btrt")]
#[no_mangle]
pub static BOOT_TEST_FAIL_TOTAL: TraceCounter =
    TraceCounter::new("BOOT_TEST_FAIL_TOTAL", "Total boot tests failed");

/// Total boot tests skipped.
///
/// GDB: `print BOOT_TEST_SKIP_TOTAL`
#[cfg(feature = "btrt")]
#[no_mangle]
pub static BOOT_TEST_SKIP_TOTAL: TraceCounter =
    TraceCounter::new("BOOT_TEST_SKIP_TOTAL", "Total boot tests skipped");

// =============================================================================
// Wait-loop park census (#772)
// =============================================================================
//
// The park side of the split below, so it can be audited rather than assumed.
// `WAIT_LOOP_PARK_TOTAL` is bumped once per call of
// `per_cpu::note_wait_loop_park`;
// `WAIT_LOOP_PARK_SKIPPED` counts the subset that did NOT reach a thread's
// `wait_loop_iters` -- per-CPU data not yet initialised, or no thread
// installed on this CPU. The difference is the population the
// REVISIT/ZERO_ITER split is computed over. A SKIPPED that is not near 0 in a
// steady-state boot is a finding about the park side, not about #772.
//
// Plain relaxed globals, deliberately NOT `TraceCounter`s:
// `TraceCounter::increment` resolves its per-CPU slot through
// `current_cpu_id()`, which on x86 is `mov reg, gs:[0]` -- the same
// uninstalled-GS-base read the park path's `PER_CPU_INITIALIZED` guard exists
// to avoid. A census that must count the parks that guard refuses therefore
// cannot itself be a per-CPU counter. These are whole-machine totals, which is
// why they carry no `_CPU0` suffix when the GDB driver reads them.
//
// Census only, on both arches. No control flow reads either value.
//
// GDB: `print WAIT_LOOP_PARK_TOTAL`, `print WAIT_LOOP_PARK_SKIPPED`
#[no_mangle]
pub static WAIT_LOOP_PARK_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Parks that did not reach a thread's `wait_loop_iters` (#772).
///
/// See `WAIT_LOOP_PARK_TOTAL` above.
#[no_mangle]
pub static WAIT_LOOP_PARK_SKIPPED: AtomicU64 = AtomicU64::new(0);

/// Record one park, before the per-CPU guard that may refuse it.
#[inline(always)]
pub fn note_park_total() {
    WAIT_LOOP_PARK_TOTAL.fetch_add(1, Ordering::Relaxed);
}

/// Record one park that could not be attributed to a thread.
#[inline(always)]
pub fn note_park_skipped() {
    WAIT_LOOP_PARK_SKIPPED.fetch_add(1, Ordering::Relaxed);
}

// The 21 counters below split each identical-frame save recorded above by
// whether the saved thread parked again between the dispatch and the save
// (#772, R113). `Thread::wait_loop_iters` counts parks on the shared halt
// primitive; the dispatch mark stamps it at dispatch and the save site reads it
// again, so an identical RIP/RSP splits three ways:
//
// * `_REVISIT`  -- the count advanced: the thread went round its wait loop and
//   re-parked on the same halt, so the dispatch retired instructions.
// * `_ZERO_ITER` -- the count is unchanged: the thread is sitting on the very
//   park it was dispatched to, having retired no instructions.
// * `_ITERS_UNKNOWN` -- the count could not be attributed: no thread is
//   installed on this CPU, the installed thread is not the one being saved, or
//   the count read below the stamp. Reported rather than folded into either
//   answer, so the three sum to the `DISPATCH_NOPROGRESS_SAVE_*` total for
//   the same reason.
//
// This is what the DIAG document asked for and could not supply: it recorded a
// revisit and a zero-instruction dispatch identically
// (docs/planning/green-program/sockets/772-DIAG-2026-09-03.md, "Report:
// confidence"). Census only -- no control flow reads any of these.
// claim-lint:ok: docs/planning/green-program/sockets/772-DIAG-2026-09-03.md
/// Identical-frame saves, park count advanced (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_REVISIT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_REVISIT: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_REVISIT",
    "identical-frame saves, park count advanced (#772 R113)",
);

/// Identical-frame saves, park count unchanged (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ZERO_ITER`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ZERO_ITER: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_ZERO_ITER",
    "identical-frame saves, park count unchanged (#772 R113)",
);

/// Identical-frame saves, park count unattributable (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ITERS_UNKNOWN`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ITERS_UNKNOWN: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_ITERS_UNKNOWN",
    "identical-frame saves, park count unattributable (#772 R113)",
);

/// Revisit: userspace save, need_resched arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_REVISIT_USER_PREEMPT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_REVISIT_USER_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_REVISIT_USER_PREEMPT",
    "revisit: userspace save, need_resched arm (#772 R113)",
);

/// Zero-iteration: userspace save, need_resched arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ZERO_ITER_USER_PREEMPT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ZERO_ITER_USER_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_ZERO_ITER_USER_PREEMPT",
    "zero-iteration: userspace save, need_resched arm (#772 R113)",
);

/// Unattributable: userspace save, need_resched arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ITERS_UNKNOWN_USER_PREEMPT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ITERS_UNKNOWN_USER_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_ITERS_UNKNOWN_USER_PREEMPT",
    "unattributable: userspace save, need_resched arm (#772 R113)",
);

/// Revisit: userspace save, blocked/terminated arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_REVISIT_USER_MANDATORY`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_REVISIT_USER_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_REVISIT_USER_MANDATORY",
    "revisit: userspace save, blocked/terminated arm (#772 R113)",
);

/// Zero-iteration: userspace save, blocked/terminated arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ZERO_ITER_USER_MANDATORY`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ZERO_ITER_USER_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_ZERO_ITER_USER_MANDATORY",
    "zero-iteration: userspace save, blocked/terminated arm (#772 R113)",
);

/// Unattributable: userspace save, blocked/terminated arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ITERS_UNKNOWN_USER_MANDATORY`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ITERS_UNKNOWN_USER_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_ITERS_UNKNOWN_USER_MANDATORY",
    "unattributable: userspace save, blocked/terminated arm (#772 R113)",
);

/// Revisit: blocked-in-syscall save, need_resched arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_REVISIT_KERNEL_BLOCKED_PREEMPT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_REVISIT_KERNEL_BLOCKED_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_REVISIT_KERNEL_BLOCKED_PREEMPT",
    "revisit: blocked-in-syscall save, need_resched arm (#772 R113)",
);

/// Zero-iteration: blocked-in-syscall save, need_resched arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ZERO_ITER_KERNEL_BLOCKED_PREEMPT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ZERO_ITER_KERNEL_BLOCKED_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_ZERO_ITER_KERNEL_BLOCKED_PREEMPT",
    "zero-iteration: blocked-in-syscall save, need_resched arm (#772 R113)",
);

/// Unattributable: blocked-in-syscall save, need_resched arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KERNEL_BLOCKED_PREEMPT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KERNEL_BLOCKED_PREEMPT: TraceCounter =
    TraceCounter::new(
        "DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KERNEL_BLOCKED_PREEMPT",
        "unattributable: blocked-in-syscall save, need_resched arm (#772 R113)",
    );

/// Revisit: blocked-in-syscall save, blocked/terminated arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_REVISIT_KERNEL_BLOCKED_MANDATORY`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_REVISIT_KERNEL_BLOCKED_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_REVISIT_KERNEL_BLOCKED_MANDATORY",
    "revisit: blocked-in-syscall save, blocked/terminated arm (#772 R113)",
);

/// Zero-iteration: blocked-in-syscall save, blocked/terminated arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ZERO_ITER_KERNEL_BLOCKED_MANDATORY`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ZERO_ITER_KERNEL_BLOCKED_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_ZERO_ITER_KERNEL_BLOCKED_MANDATORY",
    "zero-iteration: blocked-in-syscall save, blocked/terminated arm (#772 R113)",
);

/// Unattributable: blocked-in-syscall save, blocked/terminated arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KERNEL_BLOCKED_MANDATORY`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KERNEL_BLOCKED_MANDATORY: TraceCounter =
    TraceCounter::new(
        "DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KERNEL_BLOCKED_MANDATORY",
        "unattributable: blocked-in-syscall save, blocked/terminated arm (#772 R113)",
    );

/// Revisit: kthread save, need_resched arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_REVISIT_KTHREAD_PREEMPT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_REVISIT_KTHREAD_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_REVISIT_KTHREAD_PREEMPT",
    "revisit: kthread save, need_resched arm (#772 R113)",
);

/// Zero-iteration: kthread save, need_resched arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ZERO_ITER_KTHREAD_PREEMPT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ZERO_ITER_KTHREAD_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_ZERO_ITER_KTHREAD_PREEMPT",
    "zero-iteration: kthread save, need_resched arm (#772 R113)",
);

/// Unattributable: kthread save, need_resched arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KTHREAD_PREEMPT`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KTHREAD_PREEMPT: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KTHREAD_PREEMPT",
    "unattributable: kthread save, need_resched arm (#772 R113)",
);

/// Revisit: kthread save, blocked/terminated arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_REVISIT_KTHREAD_MANDATORY`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_REVISIT_KTHREAD_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_REVISIT_KTHREAD_MANDATORY",
    "revisit: kthread save, blocked/terminated arm (#772 R113)",
);

/// Zero-iteration: kthread save, blocked/terminated arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ZERO_ITER_KTHREAD_MANDATORY`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ZERO_ITER_KTHREAD_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_ZERO_ITER_KTHREAD_MANDATORY",
    "zero-iteration: kthread save, blocked/terminated arm (#772 R113)",
);

/// Unattributable: kthread save, blocked/terminated arm (#772 R113)
///
/// GDB: `print DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KTHREAD_MANDATORY`
#[no_mangle]
pub static DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KTHREAD_MANDATORY: TraceCounter = TraceCounter::new(
    "DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KTHREAD_MANDATORY",
    "unattributable: kthread save, blocked/terminated arm (#772 R113)",
);

/// The 37 counters the #772 dispatch save census defines, in one place.
///
/// Registration walks this array, so a counter added above without being
/// listed here is not registered, and a counter listed here that is removed
/// above does not compile.
pub static DISPATCH_SAVE_CENSUS_COUNTERS: [&TraceCounter; 37] = [
    &DISPATCH_SAVE_REASON_USER_PREEMPT,
    &DISPATCH_SAVE_REASON_USER_MANDATORY,
    &DISPATCH_SAVE_REASON_KERNEL_BLOCKED_PREEMPT,
    &DISPATCH_SAVE_REASON_KERNEL_BLOCKED_MANDATORY,
    &DISPATCH_SAVE_REASON_KTHREAD_PREEMPT,
    &DISPATCH_SAVE_REASON_KTHREAD_MANDATORY,
    &DISPATCH_NOPROGRESS_SAVE_USER_PREEMPT,
    &DISPATCH_NOPROGRESS_SAVE_USER_MANDATORY,
    &DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_PREEMPT,
    &DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_MANDATORY,
    &DISPATCH_NOPROGRESS_SAVE_KTHREAD_PREEMPT,
    &DISPATCH_NOPROGRESS_SAVE_KTHREAD_MANDATORY,
    &DISPATCH_SWITCH_ROLLED_BACK,
    &DISPATCH_SWITCH_IDLE_REDIRECT,
    &DISPATCH_EXC_IDLE_REDIRECT,
    &DISPATCH_GATE_PREEMPT_ACTIVE,
    &DISPATCH_NOPROGRESS_REVISIT,
    &DISPATCH_NOPROGRESS_ZERO_ITER,
    &DISPATCH_NOPROGRESS_ITERS_UNKNOWN,
    &DISPATCH_NOPROGRESS_REVISIT_USER_PREEMPT,
    &DISPATCH_NOPROGRESS_ZERO_ITER_USER_PREEMPT,
    &DISPATCH_NOPROGRESS_ITERS_UNKNOWN_USER_PREEMPT,
    &DISPATCH_NOPROGRESS_REVISIT_USER_MANDATORY,
    &DISPATCH_NOPROGRESS_ZERO_ITER_USER_MANDATORY,
    &DISPATCH_NOPROGRESS_ITERS_UNKNOWN_USER_MANDATORY,
    &DISPATCH_NOPROGRESS_REVISIT_KERNEL_BLOCKED_PREEMPT,
    &DISPATCH_NOPROGRESS_ZERO_ITER_KERNEL_BLOCKED_PREEMPT,
    &DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KERNEL_BLOCKED_PREEMPT,
    &DISPATCH_NOPROGRESS_REVISIT_KERNEL_BLOCKED_MANDATORY,
    &DISPATCH_NOPROGRESS_ZERO_ITER_KERNEL_BLOCKED_MANDATORY,
    &DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KERNEL_BLOCKED_MANDATORY,
    &DISPATCH_NOPROGRESS_REVISIT_KTHREAD_PREEMPT,
    &DISPATCH_NOPROGRESS_ZERO_ITER_KTHREAD_PREEMPT,
    &DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KTHREAD_PREEMPT,
    &DISPATCH_NOPROGRESS_REVISIT_KTHREAD_MANDATORY,
    &DISPATCH_NOPROGRESS_ZERO_ITER_KTHREAD_MANDATORY,
    &DISPATCH_NOPROGRESS_ITERS_UNKNOWN_KTHREAD_MANDATORY,
];

// =============================================================================
// Initialization
// =============================================================================

/// Initialize all built-in counters.
///
/// Registers counters with the global registry for enumeration and lookup.
pub fn init() {
    register_counter(&SYSCALL_TOTAL);
    register_counter(&IRQ_TOTAL);
    register_counter(&CTX_SWITCH_TOTAL);
    register_counter(&TIMER_TICK_TOTAL);
    register_counter(&FORK_TOTAL);
    register_counter(&EXEC_TOTAL);
    register_counter(&COW_FAULT_TOTAL);
    register_counter(&IDLE_TICK_TOTAL);
    register_counter(&GPU_BYTES_UPLOADED);
    register_counter(&GPU_FULL_UPLOADS);
    register_counter(&GPU_PARTIAL_UPLOADS);
    register_counter(&NET_RX_BUDGET_EXHAUSTED);
    register_counter(&NET_PCI_IRQ_RAISED_NETRX);
    register_counter(&GIC_SPI55_ACK_TOTAL);
    register_counter(&RECV_WAIT_STILL_BLOCKED_TRUE);
    register_counter(&RECV_WAIT_STILL_BLOCKED_FALSE);
    register_counter(&DISPATCH_NO_PROGRESS);
    register_counter(&DISPATCH_KERNEL_RESTORE_TOTAL);
    // #772 diagnostics (R111/R112). Registered with the same capacity
    // assertion the teardown provider uses: `register_counter` already panics
    // on a full registry, so this assert is a second, named guard rather than
    // the only one.
    for counter in DISPATCH_SAVE_CENSUS_COUNTERS {
        assert!(
            register_counter(counter).is_some(),
            "trace counter registry overflow while registering #772 dispatch counters"
        );
    }

    #[cfg(feature = "btrt")]
    {
        register_counter(&BOOT_TEST_TOTAL);
        register_counter(&BOOT_TEST_PASS_TOTAL);
        register_counter(&BOOT_TEST_FAIL_TOTAL);
        register_counter(&BOOT_TEST_SKIP_TOTAL);
    }

    log::info!(
        "Tracing counters initialized: SYSCALL_TOTAL, IRQ_TOTAL, CTX_SWITCH_TOTAL, TIMER_TICK_TOTAL, FORK_TOTAL, EXEC_TOTAL, COW_FAULT_TOTAL"
    );
}

// =============================================================================
// Convenience Functions
// =============================================================================

/// Increment the syscall counter.
///
/// This is an inline function for use in the syscall hot path.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_syscall() {
    SYSCALL_TOTAL.increment();
}

/// Increment the interrupt counter.
///
/// This is an inline function for use in interrupt handlers.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_irq() {
    IRQ_TOTAL.increment();
}

/// Increment the GIC SPI 55 acknowledgement counter.
#[inline(always)]
pub fn count_gic_spi55_ack() {
    crate::trace_count!(GIC_SPI55_ACK_TOTAL);
}

/// Increment the context switch counter.
///
/// This is an inline function for use in the scheduler.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_ctx_switch() {
    CTX_SWITCH_TOTAL.increment();
}

/// Increment the timer tick counter.
///
/// This is an inline function for use in the timer handler.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_timer_tick() {
    TIMER_TICK_TOTAL.increment();
}

/// Increment the fork counter.
///
/// This is an inline function for use in the fork path.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_fork() {
    FORK_TOTAL.increment();
}

/// Increment the exec counter.
///
/// This is an inline function for use in the exec path.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_exec() {
    EXEC_TOTAL.increment();
}

/// Increment the CoW fault counter.
///
/// This is an inline function for use in the CoW fault handler.
/// Compiles to a single atomic add instruction.
#[inline(always)]
pub fn count_cow_fault() {
    COW_FAULT_TOTAL.increment();
}

/// Get all counter values as a summary.
///
/// Returns a tuple of (syscall_total, irq_total, ctx_switch_total, timer_tick_total).
pub fn get_all_counters() -> (u64, u64, u64, u64) {
    (
        SYSCALL_TOTAL.aggregate(),
        IRQ_TOTAL.aggregate(),
        CTX_SWITCH_TOTAL.aggregate(),
        TIMER_TICK_TOTAL.aggregate(),
    )
}

/// Get process-related counter values.
///
/// Returns a tuple of (fork_total, exec_total, cow_fault_total).
pub fn get_process_counters() -> (u64, u64, u64) {
    (
        FORK_TOTAL.aggregate(),
        EXEC_TOTAL.aggregate(),
        COW_FAULT_TOTAL.aggregate(),
    )
}

/// Reset all built-in counters to zero.
///
/// This is not atomic across counters - some increments may be
/// recorded between individual counter resets.
pub fn reset_all() {
    SYSCALL_TOTAL.reset();
    IRQ_TOTAL.reset();
    CTX_SWITCH_TOTAL.reset();
    TIMER_TICK_TOTAL.reset();
    FORK_TOTAL.reset();
    EXEC_TOTAL.reset();
    COW_FAULT_TOTAL.reset();
}
