//! Feature-gated AArch64 self-tests for ret-dispatch and inline handoff bugs.

#![cfg(feature = "boot_tests")]

#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "ret_zero_pc_oracle",
        feature = "ret_zero_pc_oracle_exec",
        feature = "ret_stack_pc_oracle",
        feature = "ret_floor_oracle",
        feature = "strand_inject_live_outgoing"
    )
))]
use core::sync::atomic::{AtomicU64, Ordering};

#[cfg(all(
    target_arch = "aarch64",
    any(feature = "ret_zero_pc_oracle", feature = "ret_stack_pc_oracle")
))]
use super::scheduler::Scheduler;
#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle_exec"))]
use super::thread::Thread;
#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle"))]
static RET_ARMED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle"))]
static RET_FIRED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle"))]
static RET_VICTIMS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle"))]
static RET_VICTIM_TID: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle"))]
static RET_VICTIM_PROGRESS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle"))]
static RET_FIRE_CPU: AtomicU64 = AtomicU64::new(0);

#[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
static STACK_ARMED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
static STACK_FIRED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
static STACK_VICTIMS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
static STACK_VICTIM_TID: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
static STACK_VICTIM_PROGRESS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
static STACK_FIRE_CPU: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
static STACK_INJECTED_PC: AtomicU64 = AtomicU64::new(0);

#[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
static FLOOR_ARMED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
static FLOOR_FIRED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
static FLOOR_VICTIM_TID: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
static FLOOR_VICTIM_PROGRESS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
static FLOOR_INJECTED_PC: AtomicU64 = AtomicU64::new(0);

#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle_exec"))]
static EXEC_ARMED: AtomicU64 = AtomicU64::new(1);
#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle_exec"))]
static EXEC_FIRED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle_exec"))]
static EXEC_VICTIMS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle_exec"))]
static EXEC_VICTIM_TID: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle_exec"))]
static EXEC_INLINE_LEFT_SET: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle_exec"))]
static EXEC_CLEARED: AtomicU64 = AtomicU64::new(0);

#[cfg(all(target_arch = "aarch64", feature = "strand_inject_live_outgoing"))]
static LIVE_ARMED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "strand_inject_live_outgoing"))]
static LIVE_FIRED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "strand_inject_live_outgoing"))]
static LIVE_OUTGOING: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "strand_inject_live_outgoing"))]
static LIVE_OUTGOING_TID: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "strand_inject_live_outgoing"))]
static LIVE_DRIVER_TID: AtomicU64 = AtomicU64::new(0);

#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle"))]
pub(crate) fn inject_ret_zero_pc_if_armed(sched: &mut Scheduler, thread_id: u64) {
    let victim_tid = RET_VICTIM_TID.load(Ordering::Acquire);
    if victim_tid == 0 || thread_id != victim_tid {
        return;
    }
    if RET_FIRED
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    let Some(thread) = sched.get_thread_mut(thread_id) else {
        RET_FIRED.store(0, Ordering::Release);
        return;
    };
    thread.context.x30 = 0;
    RET_VICTIMS.store(1, Ordering::Release);
    let cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as u64;
    RET_FIRE_CPU.store(cpu, Ordering::Release);

    use crate::arch_impl::aarch64::context_switch::{raw_uart_dec, raw_uart_str};
    raw_uart_str("[RET_ZERO_PC_ORACLE:aarch64:leg=K:FIRED:tid=");
    raw_uart_dec(thread_id);
    raw_uart_str(":cpu=");
    raw_uart_dec(cpu);
    raw_uart_str("]\n");
}

