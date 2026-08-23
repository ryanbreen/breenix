//! Feature-gated AArch64 self-tests for ret-dispatch and inline handoff bugs.

#![cfg(feature = "boot_tests")]

#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "ret_zero_pc_oracle",
        feature = "ret_zero_pc_oracle_exec",
        feature = "ret_stack_pc_oracle",
        feature = "ret_floor_oracle",
        feature = "strand_inject_live_outgoing",
        feature = "resume_pc_el1_oracle",
        feature = "eret_zero_pc_oracle",
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle",
        feature = "resume_pc_foreign_oracle"
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
#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    ),
    not(feature = "resume_pc_el0_frame_oracle"),
    not(feature = "ret_zero_pc_oracle_exec")
))]
use super::thread::Thread;
#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el1_oracle",
        feature = "eret_zero_pc_oracle",
        all(
            feature = "resume_pc_el0_frame_oracle",
            any(
                feature = "resume_pc_el0_kernel_oracle",
                feature = "resume_pc_el0_tid_oracle"
            )
        )
    )
))]
use crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame;
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
static FLOOR_OPPORTUNITIES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
static FLOOR_VICTIM_TID: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
static FLOOR_VICTIM_PROGRESS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ret_floor_oracle"))]
static FLOOR_INJECTED_PC: AtomicU64 = AtomicU64::new(0);

#[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
static FOREIGN_ARMED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
static FOREIGN_ARM_RAN: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
static FOREIGN_PLANTED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
static FOREIGN_RECORD_CPU: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
static FOREIGN_CANARY_TID: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
static FOREIGN_CANARY_PROGRESS: AtomicU64 = AtomicU64::new(0);

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

#[cfg(all(
    target_arch = "aarch64",
    any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle")
))]
static EL1_ARMED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle")
))]
static EL1_LIVE: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle")
))]
static EL1_OPPORTUNITIES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle")
))]
static EL1_INJECTIONS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle")
))]
static EL1_INJECTED_PC: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle")
))]
static EL1_VICTIM_TID: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle")
))]
static EL1_VICTIM_PROGRESS: AtomicU64 = AtomicU64::new(0);

