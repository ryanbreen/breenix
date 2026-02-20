//! ARM64 Timer Interrupt Handler
//!
//! This module provides the timer interrupt handler for ARM64, integrating
//! with the scheduler for preemptive multitasking.
//!
//! The ARM64 Generic Timer (CNTP_EL1 or CNTV_EL0) provides periodic interrupts.
//! Unlike x86_64 which uses the PIC/APIC, ARM64 uses the GIC (Generic Interrupt
//! Controller) to route timer interrupts.
//!
//! Timer Interrupt Flow:
//! 1. Timer fires (IRQ 27 = virtual timer PPI)
//! 2. GIC routes interrupt to handle_irq()
//! 3. handle_irq() calls timer_interrupt_handler()
//! 4. Handler updates time, checks quantum, sets need_resched
//! 5. On exception return, check need_resched and perform context switch if needed

use crate::task::scheduler;
use crate::tracing::providers::irq::trace_timer_tick;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Virtual timer interrupt ID (PPI 27)
pub const TIMER_IRQ: u32 = 27;

/// Time quantum in timer ticks (10 ticks = ~50ms at 200Hz)
const TIME_QUANTUM: u32 = 10;

/// Default timer ticks per interrupt (fallback for 24MHz clock)
/// This value is overwritten at init() with the dynamically calculated value
const DEFAULT_TICKS_PER_INTERRUPT: u64 = 120_000; // For 24MHz clock = ~5ms

/// Target timer frequency in Hz (200 Hz = 5ms per interrupt)
const TARGET_TIMER_HZ: u64 = 200;

/// Dynamically calculated ticks per interrupt based on actual timer frequency
/// Set during init() and used by the interrupt handler for consistent timing
static TICKS_PER_INTERRUPT: AtomicU64 = AtomicU64::new(DEFAULT_TICKS_PER_INTERRUPT);

/// Per-CPU time quantum counters.
/// Each CPU decrements its own quantum independently.
static CURRENT_QUANTUM: [AtomicU32; crate::arch_impl::aarch64::constants::MAX_CPUS] = [
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
];

/// Whether the timer is initialized
static TIMER_INITIALIZED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Total timer interrupt count (for frequency verification)
static TIMER_INTERRUPT_COUNT: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "boot_tests")]
static RESET_QUANTUM_CALL_COUNT: AtomicU64 = AtomicU64::new(0);

/// Interval for printing timer count (every N interrupts for frequency verification)
/// Printing on every interrupt adds overhead; reduce frequency for more accurate measurement
/// At 200 Hz: print interval 200 = print once per second
const TIMER_COUNT_PRINT_INTERVAL: u64 = 200;

// ─── Soft Lockup Detector ────────────────────────────────────────────────────
//
// Detects when no context switch has occurred for LOCKUP_THRESHOLD_TICKS timer
// interrupts (~5 seconds at 1000 Hz). When triggered, dumps diagnostic state to
// serial using lock-free raw_serial_str(). Fires once per stall, resets when
// context switches resume.

/// Threshold in timer ticks before declaring a soft lockup (5 seconds at 200 Hz)
const LOCKUP_THRESHOLD_TICKS: u64 = 200 * 5;

/// Last observed context switch count (CPU 0 only)
static WATCHDOG_LAST_CTX_SWITCH: AtomicU64 = AtomicU64::new(0);

/// Last observed syscall count (CPU 0 only, tracks system liveness)
static WATCHDOG_LAST_SYSCALL: AtomicU64 = AtomicU64::new(0);

/// Timer tick when progress was last observed (ctx switch OR syscall)
static WATCHDOG_LAST_PROGRESS_TICK: AtomicU64 = AtomicU64::new(0);