#[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
pub(crate) fn inject_ret_stack_pc_if_armed(sched: &mut Scheduler, thread_id: u64) {
    let victim_tid = STACK_VICTIM_TID.load(Ordering::Acquire);
    if victim_tid == 0 || thread_id != victim_tid {
        return;
    }
    if STACK_FIRED
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    let Some(thread) = sched.get_thread_mut(thread_id) else {
        STACK_FIRED.store(0, Ordering::Release);
        return;
    };
    let injected_pc = thread.context.sp;
    thread.context.x30 = injected_pc;
    STACK_INJECTED_PC.store(injected_pc, Ordering::Release);
    STACK_VICTIMS.store(1, Ordering::Release);
    let cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as u64;
    STACK_FIRE_CPU.store(cpu, Ordering::Release);

    use crate::arch_impl::aarch64::context_switch::{raw_uart_dec, raw_uart_hex, raw_uart_str};
    raw_uart_str("[RET_STACK_PC_ORACLE:aarch64:leg=T:FIRED:tid=");
    raw_uart_dec(thread_id);
    raw_uart_str(":cpu=");
    raw_uart_dec(cpu);
    raw_uart_str(":injected_pc=");
    raw_uart_hex(injected_pc);
    raw_uart_str("]\n");
}

#[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
pub(crate) fn inject_ret_floor_if_armed(thread_id: u64, resume_pc: u64) -> u64 {
    const INJECTED_PC: u64 = 0x0000_0000_0100_0000;

    let victim_tid = FLOOR_VICTIM_TID.load(Ordering::Acquire);
    if victim_tid == 0 || thread_id != victim_tid {
        return resume_pc;
    }
    if FLOOR_FIRED
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return resume_pc;
    }

    FLOOR_INJECTED_PC.store(INJECTED_PC, Ordering::Release);
    use crate::arch_impl::aarch64::context_switch::{raw_uart_dec, raw_uart_hex, raw_uart_str};
    raw_uart_str("[RET_FLOOR_ORACLE:aarch64:leg=F:FIRED:tid=");
    raw_uart_dec(thread_id);
    raw_uart_str(":injected_pc=");
    raw_uart_hex(INJECTED_PC);
    raw_uart_str("]\n");
    INJECTED_PC
}

#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle"))]
pub(crate) fn is_retzero_subject(thread_id: u64) -> bool {
    let victim_tid = RET_VICTIM_TID.load(Ordering::Acquire);
    victim_tid != 0 && thread_id == victim_tid
}

#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle_exec"))]
pub(crate) fn inject_exec_commit_if_armed(thread: &mut Thread) {
    if EXEC_FIRED
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    thread.saved_by_inline_schedule = true;
    thread.has_started = true;
    EXEC_VICTIMS.store(1, Ordering::Release);
    EXEC_VICTIM_TID.store(thread.id(), Ordering::Release);

    use crate::arch_impl::aarch64::context_switch::{raw_uart_dec, raw_uart_str};
    raw_uart_str("[EXEC_COMMIT_DISARM_ORACLE:aarch64:FIRED:tid=");
    raw_uart_dec(thread.id());
    raw_uart_str("]\n");
}

#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle_exec"))]
pub(crate) fn record_exec_commit_inline_state(thread: &Thread) {
    if thread.id() != EXEC_VICTIM_TID.load(Ordering::Acquire) {
        return;
    }
    let inline_left_set = u64::from(thread.saved_by_inline_schedule);
    EXEC_INLINE_LEFT_SET.store(inline_left_set, Ordering::Release);
    EXEC_CLEARED.store(1 - inline_left_set, Ordering::Release);
}

#[cfg(all(target_arch = "aarch64", feature = "strand_inject_live_outgoing"))]
pub(crate) fn note_live_outgoing_armed() {
    LIVE_ARMED.store(1, Ordering::Release);
}

#[cfg(all(target_arch = "aarch64", feature = "strand_inject_live_outgoing"))]
pub(crate) fn note_live_outgoing_fired(outgoing_tid: u64, outgoing_was_idle: bool) {
    LIVE_FIRED.fetch_add(1, Ordering::AcqRel);
    LIVE_OUTGOING_TID.store(outgoing_tid, Ordering::Release);
    if !outgoing_was_idle {
        LIVE_OUTGOING.fetch_add(1, Ordering::AcqRel);
    }
}