#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    )
))]
static EL0_ARMED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    )
))]
static EL0_LIVE: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    )
))]
static EL0_OPPORTUNITIES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    )
))]
static EL0_INJECTIONS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    )
))]
static EL0_INJECTED_PC: AtomicU64 = AtomicU64::new(0);
#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    )
))]
static EL0_VICTIM_TID: AtomicU64 = AtomicU64::new(0);

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
    #[cfg(not(feature = "resume_pc_oracle_disarm"))]
    const INJECTED_PC: u64 = 0x0000_0000_0100_0000;

    let victim_tid = FLOOR_VICTIM_TID.load(Ordering::Acquire);
    if victim_tid == 0 || thread_id != victim_tid {
        return resume_pc;
    }
    FLOOR_OPPORTUNITIES.fetch_add(1, Ordering::AcqRel);
    #[cfg(feature = "resume_pc_oracle_disarm")]
    {
        return resume_pc;
    }
    #[cfg(not(feature = "resume_pc_oracle_disarm"))]
    {
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

#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el1_oracle",
        feature = "eret_zero_pc_oracle",
        all(
            feature = "resume_pc_el0_frame_oracle",
            any(
                feature = "resume_pc_el0_kernel_oracle",
                feature = "resume_pc_el0_tid_oracle"
            )
        )
    )
))]
pub(crate) fn inject_el1_frame_resume_pc_if_armed(frame: &mut Aarch64ExceptionFrame) {
    #[cfg(all(
        feature = "resume_pc_el0_frame_oracle",
        any(
            feature = "resume_pc_el0_kernel_oracle",
            feature = "resume_pc_el0_tid_oracle"
        )
    ))]
    if frame.spsr & 0xF == 0 {
        inject_el0_frame_resume_pc_if_armed(frame);
        return;
    }

    #[cfg(any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle"))]
    {
        if EL1_LIVE.load(Ordering::Acquire) != 1 {
            return;
        }
        if frame.spsr & 0xF == 0 {
            return;
        }
        if EL1_INJECTIONS.load(Ordering::Acquire) >= 3 {
            return;
        }

        let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
        let victim_tid = EL1_VICTIM_TID.load(Ordering::Acquire);
        if crate::arch_impl::aarch64::context_switch::last_dispatched_tid(cpu_id)
            != Some(victim_tid)
        {
            return;
        }

        EL1_OPPORTUNITIES.fetch_add(1, Ordering::AcqRel);
        #[cfg(not(feature = "resume_pc_oracle_disarm"))]
        {
            #[cfg(feature = "resume_pc_el1_oracle")]
            let injected = frame as *mut Aarch64ExceptionFrame as u64 + 0x10;
            #[cfg(all(not(feature = "resume_pc_el1_oracle"), feature = "eret_zero_pc_oracle"))]
            let injected = 0;
            #[cfg(feature = "resume_pc_el1_oracle")]
            let leg = "P";
            #[cfg(all(not(feature = "resume_pc_el1_oracle"), feature = "eret_zero_pc_oracle"))]
            let leg = "Z";

            frame.elr = injected;
            EL1_INJECTED_PC.store(injected, Ordering::Release);
            EL1_INJECTIONS.fetch_add(1, Ordering::AcqRel);

            use crate::arch_impl::aarch64::context_switch::{
                raw_uart_dec, raw_uart_hex, raw_uart_str,
            };
            raw_uart_str("[RESUME_PC_EL1_ORACLE:aarch64:leg=");
            raw_uart_str(leg);
            raw_uart_str(":FIRED:tid=");
            raw_uart_dec(victim_tid);
            raw_uart_str(":cpu=");
            raw_uart_dec(cpu_id as u64);
            raw_uart_str(":injected_pc=");
            raw_uart_hex(injected);
            raw_uart_str("]\n");
        }
    }
}

#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    ),
    not(feature = "resume_pc_el0_frame_oracle")
))]
pub(crate) fn inject_el0_resume_pc_if_armed(thread: &mut Thread) -> Option<u64> {
    if EL0_LIVE.load(Ordering::Acquire) != 1 {
        return None;
    }
    if thread.privilege != super::thread::ThreadPrivilege::User || thread.owner_pid.is_none() {
        return None;
    }

    let thread_id = thread.id();
    let victim_tid = match EL0_VICTIM_TID.compare_exchange(
        0,
        thread_id,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => thread_id,
        Err(victim_tid) => victim_tid,
    };
    if thread_id != victim_tid {
        return None;
    }

    EL0_OPPORTUNITIES.fetch_add(1, Ordering::AcqRel);
    #[cfg(not(feature = "resume_pc_oracle_disarm"))]
    {
        if crate::arch_impl::aarch64::context_switch::RESUME_PC_REFUSED_SOURCES
            .load(Ordering::Acquire)
            & ((1 << 6) | (1 << 7))
            != 0
        {
            return None;
        }
        let Ok(injection_index) = EL0_INJECTIONS.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |injections| (injections < 16).then_some(injections + 1),
        ) else {
            return None;
        };

        #[cfg(feature = "resume_pc_el0_kernel_oracle")]
        let injected =
            core::ptr::addr_of!(crate::arch_impl::aarch64::context_switch::RESUME_PC_REFUSALS)
                as u64;
        #[cfg(all(
            not(feature = "resume_pc_el0_kernel_oracle"),
            feature = "resume_pc_el0_tid_oracle"
        ))]
        let injected = thread_id;
        #[cfg(feature = "resume_pc_el0_kernel_oracle")]
        let leg = "UK";
        #[cfg(all(
            not(feature = "resume_pc_el0_kernel_oracle"),
            feature = "resume_pc_el0_tid_oracle"
        ))]
        let leg = "UT";

        let saved = thread.context.elr_el1;
        thread.context.elr_el1 = injected;
        EL0_INJECTED_PC.store(injected, Ordering::Release);

        if injection_index == 0 {
            let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as u64;
            use crate::arch_impl::aarch64::context_switch::{
                raw_uart_dec, raw_uart_hex, raw_uart_str,
            };
            raw_uart_str("[RESUME_PC_EL0_ORACLE:aarch64:leg=");
            raw_uart_str(leg);
            raw_uart_str(":FIRED:tid=");
            raw_uart_dec(thread_id);
            raw_uart_str(":cpu=");
            raw_uart_dec(cpu_id);
            raw_uart_str(":injected_pc=");
            raw_uart_hex(injected);
            raw_uart_str("]\n");
        }
        Some(saved)
    }
    #[cfg(feature = "resume_pc_oracle_disarm")]
    None
}

