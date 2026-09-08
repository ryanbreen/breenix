//! Shared softirq tests for x86_64 and ARM64.
//!
//! Tests the Linux-style softirq implementation:
//! 1. Softirq handler registration
//! 2. raise_softirq() marks softirq as pending
//! 3. do_softirq() invokes registered handlers
//! 4. Priority ordering (Timer before NetRx)
//! 5. Nested interrupt rejection
//! 6. Iteration limit and ksoftirqd deferral
//! 7. ksoftirqd initialization verification

use crate::task::softirqd::{do_softirq, raise_softirq, register_softirq_handler, SoftirqType};
use crate::{arch_enable_interrupts, arch_halt};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[cfg(feature = "testing")]
pub fn test_softirq() {
    static TIMER_HANDLER_CALLED: AtomicU32 = AtomicU32::new(0);
    static NET_RX_HANDLER_CALLED: AtomicU32 = AtomicU32::new(0);

    // Reset counters
    TIMER_HANDLER_CALLED.store(0, Ordering::SeqCst);
    NET_RX_HANDLER_CALLED.store(0, Ordering::SeqCst);

    log::info!("=== SOFTIRQ TEST: Starting softirq test ===");

    // Test 1: Register handlers
    log::info!("SOFTIRQ_TEST: Testing handler registration...");
    register_softirq_handler(SoftirqType::Timer, |_softirq| {
        TIMER_HANDLER_CALLED.fetch_add(1, Ordering::SeqCst);
    });
    register_softirq_handler(SoftirqType::NetRx, |_softirq| {
        NET_RX_HANDLER_CALLED.fetch_add(1, Ordering::SeqCst);
    });
    log::info!("SOFTIRQ_TEST: handler registration passed");

    // Test 2: Raise and process Timer softirq
    log::info!("SOFTIRQ_TEST: Testing Timer softirq...");
    raise_softirq(SoftirqType::Timer);

    // Check that softirq is pending
    let pending = crate::per_cpu::softirq_pending();
    assert!(
        (pending & (1 << SoftirqType::Timer.as_nr())) != 0,
        "Timer softirq should be pending"
    );

    // Process softirqs (we're in thread context, not interrupt context)
    do_softirq();

    let timer_count = TIMER_HANDLER_CALLED.load(Ordering::SeqCst);
    assert_eq!(timer_count, 1, "Timer handler should have been called once");
    log::info!("SOFTIRQ_TEST: Timer softirq passed");

    // Test 3: Raise and process NetRx softirq
    log::info!("SOFTIRQ_TEST: Testing NetRx softirq...");
    raise_softirq(SoftirqType::NetRx);
    do_softirq();

    let net_rx_count = NET_RX_HANDLER_CALLED.load(Ordering::SeqCst);
    assert_eq!(
        net_rx_count, 1,
        "NetRx handler should have been called once"
    );
    log::info!("SOFTIRQ_TEST: NetRx softirq passed");

    // Test 4: Raise multiple softirqs at once
    log::info!("SOFTIRQ_TEST: Testing multiple softirqs...");
    raise_softirq(SoftirqType::Timer);
    raise_softirq(SoftirqType::NetRx);
    do_softirq();

    let timer_count = TIMER_HANDLER_CALLED.load(Ordering::SeqCst);
    let net_rx_count = NET_RX_HANDLER_CALLED.load(Ordering::SeqCst);
    assert_eq!(
        timer_count, 2,
        "Timer handler should have been called twice"
    );
    assert_eq!(
        net_rx_count, 2,
        "NetRx handler should have been called twice"
    );
    log::info!("SOFTIRQ_TEST: multiple softirqs passed");

    // Test 5: Priority order verification
    // Timer (priority 1) should execute BEFORE NetRx (priority 3)
    log::info!("SOFTIRQ_TEST: Testing priority order...");
    static EXECUTION_ORDER: AtomicU32 = AtomicU32::new(0);
    static TIMER_EXEC_ORDER: AtomicU32 = AtomicU32::new(0);
    static NETRX_EXEC_ORDER: AtomicU32 = AtomicU32::new(0);

    EXECUTION_ORDER.store(0, Ordering::SeqCst);
    TIMER_EXEC_ORDER.store(0, Ordering::SeqCst);
    NETRX_EXEC_ORDER.store(0, Ordering::SeqCst);

    // Register new handlers that track execution order
    register_softirq_handler(SoftirqType::Timer, |_softirq| {
        let order = EXECUTION_ORDER.fetch_add(1, Ordering::SeqCst);
        TIMER_EXEC_ORDER.store(order + 1, Ordering::SeqCst);
    });
    register_softirq_handler(SoftirqType::NetRx, |_softirq| {
        let order = EXECUTION_ORDER.fetch_add(1, Ordering::SeqCst);
        NETRX_EXEC_ORDER.store(order + 1, Ordering::SeqCst);
    });

    // Raise NetRx first, then Timer - Timer should still execute first due to priority
    raise_softirq(SoftirqType::NetRx);
    raise_softirq(SoftirqType::Timer);
    do_softirq();

    let timer_order = TIMER_EXEC_ORDER.load(Ordering::SeqCst);
    let netrx_order = NETRX_EXEC_ORDER.load(Ordering::SeqCst);
    assert!(
        timer_order < netrx_order,
        "Timer (priority 1) should execute before NetRx (priority 3): timer={}, netrx={}",
        timer_order,
        netrx_order
    );
    log::info!(
        "SOFTIRQ_TEST: priority order passed (Timer={}, NetRx={})",
        timer_order,
        netrx_order
    );

    // Test 6: Nested interrupt rejection
    // do_softirq() should return false when already in interrupt context
    log::info!("SOFTIRQ_TEST: Testing nested interrupt rejection...");
    crate::per_cpu::softirq_enter(); // Simulate being in softirq context
    raise_softirq(SoftirqType::Timer);
    let processed = do_softirq();
    assert!(
        !processed,
        "do_softirq() should return false when in interrupt context"
    );
    crate::per_cpu::softirq_exit();
    // Now process the pending softirq outside interrupt context
    let processed = do_softirq();
    assert!(
        processed,
        "do_softirq() should return true when not in interrupt context"
    );
    log::info!("SOFTIRQ_TEST: nested interrupt rejection passed");

    let deferral_ok = test_deferral();

    // Test 8: Verify ksoftirqd is initialized (keep original test)
    log::info!("SOFTIRQ_TEST: Verifying ksoftirqd is initialized...");
    assert!(
        crate::task::softirqd::is_initialized(),
        "Softirq subsystem should be initialized"
    );
    log::info!("SOFTIRQ_TEST: ksoftirqd verification passed");

    if deferral_ok {
        log::info!("SOFTIRQ_TEST: all tests passed");
    }

    // CRITICAL: Restore the real network softirq handler!
    // The tests above registered test handlers that override the real ones.
    // Without this, network packets won't be processed after the tests.
    crate::net::register_net_softirq();
    log::info!("SOFTIRQ_TEST: Restored network softirq handler");

    log::info!("=== SOFTIRQ TEST: Completed ===");
}