/// Whether we've already reported a lockup (avoid spamming serial)
static WATCHDOG_REPORTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Initialize the timer interrupt system
///
/// Sets up the virtual timer to fire periodically for scheduling.
pub fn init() {
    if TIMER_INITIALIZED.load(Ordering::Relaxed) {
        return;
    }

    // Get the timer frequency from hardware
    let freq = super::timer::frequency_hz();
    log::info!("ARM64 timer interrupt init: frequency = {} Hz", freq);

    // Calculate ticks per interrupt for target Hz scheduling rate
    // For 62.5 MHz clock: 62_500_000 / 200 = 312_500 ticks
    // For 24 MHz clock: 24_000_000 / 200 = 120_000 ticks
    let ticks_per_interrupt = if freq > 0 {
        freq / TARGET_TIMER_HZ
    } else {
        DEFAULT_TICKS_PER_INTERRUPT
    };

    // Store the calculated value for use in the interrupt handler
    TICKS_PER_INTERRUPT.store(ticks_per_interrupt, Ordering::Release);

    crate::serial_println!(
        "[timer] Timer configured for ~{} Hz ({} ticks per interrupt)",
        TARGET_TIMER_HZ,
        ticks_per_interrupt
    );

    // Arm the timer for the first interrupt
    arm_timer(ticks_per_interrupt);

    // Enable the timer interrupt in the GIC
    use crate::arch_impl::aarch64::gic;
    use crate::arch_impl::traits::InterruptController;
    gic::Gicv2::enable_irq(TIMER_IRQ as u8);

    TIMER_INITIALIZED.store(true, Ordering::Release);
    log::info!("ARM64 timer interrupt initialized");
}

/// Arm the virtual timer to fire after `ticks` counter increments
fn arm_timer(ticks: u64) {
    unsafe {
        // Set countdown value (CNTV_TVAL_EL0)
        core::arch::asm!(
            "msr cntv_tval_el0, {}",
            in(reg) ticks,
            options(nomem, nostack)
        );
        // Enable timer with interrupts (CNTV_CTL_EL0)
        // Bit 0 = ENABLE, Bit 1 = IMASK (0 = interrupt enabled)
        core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
    }
}

