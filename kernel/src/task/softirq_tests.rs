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
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

static ITERATION_COUNT: AtomicU32 = AtomicU32::new(0);
static KSOFTIRQD_PROCESSED: AtomicBool = AtomicBool::new(false);
/// The CPU whose ksoftirqd was observed satisfying the daemon-identity
/// assertion, recorded by the handler at the moment it observed it.
///
/// `usize::MAX` means "not observed". The serial marker reports this rather
/// than the CPU the phase was pinned to: an evidence line that interpolates the
/// pin prints the same daemon name whichever daemon actually did the work.
static KSOFTIRQD_OBSERVED_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);
const TARGET_ITERATIONS: u32 = 25;

pub fn test_softirq() {
    run_softirq_tests();
}

/// Run Test 7's ksoftirqd-verification phase in a context the scheduler can
/// switch away from, and report which daemon satisfied it.
///
/// On aarch64 the boot CPU holds a boot-long preemption pin (main_aarch64.rs,
/// the `preempt_disable()` whose own comment says it exists so the scheduler
/// cannot switch the boot CPU to ksoftirqd), and the identity it runs under is
/// CPU 0's idle task. The phase raises a softirq, lets `do_softirq` hit its
/// iteration limit, and then waits for the LOCAL ksoftirqd to drain the rest --
/// which needs a CPU that can be scheduled away from, and needs the raise and
/// the drain to happen on the same CPU because the pending bitmap is per-CPU.
/// So the phase runs in a kthread pinned to a secondary CPU, spawned after the
/// scheduler is live and joined from boot. The 7 other tests do not move: 1-6
/// and 8 keep running in the boot context on both arches, and both of Test 7's
/// assertions are unchanged.
/// claim-lint:ok: the split is pinned by
/// tests/aarch64_testing_profile_structure.rs
#[cfg(target_arch = "aarch64")]
fn run_ksoftirqd_deferral_phase() {
    let online = crate::arch_impl::aarch64::smp::cpus_online() as usize;
    assert!(
        online > 1,
        "aarch64 softirq daemon test requires a schedulable secondary CPU"
    );
    let test_cpu = 1;
    let handle = crate::task::kthread::kthread_run_on_cpu(
        ksoftirqd_deferral_phase,
        "softirq-test/1",
        test_cpu,
    )
    .unwrap_or_else(|error| panic!("failed to spawn softirq test kthread: {:?}", error));
    crate::task::kthread::kthread_join(&handle)
        .unwrap_or_else(|error| panic!("failed to join softirq test kthread: {:?}", error));
    report_ksoftirqd_deferral_phase();
}

#[cfg(not(target_arch = "aarch64"))]
fn run_ksoftirqd_deferral_phase() {
    ksoftirqd_deferral_phase();
    report_ksoftirqd_deferral_phase();
}

/// The one serial line the #562 acceptance reads, naming the daemon that was
/// actually observed rather than the one the phase was aimed at.
fn report_ksoftirqd_deferral_phase() {
    let observed = KSOFTIRQD_OBSERVED_CPU.load(Ordering::SeqCst);
    assert_ne!(
        observed,
        usize::MAX,
        "no ksoftirqd was observed draining the deferred softirq work"
    );
    crate::serial_println!(
        "SOFTIRQ_TEST: iteration limit passed ({} total iterations, ksoftirqd/{})",
        ITERATION_COUNT.load(Ordering::SeqCst),
        observed
    );
}

fn run_softirq_tests() {
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

    // Test 7 runs in its own function: on aarch64 the scheduler must be
    // able to switch away from whoever runs it, which the boot CPU cannot
    // do. See `run_ksoftirqd_deferral_phase`.
    run_ksoftirqd_deferral_phase();

    // Test 8: Verify ksoftirqd is initialized (keep original test)
    log::info!("SOFTIRQ_TEST: Verifying ksoftirqd is initialized...");
    assert!(
        crate::task::softirqd::is_initialized(),
        "Softirq subsystem should be initialized"
    );
    log::info!("SOFTIRQ_TEST: ksoftirqd verification passed");

    log::info!("SOFTIRQ_TEST: all tests passed");

    // CRITICAL: Restore the real network softirq handler!
    // The tests above registered test handlers that override the real ones.
    // Without this, network packets won't be processed after the tests.
    crate::net::register_net_softirq();
    log::info!("SOFTIRQ_TEST: Restored network softirq handler");

    log::info!("=== SOFTIRQ TEST: Completed ===");
}