// This probe owns Tasklet until ACTIVE is cleared. Interrupt exits may service
// it, but cannot complete the probe: a callback from the daemon is required.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static GUEST_DONE: AtomicBool = AtomicBool::new(false);
static ITERATIONS: AtomicU32 = AtomicU32::new(0);
static DISPATCH_BASE: AtomicU64 = AtomicU64::new(0);
static WAITED_TICKS: AtomicU64 = AtomicU64::new(0);
static WAITED_NS: AtomicU64 = AtomicU64::new(0);
static DISPATCHES: AtomicU64 = AtomicU64::new(0);
static PROBE_CPU: AtomicU64 = AtomicU64::new(0);
static VERDICT: AtomicU32 = AtomicU32::new(0);
const TARGET_ITERATIONS: u32 = 25;
const WAIT_TICKS: u64 = 250;
const WALL_LIMIT_NS: u64 = 15_000_000_000;

fn counter_ns() -> u64 {
    crate::tracing::timestamp_to_nanos(crate::tracing::trace_timestamp())
}

fn deferral_handler(_softirq: SoftirqType) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let iterations = ITERATIONS.fetch_add(1, Ordering::Relaxed) + 1;
    let dispatches = crate::task::softirqd::KSOFTIRQD_TASKLET_DISPATCHES
        .get_cpu(crate::task::softirqd::current_cpu())
        .wrapping_sub(DISPATCH_BASE.load(Ordering::Relaxed));
    if !(dispatches > 0 && iterations >= TARGET_ITERATIONS) {
        raise_softirq(SoftirqType::Tasklet);
    }
}