/// Timer interrupt handler - minimal work in interrupt context
///
/// This is called from handle_irq() when IRQ 27 (virtual timer) fires.
/// Each CPU fires its own timer (PPI 27 is per-CPU). The handler:
/// 1. Re-arms the timer for the next interrupt
/// 2. CPU 0 only: updates global wall clock time
/// 3. CPU 0 only: polls keyboard input
/// 4. All CPUs: decrements per-CPU time quantum
/// 5. CPU 0 only: sets need_resched if quantum expired (Phase 2: only CPU 0 schedules)
#[no_mangle]
pub extern "C" fn timer_interrupt_handler() {
    // Enter IRQ context (increment HARDIRQ count)
    crate::per_cpu_aarch64::irq_enter();

    let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;

    // Re-arm the timer for the next interrupt using the dynamically calculated value
    arm_timer(TICKS_PER_INTERRUPT.load(Ordering::Relaxed));

    // CPU 0 only: update global wall clock time (single atomic operation)
    if cpu_id == 0 {
        crate::time::timer_interrupt();
    }

    // Trace timer tick (lock-free counter + optional event recording)
    trace_timer_tick(crate::time::get_ticks());

    // Increment timer interrupt counter (used for debugging when needed)
    let _count = TIMER_INTERRUPT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;

    // CPU 0 only: poll input devices (single-device, not safe from multiple CPUs)
    if cpu_id == 0 {
        poll_keyboard_to_stdin();
        // Poll XHCI USB HID events (needed when PCI interrupt routing isn't available)
        crate::drivers::usb::xhci::poll_hid_events();
    }

    // CPU 0 only: soft lockup detector
    if cpu_id == 0 {
        check_soft_lockup(_count);
    }

    // CPU 0 only: periodic heartbeat every 2000 CPU 0 ticks (~10 seconds at 200Hz)
    // Uses a dedicated CPU 0 counter to avoid non-determinism from the global counter.
    if cpu_id == 0 {
        static CPU0_TICK: AtomicU64 = AtomicU64::new(0);
        let cpu0_tick = CPU0_TICK.fetch_add(1, Ordering::Relaxed) + 1;
        if cpu0_tick % 2000 == 0 {
            raw_serial_str(b"\n[HB t=");
            print_timer_count_decimal(cpu0_tick / TARGET_TIMER_HZ);
            raw_serial_str(b"s ctx=");
            print_timer_count_decimal(crate::task::scheduler::context_switch_count());
            raw_serial_str(b" sys=");
            print_timer_count_decimal(crate::tracing::providers::counters::SYSCALL_TOTAL.aggregate());
            raw_serial_str(b" flush=");
            print_timer_count_decimal(crate::syscall::graphics::FB_FLUSH_COUNT.load(Ordering::Relaxed));
            raw_serial_str(b" fork=");
            print_timer_count_decimal(crate::tracing::providers::counters::FORK_TOTAL.aggregate());
            raw_serial_str(b" exec=");
            print_timer_count_decimal(crate::tracing::providers::counters::EXEC_TOTAL.aggregate());
            raw_serial_str(b" cow=");
            print_timer_count_decimal(crate::memory::cow_stats::TOTAL_FAULTS.load(Ordering::Relaxed));
            raw_serial_str(b" cowpm=");
            print_timer_count_decimal(crate::memory::cow_stats::MANAGER_PATH.load(Ordering::Relaxed));
            raw_serial_str(b" cowcp=");
            print_timer_count_decimal(crate::memory::cow_stats::PAGES_COPIED.load(Ordering::Relaxed));
            raw_serial_str(b" cowso=");
            print_timer_count_decimal(crate::memory::cow_stats::SOLE_OWNER_OPT.load(Ordering::Relaxed));
            raw_serial_str(b" pty=");
            print_timer_count_decimal(crate::tty::pty::pair::PTY_SLAVE_BYTES_WRITTEN.load(Ordering::Relaxed));
            raw_serial_str(b" tgn=");
            print_timer_count_decimal(crate::arch_impl::aarch64::context_switch::TTBR_PROCESS_GONE_COUNT.load(Ordering::Relaxed));
            raw_serial_str(b" tlb=");
            print_timer_count_decimal(crate::arch_impl::aarch64::context_switch::TTBR_PM_LOCK_BUSY_COUNT.load(Ordering::Relaxed));
            raw_serial_str(b" up=");
            print_timer_count_decimal(crate::drivers::usb::xhci::POLL_COUNT.load(Ordering::Relaxed));
            raw_serial_str(b" ue=");
            print_timer_count_decimal(crate::drivers::usb::xhci::EVENT_COUNT.load(Ordering::Relaxed));
            raw_serial_str(b" uk=");
            print_timer_count_decimal(crate::drivers::usb::xhci::KBD_EVENT_COUNT.load(Ordering::Relaxed));
            raw_serial_str(b" xo=");
            print_timer_count_decimal(crate::drivers::usb::xhci::XFER_OTHER_COUNT.load(Ordering::Relaxed));
            raw_serial_str(b" psc=");
            print_timer_count_decimal(crate::drivers::usb::xhci::PSC_COUNT.load(Ordering::Relaxed));
            raw_serial_str(b" r=");
            print_timer_count_decimal(crate::drivers::usb::xhci::EP0_RESET_COUNT.load(Ordering::Relaxed));
            raw_serial_str(b" rf=");
            print_timer_count_decimal(crate::drivers::usb::xhci::EP0_RESET_FAIL_COUNT.load(Ordering::Relaxed));
            raw_serial_str(b" ps=");
            print_timer_count_decimal(crate::drivers::usb::xhci::EP0_PENDING_STUCK_COUNT.load(Ordering::Relaxed));
            raw_serial_str(b"]\n");
        }
    }

    // Decrement per-CPU quantum and check for reschedule
    let quantum_idx = if cpu_id < crate::arch_impl::aarch64::constants::MAX_CPUS {
        cpu_id
    } else {
        0
    };
    let old_quantum = CURRENT_QUANTUM[quantum_idx].fetch_sub(1, Ordering::Relaxed);
    if old_quantum <= 1 {
        // Quantum expired - request reschedule (all CPUs participate)
        scheduler::set_need_resched();
        CURRENT_QUANTUM[quantum_idx].store(TIME_QUANTUM, Ordering::Relaxed);
    }

    // IDLE CPU FAST PATH: If this CPU is running its idle thread, always
    // request reschedule on every timer tick. This ensures that threads
    // added to the ready queue (by unblock() on another CPU) are picked up
    // within one timer tick (~5ms) instead of waiting for a full quantum
    // (~50ms). The scheduling decision quickly returns None if the ready
    // queue is empty, so the overhead is negligible for idle CPUs.
    if scheduler::is_cpu_idle(cpu_id) {
        scheduler::set_need_resched();
    }

    // Exit IRQ context (decrement HARDIRQ count)
    crate::per_cpu_aarch64::irq_exit();
}

/// Raw serial output - no locks, single char for debugging (used by print_timer_count)
#[inline(always)]
fn raw_serial_char(c: u8) {
    crate::serial_aarch64::raw_serial_char(c);
}

/// Raw serial output - write a string without locks for debugging
#[allow(dead_code)] // Debug utility, kept for future use
#[inline(always)]
fn raw_serial_str(s: &[u8]) {
    crate::serial_aarch64::raw_serial_str(s);
}