#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    ),
    not(feature = "resume_pc_el0_frame_oracle")
))]
pub(crate) fn restore_el0_resume_pc(thread: &mut Thread, saved: Option<u64>) {
    if let Some(saved) = saved {
        thread.context.elr_el1 = saved;
    }
}

#[cfg(all(
    target_arch = "aarch64",
    feature = "resume_pc_el0_frame_oracle",
    any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    )
))]
fn inject_el0_frame_resume_pc_if_armed(frame: &mut Aarch64ExceptionFrame) {
    if EL0_LIVE.load(Ordering::Acquire) != 1 {
        return;
    }
    if frame.spsr & 0xF != 0 {
        return;
    }
    if EL0_INJECTIONS.load(Ordering::Acquire) >= 16 {
        return;
    }

    EL0_OPPORTUNITIES.fetch_add(1, Ordering::AcqRel);
    #[cfg(not(feature = "resume_pc_oracle_disarm"))]
    {
        if crate::arch_impl::aarch64::context_switch::RESUME_PC_EL0_ASM_REFUSALS
            .load(Ordering::Acquire)
            != 0
        {
            return;
        }
        let Ok(injection_index) = EL0_INJECTIONS.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |injections| (injections < 16).then_some(injections + 1),
        ) else {
            return;
        };

        let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
        let thread_id =
            crate::arch_impl::aarch64::context_switch::last_dispatched_tid(cpu_id).unwrap_or(0);
        #[cfg(feature = "resume_pc_el0_kernel_oracle")]
        let injected =
            core::ptr::addr_of!(crate::arch_impl::aarch64::context_switch::RESUME_PC_REFUSALS)
                as u64;
        #[cfg(all(
            not(feature = "resume_pc_el0_kernel_oracle"),
            feature = "resume_pc_el0_tid_oracle"
        ))]
        let injected = thread_id;
        #[cfg(feature = "resume_pc_el0_kernel_oracle")]
        let leg = "FK";
        #[cfg(all(
            not(feature = "resume_pc_el0_kernel_oracle"),
            feature = "resume_pc_el0_tid_oracle"
        ))]
        let leg = "FT";

        frame.elr = injected;
        EL0_INJECTED_PC.store(injected, Ordering::Release);

        if injection_index == 0 {
            use crate::arch_impl::aarch64::context_switch::{
                raw_uart_dec, raw_uart_hex, raw_uart_str,
            };
            raw_uart_str("[RESUME_PC_EL0_ORACLE:aarch64:leg=");
            raw_uart_str(leg);
            raw_uart_str(":FIRED:tid=");
            raw_uart_dec(thread_id);
            raw_uart_str(":cpu=");
            raw_uart_dec(cpu_id as u64);
            raw_uart_str(":injected_pc=");
            raw_uart_hex(injected);
            raw_uart_str("]\n");
        }
    }
}

#[cfg(all(
    target_arch = "aarch64",
    any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle")
))]
fn resume_pc_victim() {
    loop {
        EL1_VICTIM_PROGRESS.fetch_add(1, Ordering::AcqRel);
        for _ in 0..200_000 {
            core::hint::spin_loop();
        }
        super::scheduler::schedule();
        super::strand_oracle::sleep_sample_period();
    }
}