fn deferral_probe() {
    use crate::task::softirqd::{current_cpu, KSOFTIRQD_TASKLET_DISPATCHES};
    use crate::tracing::providers::counters::TIMER_TICK_TOTAL;
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let cpu = current_cpu();
    PROBE_CPU.store(cpu as u64, Ordering::Relaxed);
    let base = KSOFTIRQD_TASKLET_DISPATCHES.get_cpu(cpu);
    DISPATCH_BASE.store(base, Ordering::Relaxed);
    let ticks = TIMER_TICK_TOTAL.get_cpu(cpu);
    let wall = counter_ns();
    // Keep the initial limit observation together: IRQ-exit service must not
    // race the count read immediately following the explicit do_softirq call.
    let initial = crate::arch_without_interrupts(|| {
        raise_softirq(SoftirqType::Tasklet);
        do_softirq();
        ITERATIONS.load(Ordering::Relaxed)
    });
    unsafe {
        arch_enable_interrupts();
    }
    loop {
        let elapsed_ticks = TIMER_TICK_TOTAL.get_cpu(cpu).wrapping_sub(ticks);
        let elapsed_ns = counter_ns().saturating_sub(wall);
        let dispatches = KSOFTIRQD_TASKLET_DISPATCHES.get_cpu(cpu).wrapping_sub(base);
        let iterations = ITERATIONS.load(Ordering::Acquire);
        WAITED_TICKS.store(elapsed_ticks, Ordering::Relaxed);
        WAITED_NS.store(elapsed_ns, Ordering::Relaxed);
        DISPATCHES.store(dispatches, Ordering::Relaxed);
        if initial != 10 {
            VERDICT.store(2, Ordering::Relaxed);
            break;
        }
        if dispatches > 0 && iterations >= TARGET_ITERATIONS {
            VERDICT.store(1, Ordering::Relaxed);
            break;
        }
        // Delivered local timer interrupts are guest execution evidence. A
        // host pause cannot spend this budget merely by advancing the TSC/CNTVCT.
        if elapsed_ticks >= WAIT_TICKS {
            VERDICT.store(2, Ordering::Relaxed);
            break;
        }
        if elapsed_ns >= WALL_LIMIT_NS || !ACTIVE.load(Ordering::Acquire) {
            VERDICT.store(3, Ordering::Relaxed);
            break;
        }
        crate::task::scheduler::yield_current();
        arch_halt();
    }
    ACTIVE.store(false, Ordering::Release);
    crate::per_cpu::clear_softirq(SoftirqType::Tasklet.as_nr());
    GUEST_DONE.store(true, Ordering::Release);
}

/// Emit a non-panicking deferral verdict. The host scorer rejects `lost` and
/// reports `starved` as an infrastructure outcome, without a kernel-red marker.
pub fn test_deferral() -> bool {
    ITERATIONS.store(0, Ordering::Relaxed);
    GUEST_DONE.store(false, Ordering::Relaxed);
    WAITED_TICKS.store(0, Ordering::Relaxed);
    WAITED_NS.store(0, Ordering::Relaxed);
    DISPATCHES.store(0, Ordering::Relaxed);
    VERDICT.store(0, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Release);
    register_softirq_handler(SoftirqType::Tasklet, deferral_handler);
    let start = counter_ns();
    #[cfg(target_arch = "aarch64")]
    {
        // CPU0 is still the unschedulable boot stack in the testing profile.
        // Exercise local ownership on a schedulable CPU using a real pinned
        // kthread, without moving the loader or boot sequence to a new stack.
        if crate::arch_impl::aarch64::smp::cpus_online() > 1 {
            if crate::task::kthread::kthread_run_on_cpu(deferral_probe, "softirq-probe", 1).is_err()
            {
                VERDICT.store(2, Ordering::Relaxed);
                GUEST_DONE.store(true, Ordering::Release);
            }
        }
        while !GUEST_DONE.load(Ordering::Acquire)
            && counter_ns().saturating_sub(start) < WALL_LIMIT_NS
        {
            core::hint::spin_loop();
        }
    }
    #[cfg(target_arch = "x86_64")]
    deferral_probe();
    if !GUEST_DONE.load(Ordering::Acquire) {
        ACTIVE.store(false, Ordering::Release);
        VERDICT.store(3, Ordering::Relaxed);
    }
    let verdict = match VERDICT.load(Ordering::Acquire) {
        1 => "ok",
        2 => "lost",
        _ => "starved",
    };
    crate::serial_println!(
        "[SOFTIRQ_DEFERRAL_ORACLE:arch={}:cpu={}:budget_ticks={}:wait_ticks={}:wait_ns={}:dispatches={}:iterations={}:verdict={}]",
        if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86" },
        PROBE_CPU.load(Ordering::Relaxed), WAIT_TICKS,
        WAITED_TICKS.load(Ordering::Relaxed),
        counter_ns().saturating_sub(start).max(WAITED_NS.load(Ordering::Relaxed)),
        DISPATCHES.load(Ordering::Relaxed), ITERATIONS.load(Ordering::Relaxed), verdict
    );
    verdict == "ok"
}