/// Print a decimal number using raw serial output
/// Used by timer interrupt handler to output [TIMER_COUNT:N] markers
#[allow(dead_code)] // Debug utility, kept for future use
fn print_timer_count_decimal(count: u64) {
    if count == 0 {
        raw_serial_char(b'0');
    } else {
        // Convert to decimal digits (max u64 is 20 digits)
        let mut digits = [0u8; 20];
        let mut n = count;
        let mut i = 0;
        while n > 0 {
            digits[i] = (n % 10) as u8 + b'0';
            n /= 10;
            i += 1;
        }
        // Print in reverse order
        while i > 0 {
            i -= 1;
            raw_serial_char(digits[i]);
        }
    }
}

/// Check for soft lockup (CPU 0 only, called from timer interrupt).
///
/// Compares the current context switch count against the last observed value.
/// If no context switches have occurred for LOCKUP_THRESHOLD_TICKS timer
/// interrupts (~5 seconds), checks whether this is a real stall:
/// - If the scheduler lock is held → likely deadlock, report immediately
/// - If the ready queue is empty → single runnable thread, not a lockup
/// - If the ready queue has threads → scheduler is stuck, report
fn check_soft_lockup(current_tick: u64) {
    let ctx_count = crate::task::scheduler::context_switch_count();
    let last_ctx = WATCHDOG_LAST_CTX_SWITCH.load(Ordering::Relaxed);

    // Check context switch progress
    let ctx_progressed = ctx_count != last_ctx;
    if ctx_progressed {
        WATCHDOG_LAST_CTX_SWITCH.store(ctx_count, Ordering::Relaxed);
    }

    // Check syscall progress (system is alive if syscalls are being made)
    let syscall_count = crate::tracing::providers::counters::SYSCALL_TOTAL.aggregate();
    let last_syscall = WATCHDOG_LAST_SYSCALL.load(Ordering::Relaxed);
    let syscall_progressed = syscall_count != last_syscall;
    if syscall_progressed {
        WATCHDOG_LAST_SYSCALL.store(syscall_count, Ordering::Relaxed);
    }

    if ctx_progressed || syscall_progressed {
        // System is making progress — update baseline
        WATCHDOG_LAST_PROGRESS_TICK.store(current_tick, Ordering::Relaxed);
        WATCHDOG_REPORTED.store(false, Ordering::Relaxed);
        return;
    }

    // No progress on either metric — check how long
    let stall_start = WATCHDOG_LAST_PROGRESS_TICK.load(Ordering::Relaxed);
    if stall_start == 0 {
        // Not yet initialized
        WATCHDOG_LAST_PROGRESS_TICK.store(current_tick, Ordering::Relaxed);
        return;
    }

    let stall_ticks = current_tick.wrapping_sub(stall_start);
    if stall_ticks >= LOCKUP_THRESHOLD_TICKS && !WATCHDOG_REPORTED.load(Ordering::Relaxed) {
        // Always report stall — even if ready_queue is empty, userspace threads
        // might be stuck in BlockedOnTimer/Blocked state without being woken.
        // The dump includes per-thread state so we can diagnose the exact issue.
        WATCHDOG_REPORTED.store(true, Ordering::Relaxed);
        dump_lockup_state(stall_ticks);
    }
}