#[cfg(all(target_arch = "aarch64", feature = "strand_inject_live_outgoing"))]
#[inline(always)]
pub(crate) fn is_strand_live_driver(thread_id: u64) -> bool {
    thread_id == LIVE_DRIVER_TID.load(Ordering::Acquire)
}

#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle"))]
fn retzero_victim() {
    loop {
        RET_VICTIM_PROGRESS.fetch_add(1, Ordering::AcqRel);
        super::strand_oracle::sleep_sample_period();
    }
}

#[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
fn retstack_victim() {
    loop {
        STACK_VICTIM_PROGRESS.fetch_add(1, Ordering::AcqRel);
        super::scheduler::schedule();
        super::strand_oracle::sleep_sample_period();
    }
}

#[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
fn retfloor_victim() {
    loop {
        FLOOR_VICTIM_PROGRESS.fetch_add(1, Ordering::AcqRel);
        super::scheduler::schedule();
        super::strand_oracle::sleep_sample_period();
    }
}

#[cfg(all(target_arch = "aarch64", feature = "strand_inject_live_outgoing"))]
fn strand_live_driver() {
    let Some(tid) = super::scheduler::current_thread_id() else {
        return;
    };
    LIVE_DRIVER_TID.store(tid, Ordering::Release);

    loop {
        // Enter the handoff while still Running so the outgoing transaction
        // carries an affirmative requeue intent. No timer or other wakeup is
        // pending until this call returns, so an abandoned handoff remains a
        // Ready, unqueued strand for longer than STRAND_DWELL_MS.
        super::scheduler::schedule();
        super::strand_oracle::sleep_sample_period();
    }
}

#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "ret_zero_pc_oracle",
        feature = "ret_zero_pc_oracle_exec",
        feature = "ret_stack_pc_oracle",
        feature = "ret_floor_oracle",
        feature = "strand_inject_live_outgoing"
    )
))]
fn monotonic_now_ms() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds
        .saturating_mul(1_000)
        .saturating_add(nanos / 1_000_000)
}

#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "ret_zero_pc_oracle",
        feature = "ret_zero_pc_oracle_exec",
        feature = "ret_stack_pc_oracle",
        feature = "ret_floor_oracle",
        feature = "strand_inject_live_outgoing"
    )
))]
fn every_compiled_test_fired() -> bool {
    #[cfg(feature = "ret_zero_pc_oracle")]
    if RET_FIRED.load(Ordering::Acquire) == 0 {
        return false;
    }
    #[cfg(feature = "ret_zero_pc_oracle_exec")]
    if EXEC_FIRED.load(Ordering::Acquire) == 0 {
        return false;
    }
    #[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
    if STACK_FIRED.load(Ordering::Acquire) == 0 {
        return false;
    }
    #[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
    if FLOOR_FIRED.load(Ordering::Acquire) == 0 {
        return false;
    }
    #[cfg(feature = "strand_inject_live_outgoing")]
    if LIVE_FIRED.load(Ordering::Acquire) == 0 {
        return false;
    }
    true
}

#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle"))]
fn report_ret_zero_pc() {
    let armed = RET_ARMED.load(Ordering::Acquire);
    let fired = RET_FIRED.load(Ordering::Acquire);
    let victims = RET_VICTIMS.load(Ordering::Acquire);
    let victim_tid = RET_VICTIM_TID.load(Ordering::Acquire);
    let fire_cpu = RET_FIRE_CPU.load(Ordering::Acquire);
    let refused =
        crate::arch_impl::aarch64::context_switch::RET_DISPATCH_REFUSALS.load(Ordering::Acquire);
    let refused_tid =
        crate::arch_impl::aarch64::context_switch::RET_DISPATCH_REFUSED_TID.load(Ordering::Acquire);
    let passed =
        armed == 1 && fired == 1 && victims == 1 && refused >= 1 && refused_tid == victim_tid;
    crate::serial_println!(
        "[RET_ZERO_PC_ORACLE:aarch64:leg=K:armed={}:fired={}:victims={}:victim_tid={}:fire_cpu={}:refused={}:refused_tid={}:{}]",
        armed,
        fired,
        victims,
        victim_tid,
        fire_cpu,
        refused,
        refused_tid,
        if passed { "PASS" } else { "FAIL" },
    );
}