#[cfg(all(
    target_arch = "aarch64",
    any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle")
))]
fn arm_el1_resume_pc_oracle() {
    while monotonic_now_ms() < 6_000 {
        super::strand_oracle::sleep_sample_period();
    }
    EL1_LIVE.store(1, Ordering::Release);
}

#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    )
))]
fn arm_el0_resume_pc_oracle() {
    while monotonic_now_ms() < 6_000 {
        super::strand_oracle::sleep_sample_period();
    }
    EL0_LIVE.store(1, Ordering::Release);
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

#[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
fn foreign_canary() {
    loop {
        FOREIGN_CANARY_PROGRESS.fetch_add(1, Ordering::AcqRel);
        super::scheduler::schedule();
        super::strand_oracle::sleep_sample_period();
    }
}

#[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
fn arm_foreign_record_oracle() {
    // Let the canary reach the ready queue and run at least once, so "still
    // alive at report time" is a statement about a scheduled thread.
    while FOREIGN_CANARY_PROGRESS.load(Ordering::Acquire) < 2 {
        super::strand_oracle::sleep_sample_period();
    }
    FOREIGN_ARM_RAN.store(1, Ordering::Release);

    let canary_tid = FOREIGN_CANARY_TID.load(Ordering::Acquire);
    if canary_tid == 0 {
        return;
    }
    // An offline slot: MAX_CPUS per-CPU records exist, but the gate boots with
    // fewer CPUs online. Planting there is foreign by construction, can never
    // be the draining CPU, and cannot perturb a running CPU's own record or
    // OWNER-TID canary.
    let record_cpu = crate::arch_impl::aarch64::constants::MAX_CPUS - 1;
    if crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize == record_cpu {
        return;
    }
    #[cfg(feature = "resume_pc_oracle_disarm")]
    {
        FOREIGN_RECORD_CPU.store(record_cpu as u64, Ordering::Release);
        return;
    }
    #[cfg(not(feature = "resume_pc_oracle_disarm"))]
    {
        // The record's CPU must name the canary, exactly as a real refusal on
        // that CPU would: that is the identity the old all-CPU drain read
        // across the race, and the thread this leg proves survives.
        crate::arch_impl::aarch64::context_switch::stamp_last_dispatched_tid_for_test(
            record_cpu, canary_tid,
        );
        const PLANTED_PC: u64 = 0x0000_0000_0200_0000;
        let planted = crate::per_cpu_aarch64::plant_synthetic_eret_guard_record(
            record_cpu,
            PLANTED_PC,
            0,
            crate::arch_impl::aarch64::context_switch::RESUME_PC_SOURCE_RET_DISPATCH,
        );
        FOREIGN_RECORD_CPU.store(record_cpu as u64, Ordering::Release);
        if planted {
            FOREIGN_PLANTED.store(1, Ordering::Release);
            crate::serial_println!(
                "[RESUME_PC_FOREIGN_ORACLE:aarch64:leg=G:PLANTED:record_cpu={}:canary_tid={}]",
                record_cpu,
                canary_tid,
            );
        }
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
        feature = "strand_inject_live_outgoing",
        feature = "resume_pc_el1_oracle",
        feature = "eret_zero_pc_oracle",
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle",
        feature = "resume_pc_foreign_oracle"
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
        feature = "strand_inject_live_outgoing",
        feature = "resume_pc_el1_oracle",
        feature = "eret_zero_pc_oracle",
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle",
        feature = "resume_pc_foreign_oracle"
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
    #[cfg(all(
        target_arch = "aarch64",
        feature = "ret_floor_oracle",
        not(feature = "resume_pc_oracle_disarm")
    ))]
    if FLOOR_FIRED.load(Ordering::Acquire) == 0 {
        return false;
    }
    #[cfg(all(
        target_arch = "aarch64",
        feature = "ret_floor_oracle",
        feature = "resume_pc_oracle_disarm"
    ))]
    if FLOOR_OPPORTUNITIES.load(Ordering::Acquire) == 0 {
        return false;
    }
    #[cfg(feature = "strand_inject_live_outgoing")]
    if LIVE_FIRED.load(Ordering::Acquire) == 0 {
        return false;
    }
    #[cfg(any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle"))]
    if EL1_OPPORTUNITIES.load(Ordering::Acquire) == 0 {
        return false;
    }
    #[cfg(any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    ))]
    if EL0_OPPORTUNITIES.load(Ordering::Acquire) == 0 {
        return false;
    }
    #[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
    if FOREIGN_ARM_RAN.load(Ordering::Acquire) == 0 {
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
    let opportunities = FLOOR_OPPORTUNITIES.load(Ordering::Acquire);
    let victim_tid = FLOOR_VICTIM_TID.load(Ordering::Acquire);
    let injected_pc = FLOOR_INJECTED_PC.load(Ordering::Acquire);
    let (refused, refused_tid, _refused_pc, refused_sources, _el0_asm_refused) =
        crate::arch_impl::aarch64::context_switch::resume_pc_refusal_snapshot();
    let ret_dispatch_arm = (refused_sources
        >> crate::arch_impl::aarch64::context_switch::RESUME_PC_SOURCE_RET_DISPATCH)
        & 1;
    let custody_checks =
        crate::arch_impl::aarch64::context_switch::RESUME_PC_CUSTODY_CHECKS.load(Ordering::Acquire);
    let custody_blind =
        crate::arch_impl::aarch64::context_switch::RESUME_PC_CUSTODY_BLIND.load(Ordering::Acquire);
    let current_dangling = crate::arch_impl::aarch64::context_switch::RESUME_PC_CURRENT_DANGLING
        .load(Ordering::Acquire);
    let current_repointed = crate::arch_impl::aarch64::context_switch::RESUME_PC_CURRENT_REPOINTED
        .load(Ordering::Acquire);
    let fatal = if crate::arch_impl::aarch64::exception::any_fatal_postmortem_captured() {
        1
    } else {
        0
    };
    #[cfg(not(feature = "resume_pc_oracle_disarm"))]
    // The refused ret-dispatch had already published its victim as this CPU's
    // current thread. `current_dangling >= 1` is the anti-vacuity half: the
    // hazard has to actually occur in this leg, or the repoint proves nothing.
    // `current_repointed == current_dangling` is the fix: every occurrence was
    // repointed at idle before the thread was marked Terminated, so no reclaim
    // can free a Box this CPU still publishes as current.
    let passed = armed == 1
        && fired == 1
        && ret_dispatch_arm == 1
        && refused_tid == victim_tid
        && custody_checks >= 1
        && custody_blind == 0
        && current_dangling >= 1
        && current_repointed == current_dangling
        && fatal == 0;
    #[cfg(feature = "resume_pc_oracle_disarm")]
    let passed = armed == 1
        && opportunities >= 1
        && fired == 0
        && refused == 0
        && refused_tid == 0
        && custody_checks == 0
        && custody_blind == 0
        && current_dangling == 0
        && current_repointed == 0
        && fatal == 0;
    crate::serial_println!(
        "[RET_FLOOR_ORACLE:aarch64:leg=F:armed={}:fired={}:opportunities={}:victim_tid={}:injected_pc=0x{:x}:refused={}:refused_tid={}:refused_sources=0x{:x}:ret_dispatch_arm={}:custody_checks={}:custody_blind={}:current_dangling={}:current_repointed={}:fatal={}:{}]",
        armed,
        fired,
        opportunities,
        victim_tid,
        injected_pc,
        refused,
        refused_tid,
        refused_sources,
        ret_dispatch_arm,
        custody_checks,
        custody_blind,
        current_dangling,
        current_repointed,
        fatal,
        if passed { "PASS" } else { "FAIL" },
    );
}