/// Test 7: the iteration limit, and the deferral to the LOCAL ksoftirqd.
///
/// Both assertions are the originals: the bounded `do_softirq()` call must stop
/// at `MAX_SOFTIRQ_RESTART`, and the daemon-identity flag must be set by the
/// ksoftirqd that owns the CPU whose pending bitmap the workload raised.
fn ksoftirqd_deferral_phase() {
    // Test 7: Iteration limit and ksoftirqd deferral
    // A handler that re-raises itself will exceed MAX_SOFTIRQ_RESTART=10
    // After the limit, remaining work is deferred to ksoftirqd
    log::info!("SOFTIRQ_TEST: Testing iteration limit and ksoftirqd deferral...");
    const MAX_SOFTIRQ_RESTART: u32 = crate::task::softirqd::MAX_SOFTIRQ_RESTART;
    // TARGET must exceed 2 * MAX_SOFTIRQ_RESTART to force ksoftirqd involvement:
    // - test's do_softirq(): 10 iterations, hits limit, wakes ksoftirqd
    // - timer's irq_exit do_softirq(): 10 more iterations, hits limit, wakes ksoftirqd
    // - ksoftirqd finally runs and processes remaining iterations
    ITERATION_COUNT.store(0, Ordering::SeqCst);
    KSOFTIRQD_PROCESSED.store(false, Ordering::SeqCst);

    // Get ksoftirqd tid for later comparison
    let ksoftirqd_tid = crate::task::softirqd::ksoftirqd_tid();

    // Register a handler that re-raises itself until TARGET_ITERATIONS
    // and tracks whether it runs in ksoftirqd context
    register_softirq_handler(SoftirqType::Tasklet, |_softirq| {
        let count = ITERATION_COUNT.fetch_add(1, Ordering::SeqCst);

        // Check if we're running in ksoftirqd context.
        //
        // The identity read is the LOCK-FREE per-CPU one. This handler is
        // dispatched from `do_softirq` on the IRQ-exit path, and
        // `scheduler::current_thread_id()` takes the global SCHEDULER spin lock
        // -- a bare lock from interrupt context, the shape #609 was about, and
        // the one the #562 RCA flagged as needing re-examination the moment
        // this workload moved onto a preemptible CPU with peers contending that
        // lock. The per-CPU pointer is written by the context-switch path on the
        // CPU that owns it and read here on that same CPU.
        if let Some(ksoft_tid) = crate::task::softirqd::ksoftirqd_tid() {
            if let Some(current_tid) = crate::per_cpu::current_thread_id_lock_free() {
                if current_tid == ksoft_tid {
                    KSOFTIRQD_PROCESSED.store(true, Ordering::SeqCst);
                    KSOFTIRQD_OBSERVED_CPU
                        .store(crate::task::softirqd::current_cpu_id(), Ordering::SeqCst);
                }
            }
        }

        if count + 1 < TARGET_ITERATIONS {
            raise_softirq(SoftirqType::Tasklet);
        }
    });

    // Measure only this bounded call. Once ksoftirqd is correctly local and
    // runnable, it may finish the workload immediately after do_softirq returns;
    // masking IRQs through the snapshot keeps that valid concurrency from being
    // attributed to the bounded call itself.
    let count_after_dosoftirq = crate::arch_without_interrupts(|| {
        raise_softirq(SoftirqType::Tasklet);
        do_softirq();
        ITERATION_COUNT.load(Ordering::SeqCst)
    });

    // The directly invoked do_softirq() must stop at the iteration limit.
    assert!(
        count_after_dosoftirq <= MAX_SOFTIRQ_RESTART,
        "do_softirq() should stop at iteration limit: got {}, expected <= {}",
        count_after_dosoftirq,
        MAX_SOFTIRQ_RESTART
    );
    log::info!(
        "SOFTIRQ_TEST: After do_softirq(): {} iterations (limit={})",
        count_after_dosoftirq,
        MAX_SOFTIRQ_RESTART
    );

    // If ksoftirqd is working, remaining iterations will be processed
    // Give ksoftirqd some time to process deferred softirqs
    // Note: Remaining work can be processed by either:
    // 1. irq_exit's do_softirq() during timer interrupts, or
    // 2. ksoftirqd when it gets scheduled
    // We use yield_current() + HLT to ensure scheduling opportunities
    unsafe {
        arch_enable_interrupts();
    } // Ensure interrupts are enabled
    for _ in 0..100 {
        // yield_current() sets need_resched, ensuring a context switch opportunity
        // when the next interrupt occurs
        crate::task::scheduler::yield_current();
        arch_halt();
        let current = ITERATION_COUNT.load(Ordering::SeqCst);
        if current >= TARGET_ITERATIONS {
            break;
        }
    }

    let final_count = ITERATION_COUNT.load(Ordering::SeqCst);
    assert!(
        final_count >= TARGET_ITERATIONS,
        "ksoftirqd should have processed deferred softirqs: got {} iterations, expected {}",
        final_count,
        TARGET_ITERATIONS
    );

    // Verify ksoftirqd specifically processed the deferred work
    let ksoftirqd_did_work = KSOFTIRQD_PROCESSED.load(Ordering::SeqCst);
    assert!(
        ksoftirqd_did_work,
        "ksoftirqd should have processed deferred softirqs (tid={:?})",
        ksoftirqd_tid
    );
    log::info!(
        "SOFTIRQ_TEST: iteration limit passed ({} total iterations, ksoftirqd verified)",
        final_count
    );
}