/// Dump diagnostic state when a soft lockup is detected.
/// Uses only lock-free serial output — safe to call from interrupt context.
fn dump_lockup_state(stall_ticks: u64) {
    raw_serial_str(b"\n\n!!! SOFT LOCKUP DETECTED !!!\n");
    raw_serial_str(b"No context switch for ~");
    print_timer_count_decimal(stall_ticks / TARGET_TIMER_HZ);
    raw_serial_str(b" seconds (");
    print_timer_count_decimal(stall_ticks);
    raw_serial_str(b" ticks)\n");

    // Try to get scheduler info without blocking (try_lock)
    // If the scheduler lock is held, that itself is diagnostic info
    raw_serial_str(b"Scheduler lock: ");
    // We use the global SCHEDULER directly via the public with_scheduler_try_lock helper
    if let Some(info) = crate::task::scheduler::try_dump_state() {
        raw_serial_str(b"acquired\n");
        raw_serial_str(b"  Ready queue length: ");
        print_timer_count_decimal(info.ready_queue_len);
        raw_serial_str(b"\n  Total threads: ");
        print_timer_count_decimal(info.total_threads);
        raw_serial_str(b"\n  Blocked threads: ");
        print_timer_count_decimal(info.blocked_count);

        // Per-CPU current/previous threads
        raw_serial_str(b"\n  Per-CPU state:\n");
        for cpu in 0..4usize {
            raw_serial_str(b"    CPU ");
            raw_serial_char(b'0' + cpu as u8);
            raw_serial_str(b": current=");
            print_timer_count_decimal(info.per_cpu_current[cpu]);
            raw_serial_str(b" previous=");
            print_timer_count_decimal(info.per_cpu_previous[cpu]);
            raw_serial_str(b"\n");
        }

        // Ready queue contents
        raw_serial_str(b"  Ready queue: [");
        for (i, &tid) in info.ready_queue_ids.iter().enumerate() {
            if i > 0 { raw_serial_str(b", "); }
            print_timer_count_decimal(tid);
        }
        raw_serial_str(b"]\n");

        // Per-thread state (state names: R=Ready, X=Running, B=Blocked, S=Signal, C=ChildExit, T=Timer, D=Terminated)
        raw_serial_str(b"  Threads:\n");
        for t in &info.threads {
            raw_serial_str(b"    tid=");
            print_timer_count_decimal(t.id);
            raw_serial_str(b" state=");
            let state_ch = match t.state {
                0 => b'R', // Ready
                1 => b'X', // Running
                2 => b'B', // Blocked
                3 => b'S', // BlockedOnSignal
                4 => b'C', // BlockedOnChildExit
                5 => b'T', // BlockedOnTimer
                6 => b'D', // Terminated
                _ => b'?',
            };
            raw_serial_char(state_ch);
            if t.blocked_in_syscall { raw_serial_str(b" bis"); }
            if t.has_wake_time { raw_serial_str(b" wt"); }
            if t.privilege == 1 { raw_serial_str(b" user"); }
            raw_serial_str(b"\n");
        }
    } else {
        raw_serial_str(b"HELD (possible deadlock)\n");
    }

    // Try to get process manager info
    raw_serial_str(b"Process manager lock: ");
    if let Some(info) = crate::process::try_dump_state() {
        raw_serial_str(b"acquired\n");
        raw_serial_str(b"  Total processes: ");
        print_timer_count_decimal(info.total_processes);
        raw_serial_str(b"\n  Running: ");
        print_timer_count_decimal(info.running_count);
        raw_serial_str(b"\n  Blocked: ");
        print_timer_count_decimal(info.blocked_count);
        raw_serial_str(b"\n");
        // Dump individual process names and states
        for p in &info.processes {
            raw_serial_str(b"  PID ");
            print_timer_count_decimal(p.pid);
            raw_serial_str(b" [");
            raw_serial_str(p.state_str.as_bytes());
            raw_serial_str(b"] ");
            raw_serial_str(p.name.as_bytes());
            raw_serial_str(b"\n");
        }
    } else {
        raw_serial_str(b"HELD (possible deadlock)\n");
    }

    // Dump trace counters (lock-free atomics, always safe from interrupt context)
    dump_trace_counters();

    raw_serial_str(b"!!! END SOFT LOCKUP DUMP !!!\n\n");
}

/// Dump trace counter values using lock-free serial output.
/// Safe to call from interrupt context since TraceCounter uses per-CPU atomics.
fn dump_trace_counters() {
    use crate::tracing::providers::counters;

    raw_serial_str(b"Trace counters:\n");

    raw_serial_str(b"  SYSCALL_TOTAL:    ");
    print_timer_count_decimal(counters::SYSCALL_TOTAL.aggregate());
    raw_serial_str(b"\n  IRQ_TOTAL:        ");
    print_timer_count_decimal(counters::IRQ_TOTAL.aggregate());
    raw_serial_str(b"\n  CTX_SWITCH_TOTAL: ");
    print_timer_count_decimal(counters::CTX_SWITCH_TOTAL.aggregate());
    raw_serial_str(b"\n  TIMER_TICK_TOTAL: ");
    print_timer_count_decimal(counters::TIMER_TICK_TOTAL.aggregate());
    raw_serial_str(b"\n  FORK_TOTAL:       ");
    print_timer_count_decimal(counters::FORK_TOTAL.aggregate());
    raw_serial_str(b"\n  EXEC_TOTAL:       ");
    print_timer_count_decimal(counters::EXEC_TOTAL.aggregate());
    raw_serial_str(b"\n  Global ticks:     ");
    print_timer_count_decimal(crate::time::get_ticks());
    raw_serial_str(b"\n  Timer IRQ count:  ");
    print_timer_count_decimal(TIMER_INTERRUPT_COUNT.load(Ordering::Relaxed));
    raw_serial_str(b"\n");
}