#[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
fn report_foreign_record() {
    let armed = FOREIGN_ARMED.load(Ordering::Acquire);
    let planted = FOREIGN_PLANTED.load(Ordering::Acquire);
    let record_cpu = FOREIGN_RECORD_CPU.load(Ordering::Acquire);
    let canary_tid = FOREIGN_CANARY_TID.load(Ordering::Acquire);
    let foreign_reports =
        crate::arch_impl::aarch64::context_switch::RESUME_PC_FOREIGN_REPORTS.load(Ordering::Acquire);
    let still_published = u64::from(crate::per_cpu_aarch64::eret_guard_record_is_published(
        record_cpu as usize,
    ));
    let (canary_present, canary_terminated) = super::scheduler::with_scheduler(|sched| {
        match sched.get_thread(canary_tid) {
            Some(thread) => (
                1u64,
                u64::from(thread.state == crate::task::thread::ThreadState::Terminated),
            ),
            None => (0u64, 0u64),
        }
    })
    .unwrap_or((0, 0));
    let canary_progress = FOREIGN_CANARY_PROGRESS.load(Ordering::Acquire);
    let fatal = if crate::arch_impl::aarch64::exception::any_fatal_postmortem_captured() {
        1
    } else {
        0
    };
    #[cfg(not(feature = "resume_pc_oracle_disarm"))]
    let passed = armed == 1
        && planted == 1
        && foreign_reports >= 1
        && still_published == 1
        && canary_present == 1
        && canary_terminated == 0
        && fatal == 0;
    #[cfg(feature = "resume_pc_oracle_disarm")]
    let passed = armed == 1
        && FOREIGN_ARM_RAN.load(Ordering::Acquire) == 1
        && planted == 0
        && foreign_reports == 0
        && still_published == 0
        && canary_present == 1
        && canary_terminated == 0
        && fatal == 0;
    crate::serial_println!(
        "[RESUME_PC_FOREIGN_ORACLE:aarch64:leg=G:armed={}:planted={}:record_cpu={}:canary_tid={}:canary_progress={}:foreign_reports={}:record_still_published={}:canary_present={}:canary_terminated={}:fatal={}:{}]",
        armed,
        planted,
        record_cpu,
        canary_tid,
        canary_progress,
        foreign_reports,
        still_published,
        canary_present,
        canary_terminated,
        fatal,
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
    any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle")
))]
fn report_el1_resume_pc() {
    #[cfg(feature = "resume_pc_el1_oracle")]
    let leg = "P";
    #[cfg(all(not(feature = "resume_pc_el1_oracle"), feature = "eret_zero_pc_oracle"))]
    let leg = "Z";

    let armed = EL1_ARMED.load(Ordering::Acquire);
    let live = EL1_LIVE.load(Ordering::Acquire);
    let opportunities = EL1_OPPORTUNITIES.load(Ordering::Acquire);
    let injected = EL1_INJECTIONS.load(Ordering::Acquire);
    let injected_pc = EL1_INJECTED_PC.load(Ordering::Acquire);
    let victim_tid = EL1_VICTIM_TID.load(Ordering::Acquire);
    let (refused, _refused_tid, refused_pc, refused_sources, _el0_asm_refused) =
        crate::arch_impl::aarch64::context_switch::resume_pc_refusal_snapshot();
    // Review note N8: `injected` counts injections, `refused` counts DRAINED
    // records, and the per-CPU record slot holds one entry — two refusals
    // between drains coalesce. `guard_events` is the per-arm execution count,
    // which does not coalesce, so the arithmetic is readable from the verdict.
    let guard_events = crate::per_cpu_aarch64::eret_guard_events_total();
    let el0_faults =
        crate::arch_impl::aarch64::exception::EL0_INSTRUCTION_FAULTS.load(Ordering::Acquire);
    let fatal = u64::from(crate::arch_impl::aarch64::exception::any_fatal_postmortem_captured());
    #[cfg(not(feature = "resume_pc_oracle_disarm"))]
    let passed = armed == 1
        && opportunities >= 1
        && injected >= 1
        && (refused_sources & 0b1_1110) != 0
        && guard_events >= refused
        && fatal == 0;
    #[cfg(feature = "resume_pc_oracle_disarm")]
    let passed = armed == 1
        && opportunities >= 1
        && injected == 0
        && refused == 0
        && guard_events >= refused
        && fatal == 0;

    crate::serial_println!(
        "[RESUME_PC_EL1_ORACLE:aarch64:leg={}:armed={}:live={}:opportunities={}:injected={}:injected_pc=0x{:x}:victim_tid={}:refused={}:guard_events={}:refused_sources=0x{:x}:refused_pc=0x{:x}:el0_faults={}:fatal={}:{}]",
        leg,
        armed,
        live,
        opportunities,
        injected,
        injected_pc,
        victim_tid,
        refused,
        guard_events,
        refused_sources,
        refused_pc,
        el0_faults,
        fatal,
        if passed { "PASS" } else { "FAIL" },
    );
}