#[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
fn report_ret_stack_pc() {
    let armed = STACK_ARMED.load(Ordering::Acquire);
    let fired = STACK_FIRED.load(Ordering::Acquire);
    let victims = STACK_VICTIMS.load(Ordering::Acquire);
    let victim_tid = STACK_VICTIM_TID.load(Ordering::Acquire);
    let fire_cpu = STACK_FIRE_CPU.load(Ordering::Acquire);
    let injected_pc = STACK_INJECTED_PC.load(Ordering::Acquire);
    let refused =
        crate::arch_impl::aarch64::context_switch::RET_DISPATCH_REFUSALS.load(Ordering::Acquire);
    let refused_tid =
        crate::arch_impl::aarch64::context_switch::RET_DISPATCH_REFUSED_TID.load(Ordering::Acquire);
    let passed =
        armed == 1 && fired == 1 && victims == 1 && refused >= 1 && refused_tid == victim_tid;
    crate::serial_println!(
        "[RET_STACK_PC_ORACLE:aarch64:leg=T:armed={}:fired={}:victims={}:victim_tid={}:fire_cpu={}:injected_pc=0x{:x}:refused={}:refused_tid={}:{}]",
        armed,
        fired,
        victims,
        victim_tid,
        fire_cpu,
        injected_pc,
        refused,
        refused_tid,
        if passed { "PASS" } else { "FAIL" },
    );
}

#[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
fn report_ret_floor() {
    let armed = FLOOR_ARMED.load(Ordering::Acquire);
    let fired = FLOOR_FIRED.load(Ordering::Acquire);
    let victim_tid = FLOOR_VICTIM_TID.load(Ordering::Acquire);
    let injected_pc = FLOOR_INJECTED_PC.load(Ordering::Acquire);
    let passed = armed == 1 && fired == 1;
    crate::serial_println!(
        "[RET_FLOOR_ORACLE:aarch64:leg=F:armed={}:fired={}:victim_tid={}:injected_pc=0x{:x}:{}]",
        armed,
        fired,
        victim_tid,
        injected_pc,
        if passed { "PASS" } else { "FAIL" },
    );
}

#[cfg(all(target_arch = "aarch64", feature = "ret_zero_pc_oracle_exec"))]
fn report_exec_commit() {
    let armed = EXEC_ARMED.load(Ordering::Acquire);
    let fired = EXEC_FIRED.load(Ordering::Acquire);
    let victims = EXEC_VICTIMS.load(Ordering::Acquire);
    let victim_tid = EXEC_VICTIM_TID.load(Ordering::Acquire);
    let inline_left_set = EXEC_INLINE_LEFT_SET.load(Ordering::Acquire);
    let cleared = EXEC_CLEARED.load(Ordering::Acquire);
    let passed = armed == 1 && fired == 1 && victims == 1 && inline_left_set == 0;
    crate::serial_println!(
        "[EXEC_COMMIT_DISARM_ORACLE:aarch64:armed={}:fired={}:victims={}:victim_tid={}:inline_left_set={}:cleared={}:{}]",
        armed,
        fired,
        victims,
        victim_tid,
        inline_left_set,
        cleared,
        if passed { "PASS" } else { "FAIL" },
    );
}