/// Poll VirtIO keyboard and push characters to TTY
///
/// This routes keyboard input through the TTY subsystem for:
/// 1. Echo (so you can see what you type)
/// 2. Line discipline processing (backspace, Ctrl-C, etc.)
/// 3. Proper stdin delivery to userspace via TTY read
fn poll_keyboard_to_stdin() {
    use crate::drivers::virtio::input_mmio::{self, event_type};

    // Track shift state across calls
    static SHIFT_PRESSED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);

    if !input_mmio::is_initialized() {
        return;
    }

    for event in input_mmio::poll_events() {
        if event.event_type == event_type::EV_KEY {
            let keycode = event.code;
            let pressed = event.value != 0;

            // Track modifier key state
            if input_mmio::is_shift(keycode) {
                SHIFT_PRESSED.store(pressed, core::sync::atomic::Ordering::Relaxed);
                continue;
            }

            // Only process key presses and repeats (not releases)
            if pressed {
                // Generate VT100 escape sequences for special keys
                // (F-keys, arrows, Home, End, Delete)
                if let Some(seq) = input_mmio::keycode_to_escape_seq(keycode) {
                    for &b in seq {
                        if !crate::tty::push_char_nonblock(b) {
                            crate::ipc::stdin::push_byte_from_irq(b);
                        }
                    }
                    continue;
                }

                let shift = SHIFT_PRESSED.load(core::sync::atomic::Ordering::Relaxed);
                if let Some(c) = input_mmio::keycode_to_char(keycode, shift) {
                    // Route through TTY for echo and line discipline processing.
                    // This is the non-blocking version safe for interrupt context.
                    if !crate::tty::push_char_nonblock(c as u8) {
                        // TTY busy - fall back to raw stdin buffer
                        // (no echo, but at least input isn't lost)
                        crate::ipc::stdin::push_byte_from_irq(c as u8);
                    }
                }
            }
        }
    }
}

/// Reset the quantum counter for the current CPU (called when switching threads)
pub fn reset_quantum() {
    #[cfg(feature = "boot_tests")]
    RESET_QUANTUM_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
    let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let idx = if cpu_id < crate::arch_impl::aarch64::constants::MAX_CPUS {
        cpu_id
    } else {
        0
    };
    CURRENT_QUANTUM[idx].store(TIME_QUANTUM, Ordering::Relaxed);
}

/// Get reset_quantum() call count for tests.
#[cfg(feature = "boot_tests")]
pub fn reset_quantum_call_count() -> u64 {
    RESET_QUANTUM_CALL_COUNT.load(Ordering::SeqCst)
}

/// Reset reset_quantum() call count for tests.
#[cfg(feature = "boot_tests")]
pub fn reset_quantum_call_count_reset() {
    RESET_QUANTUM_CALL_COUNT.store(0, Ordering::SeqCst);
}

/// Initialize the timer on a secondary CPU.
///
/// Each CPU has its own virtual timer (PPI 27 is per-CPU). The distributor
/// does not need re-configuration for PPIs. We just arm the timer and enable
/// the interrupt in this CPU's GIC interface.
pub fn init_secondary() {
    // Arm the timer with the same interval as CPU 0
    let ticks = TICKS_PER_INTERRUPT.load(Ordering::Relaxed);
    arm_timer(ticks);

    // Enable the virtual timer PPI in the GIC for this CPU.
    // PPIs are per-CPU, but ISENABLER0 (IRQs 0-31) is banked per-CPU,
    // so writing to it from this CPU enables it for this CPU.
    use crate::arch_impl::traits::InterruptController;
    crate::arch_impl::aarch64::gic::Gicv2::enable_irq(TIMER_IRQ as u8);
}

/// Check if the timer is initialized
pub fn is_initialized() -> bool {
    TIMER_INITIALIZED.load(Ordering::Acquire)
}

/// Get the current CPU's quantum value (for debugging)
#[allow(dead_code)]
pub fn current_quantum() -> u32 {
    let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let idx = if cpu_id < crate::arch_impl::aarch64::constants::MAX_CPUS {
        cpu_id
    } else {
        0
    };
    CURRENT_QUANTUM[idx].load(Ordering::Relaxed)
}