#[cfg(all(
    target_arch = "aarch64",
    any(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle"
    )
))]
fn report_el0_resume_pc() {
    #[cfg(all(
        feature = "resume_pc_el0_kernel_oracle",
        not(feature = "resume_pc_el0_frame_oracle")
    ))]
    let leg = "UK";
    #[cfg(all(
        not(feature = "resume_pc_el0_kernel_oracle"),
        feature = "resume_pc_el0_tid_oracle",
        not(feature = "resume_pc_el0_frame_oracle")
    ))]
    let leg = "UT";
    #[cfg(all(
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_frame_oracle"
    ))]
    let leg = "FK";
    #[cfg(all(
        not(feature = "resume_pc_el0_kernel_oracle"),
        feature = "resume_pc_el0_tid_oracle",
        feature = "resume_pc_el0_frame_oracle"
    ))]
    let leg = "FT";

    let armed = EL0_ARMED.load(Ordering::Acquire);
    let live = EL0_LIVE.load(Ordering::Acquire);
    let opportunities = EL0_OPPORTUNITIES.load(Ordering::Acquire);
    let injected = EL0_INJECTIONS.load(Ordering::Acquire);
    let injected_pc = EL0_INJECTED_PC.load(Ordering::Acquire);
    let victim_tid = EL0_VICTIM_TID.load(Ordering::Acquire);
    let (refused, _refused_tid, refused_pc, refused_sources, el0_asm_refused) =
        crate::arch_impl::aarch64::context_switch::resume_pc_refusal_snapshot();
    let el0_faults =
        crate::arch_impl::aarch64::exception::EL0_INSTRUCTION_FAULTS.load(Ordering::Acquire);
    let fatal = u64::from(crate::arch_impl::aarch64::exception::any_fatal_postmortem_captured());
    #[cfg(all(
        not(feature = "resume_pc_oracle_disarm"),
        not(feature = "resume_pc_el0_frame_oracle")
    ))]
    let passed = armed == 1
        && injected >= 1
        && (refused_sources & ((1 << 6) | (1 << 7))) != 0
        && el0_faults == 0
        && fatal == 0;
    #[cfg(all(
        not(feature = "resume_pc_oracle_disarm"),
        feature = "resume_pc_el0_frame_oracle"
    ))]
    let passed = armed == 1
        && injected >= 1
        && el0_asm_refused >= 1
        && el0_faults == 0
        && fatal == 0;
    #[cfg(feature = "resume_pc_oracle_disarm")]
    let passed = armed == 1
        && opportunities >= 1
        && injected == 0
        && refused == 0
        && el0_faults == 0
        && fatal == 0;

    crate::serial_println!(
        "[RESUME_PC_EL0_ORACLE:aarch64:leg={}:armed={}:live={}:opportunities={}:injected={}:injected_pc=0x{:x}:victim_tid={}:refused={}:refused_sources=0x{:x}:refused_pc=0x{:x}:el0_asm_refused={}:el0_faults={}:fatal={}:{}]",
        leg,
        armed,
        live,
        opportunities,
        injected,
        injected_pc,
        victim_tid,
        refused,
        refused_sources,
        refused_pc,
        el0_asm_refused,
        el0_faults,
        fatal,
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
        feature = "strand_inject_live_outgoing",
        feature = "resume_pc_el1_oracle",
        feature = "eret_zero_pc_oracle",
        feature = "resume_pc_el0_kernel_oracle",
        feature = "resume_pc_el0_tid_oracle",
        feature = "resume_pc_foreign_oracle"
    )
))]
fn retzero_oracle_thread() {
    #[cfg(feature = "strand_inject_live_outgoing")]
    const SETTLE_MS: u64 = 3_000;
    #[cfg(not(feature = "strand_inject_live_outgoing"))]
    const SETTLE_MS: u64 = 2_000;
    #[cfg(feature = "ret_zero_pc_oracle_exec")]
    const HARD_CAP_MS: u64 = 40_000;
    #[cfg(all(
        not(feature = "ret_zero_pc_oracle_exec"),
        any(
            feature = "resume_pc_el1_oracle",
            feature = "eret_zero_pc_oracle",
            feature = "resume_pc_el0_kernel_oracle",
            feature = "resume_pc_el0_tid_oracle",
            feature = "resume_pc_foreign_oracle"
        )
    ))]
    const HARD_CAP_MS: u64 = 30_000;
    #[cfg(all(
        not(feature = "ret_zero_pc_oracle_exec"),
        not(any(
            feature = "resume_pc_el1_oracle",
            feature = "eret_zero_pc_oracle",
            feature = "resume_pc_el0_kernel_oracle",
            feature = "resume_pc_el0_tid_oracle",
            feature = "resume_pc_foreign_oracle"
        ))
    ))]
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
            #[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
            report_foreign_record();
            #[cfg(feature = "strand_inject_live_outgoing")]
            report_live_outgoing();
            #[cfg(any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle"))]
            report_el1_resume_pc();
            #[cfg(any(
                feature = "resume_pc_el0_kernel_oracle",
                feature = "resume_pc_el0_tid_oracle"
            ))]
            report_el0_resume_pc();
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
            feature = "strand_inject_live_outgoing",
            feature = "resume_pc_el1_oracle",
            feature = "eret_zero_pc_oracle",
            feature = "resume_pc_el0_kernel_oracle",
            feature = "resume_pc_el0_tid_oracle",
            feature = "resume_pc_foreign_oracle"
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
        #[cfg(any(feature = "resume_pc_el1_oracle", feature = "eret_zero_pc_oracle"))]
        {
            if let Ok(victim) = super::kthread::kthread_run(resume_pc_victim, "resume-pc-victim") {
                EL1_VICTIM_TID.store(victim.tid(), Ordering::Release);
                EL1_ARMED.store(1, Ordering::Release);
            }
            let _ = super::kthread::kthread_run(arm_el1_resume_pc_oracle, "resume-pc-arm");
        }
        #[cfg(any(
            feature = "resume_pc_el0_kernel_oracle",
            feature = "resume_pc_el0_tid_oracle"
        ))]
        {
            EL0_ARMED.store(1, Ordering::Release);
            let _ = super::kthread::kthread_run(arm_el0_resume_pc_oracle, "resume-pc-el0-arm");
        }
        #[cfg(all(target_arch = "aarch64", feature = "resume_pc_foreign_oracle"))]
        {
            if let Ok(canary) = super::kthread::kthread_run(foreign_canary, "foreign-canary") {
                FOREIGN_CANARY_TID.store(canary.tid(), Ordering::Release);
                FOREIGN_ARMED.store(1, Ordering::Release);
                let _ = super::kthread::kthread_run(arm_foreign_record_oracle, "foreign-arm");
            }
        }
        let _ = super::kthread::kthread_run(retzero_oracle_thread, "retzero-oracle");
    }
}