#[cfg(all(target_arch = "aarch64", feature = "strand_inject_live_outgoing"))]
fn report_live_outgoing() {
    let armed = LIVE_ARMED.load(Ordering::Acquire);
    let fired = LIVE_FIRED.load(Ordering::Acquire);
    let live_outgoing = LIVE_OUTGOING.load(Ordering::Acquire);
    let outgoing_tid = LIVE_OUTGOING_TID.load(Ordering::Acquire);
    let stranded = super::strand_oracle::stranded_count();
    let passed = armed == 1 && fired >= 1 && live_outgoing >= 1 && stranded == 0;
    crate::serial_println!(
        "[STRAND_LIVE_OUTGOING_ORACLE:aarch64:armed={}:fired={}:live_outgoing={}:outgoing_tid={}:stranded={}:{}]",
        armed,
        fired,
        live_outgoing,
        outgoing_tid,
        stranded,
        if passed { "PASS" } else { "FAIL" },
    );
}

#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "ret_zero_pc_oracle",
        feature = "ret_zero_pc_oracle_exec",
        feature = "ret_stack_pc_oracle",
        feature = "ret_floor_oracle",
        feature = "strand_inject_live_outgoing"
    )
))]
fn retzero_oracle_thread() {
    #[cfg(feature = "strand_inject_live_outgoing")]
    const SETTLE_MS: u64 = 3_000;
    #[cfg(not(feature = "strand_inject_live_outgoing"))]
    const SETTLE_MS: u64 = 2_000;
    #[cfg(feature = "ret_zero_pc_oracle_exec")]
    const HARD_CAP_MS: u64 = 40_000;
    #[cfg(not(feature = "ret_zero_pc_oracle_exec"))]
    const HARD_CAP_MS: u64 = 14_000;

    let start_ms = monotonic_now_ms();
    let mut settle_start_ms = 0;
    loop {
        let now_ms = monotonic_now_ms();
        if settle_start_ms == 0 && every_compiled_test_fired() {
            settle_start_ms = now_ms;
        }
        let settled = settle_start_ms != 0 && now_ms.saturating_sub(settle_start_ms) >= SETTLE_MS;
        let capped = now_ms.saturating_sub(start_ms) >= HARD_CAP_MS;
        if settled || capped {
            #[cfg(feature = "ret_zero_pc_oracle")]
            report_ret_zero_pc();
            #[cfg(feature = "ret_zero_pc_oracle_exec")]
            report_exec_commit();
            #[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
            report_ret_stack_pc();
            #[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
            report_ret_floor();
            #[cfg(feature = "strand_inject_live_outgoing")]
            report_live_outgoing();
            return;
        }
        super::strand_oracle::sleep_sample_period();
    }
}

/// Start the selected one-shot AArch64 oracles. With no oracle feature enabled,
/// the boot-tests build emits no additional threads or hooks.
pub fn start() {
    #[cfg(all(
        target_arch = "aarch64",
        any(
            feature = "ret_zero_pc_oracle",
            feature = "ret_zero_pc_oracle_exec",
            feature = "ret_stack_pc_oracle",
            feature = "ret_floor_oracle",
            feature = "strand_inject_live_outgoing"
        )
    ))]
    {
        #[cfg(feature = "ret_zero_pc_oracle")]
        if let Ok(victim) = super::kthread::kthread_run(retzero_victim, "retzero-victim") {
            RET_VICTIM_TID.store(victim.tid(), Ordering::Release);
            RET_ARMED.store(1, Ordering::Release);
        }
        #[cfg(all(target_arch = "aarch64", feature = "ret_stack_pc_oracle"))]
        if let Ok(victim) = super::kthread::kthread_run(retstack_victim, "retstack-victim") {
            STACK_VICTIM_TID.store(victim.tid(), Ordering::Release);
            STACK_ARMED.store(1, Ordering::Release);
        }
        #[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
        if let Ok(victim) = super::kthread::kthread_run(retfloor_victim, "retfloor-victim") {
            FLOOR_VICTIM_TID.store(victim.tid(), Ordering::Release);
            FLOOR_ARMED.store(1, Ordering::Release);
        }
        #[cfg(feature = "strand_inject_live_outgoing")]
        let _ = super::kthread::kthread_run(strand_live_driver, "strand-live-driver");
        let _ = super::kthread::kthread_run(retzero_oracle_thread, "retzero-oracle");
    }
}
