//! ARM64 context switching logic.
//!
//! This module handles context switching on ARM64 (AArch64) when returning from
//! exceptions or performing explicit thread switches. It integrates with the
//! scheduler to perform preemptive multitasking.
//!
//! ## Single Lock Hold Architecture
//!
//! The entire context switch operation (scheduling decision, context save,
//! cpu_state update, context restore) is performed under a SINGLE acquisition
//! of the SCHEDULER lock. This eliminates TOCTOU races that caused DATA_ABORT
//! and INSTRUCTION_ABORT crashes when 15-22 separate lock acquisitions created
//! windows for other CPUs to corrupt scheduler state.
//!
//! Key differences from x86_64:
//! - Uses TTBR0_EL1 instead of CR3 for user page tables
//! - Uses ERET instead of IRETQ for exception return
//! - Uses TPIDR_EL1 for per-CPU data (like GS segment on x86)
//! - Memory barriers (DSB, ISB) required after page table switches

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use super::exception_frame::Aarch64ExceptionFrame;
use super::percpu::Aarch64PerCpu;
use crate::arch_impl::traits::PerCpuOps;
use crate::task::scheduler::Scheduler;
use crate::task::thread::{CpuContext, Thread, ThreadPrivilege, ThreadState};
use crate::tracing::providers::sched::trace_ctx_switch;

const SPSR_MODE_MASK: u64 = 0xF;
const SPSR_EL1H: u64 = 0x5;
const SPSR_DAIF_IRQ_BIT: u64 = 1 << 7;

#[inline]
fn dispatch_spsr(spsr: u64) -> u64 {
    spsr & !SPSR_DAIF_IRQ_BIT
}

#[inline]
fn kernel_dispatch_spsr(spsr: u64) -> u64 {
    ((spsr & !SPSR_MODE_MASK) | SPSR_EL1H) & !SPSR_DAIF_IRQ_BIT
}

core::arch::global_asm!(
    r#"
.section .text
.global aarch64_inline_schedule_switch
.type aarch64_inline_schedule_switch, @function
aarch64_inline_schedule_switch:
    // aarch64_inline_schedule_switch(old_ctx, scheduler_stack_top, trampoline)
    //   x0 = *mut CpuContext for the outgoing thread
    //   x1 = per-CPU scheduler stack top
    //   x2 = trampoline function entry
    //
    // Save the callee-saved kernel context at the exact point of the call,
    // then move to a neutral per-CPU stack before entering Rust again.
    //
    // IMPORTANT: This helper never restores the incoming thread directly.
    // It pivots to the scheduler trampoline, which builds an exception frame
    // and re-enters the selected thread via aarch64_enter_exception_frame/ERET.
    stp x19, x20, [x0, #152]
    stp x21, x22, [x0, #168]
    stp x23, x24, [x0, #184]
    stp x25, x26, [x0, #200]
    stp x27, x28, [x0, #216]
    stp x29, x30, [x0, #232]
    mov x3, sp
    str x3, [x0, #248]

    mov sp, x1
    br x2

// Linux-style ret-based kernel thread dispatch.
// Restores callee-saved registers + SP from a CpuContext, then ret to x30.
// This avoids ERET entirely, so no SPSR/DAIF state is restored from the
// thread's saved context. The caller controls DAIF independently.
//
// ret-based kernel thread dispatch. Avoids ERET entirely — no SPSR/DAIF
// state is restored from the thread. Prevents CPU IRQ death caused by
// ERET restoring PSTATE.I from a thread interrupted inside without_interrupts.
// Used for ALL kernel thread dispatches from schedule_from_kernel().
.global aarch64_ret_to_kernel_context
.type aarch64_ret_to_kernel_context, @function
aarch64_ret_to_kernel_context:
    // aarch64_ret_to_kernel_context(ctx: *const CpuContext, resume_pc: u64) -> !
    //   x0 = *const CpuContext to restore callee-saved regs + SP from
    //   x1 = resume PC (elr_el1 for exception-saved, x30 for inline-saved)
    ldp x19, x20, [x0, #152]
    ldp x21, x22, [x0, #168]
    ldp x23, x24, [x0, #184]
    ldp x25, x26, [x0, #200]
    ldp x27, x28, [x0, #216]
    ldp x29, x30, [x0, #232]
    ldr x2, [x0, #248]
    mov sp, x2
    // Enable IRQs before branching (matches Linux finish_task_switch).
    // daifclr unmasks IRQs, ISB is a context synchronization event that
    // ensures pending interrupts are recognized. On Parallels' GICv3
    // emulation, ISB alone may not be sufficient — WFI guarantees the
    // hypervisor checks for pending virtual interrupts.
    msr daifclr, #3
    isb
    // If a pending IRQ exists, it will fire here (between ISB and br).
    // If not, we branch directly to the resume PC.
    br x1

.global aarch64_enter_exception_frame
.type aarch64_enter_exception_frame, @function
aarch64_enter_exception_frame:
    // aarch64_enter_exception_frame(frame) -> !
    //   x0 = *const Aarch64ExceptionFrame
    //
    // Reuse the same restore/ERET rules as the IRQ return path by treating
    // the prepared frame as if it had been produced by an exception entry.
    mov sp, x0

    ldr x1, [sp, #248]
    cmp x1, #0x1000
    b.hs 1f
    adrp x1, idle_loop_arm64
    add x1, x1, :lo12:idle_loop_arm64
    str x1, [sp, #248]
    mov x2, #0x5
    str x2, [sp, #256]
1:
    msr elr_el1, x1
    ldr x1, [sp, #256]
    // Never propagate a stale saved DAIF.I bit through ERET.
    bic x1, x1, #0x80
    msr spsr_el1, x1

    ldp x0, x1, [sp, #0]
    ldp x2, x3, [sp, #16]
    ldp x4, x5, [sp, #32]
    ldp x6, x7, [sp, #48]
    ldp x8, x9, [sp, #64]
    ldp x10, x11, [sp, #80]
    ldp x12, x13, [sp, #96]
    ldp x14, x15, [sp, #112]
    ldr x17, [sp, #136]
    ldp x18, x19, [sp, #144]
    ldp x20, x21, [sp, #160]
    ldp x22, x23, [sp, #176]
    ldp x24, x25, [sp, #192]
    ldp x26, x27, [sp, #208]
    ldp x28, x29, [sp, #224]
    ldr x30, [sp, #240]

    mrs x16, tpidr_el1
    ldr x17, [sp, #128]
    str x17, [x16, #96]
    ldr x17, [sp, #136]

    mrs x16, spsr_el1
    and x16, x16, #0xF
    cbnz x16, 2f
    mrs x16, tpidr_el1
    ldr x16, [x16, #16]
    b 3f
2:
    mrs x16, tpidr_el1
    ldr x16, [x16, #40]
3:
    mov sp, x16

    mrs x16, tpidr_el1
    ldr x16, [x16, #96]
    eret
"#
);

extern "C" {
    fn aarch64_inline_schedule_switch(
        old_context: *mut CpuContext,
        scheduler_stack_top: u64,
        trampoline: extern "C" fn() -> !,
    );

    fn aarch64_enter_exception_frame(frame: *const Aarch64ExceptionFrame) -> !;
    fn aarch64_ret_to_kernel_context(ctx: *const CpuContext, resume_pc: u64) -> !;
}

const _: () = assert!(core::mem::offset_of!(CpuContext, x19) == 152);
const _: () = assert!(core::mem::offset_of!(CpuContext, x30) == 240);
const _: () = assert!(core::mem::offset_of!(CpuContext, sp) == 248);
const _: () = assert!(core::mem::offset_of!(Aarch64ExceptionFrame, x16) == 128);
const _: () = assert!(core::mem::offset_of!(Aarch64ExceptionFrame, x30) == 240);
const _: () = assert!(core::mem::offset_of!(Aarch64ExceptionFrame, elr) == 248);
const _: () = assert!(core::mem::offset_of!(Aarch64ExceptionFrame, spsr) == 256);

/// Diagnostic counter: number of times a thread dispatch hit ProcessGone
/// (TTBR0 lookup couldn't find the thread's process).
pub static TTBR_PROCESS_GONE_COUNT: AtomicU64 = AtomicU64::new(0);

/// Diagnostic counter: number of times a thread dispatch hit PmLockBusy
/// (PROCESS_MANAGER lock was contended during TTBR0 lookup).
pub static TTBR_PM_LOCK_BUSY_COUNT: AtomicU64 = AtomicU64::new(0);

/// Per-CPU deferred requeue storage.
///
/// CRITICAL SMP FIX: After a context switch, the old thread must NOT be
/// requeued to the ready queue until the current CPU is done reading from
/// the exception frame on the old thread's kernel stack. If requeued
/// immediately, another CPU can dispatch the old thread, and when that
/// thread takes its next exception, the new exception frame OVERWRITES
/// the frame that the current CPU is still reading (same kernel stack
/// address), causing corrupted ELR/SPSR/registers and ERET to address 0x0.
///
/// Solution: store the thread ID to requeue in per-CPU data. Process it
/// at the START of the next check_need_resched_and_switch_arm64 call,
/// which runs on a different stack (the new thread's kernel stack or
/// the boot stack after ERET).
///
/// Value 0 = no pending requeue. Non-zero = thread ID to requeue.
static DEFERRED_REQUEUE: [AtomicU64; 8] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

struct InlineScheduleState {
    scheduler_ptr: AtomicUsize,
    old_thread_id: AtomicU64,
    new_thread_id: AtomicU64,
    should_requeue_old: AtomicBool,
}

static INLINE_SCHEDULE_STATE: [InlineScheduleState;
    crate::arch_impl::aarch64::constants::MAX_CPUS] = [
    InlineScheduleState {
        scheduler_ptr: AtomicUsize::new(0),
        old_thread_id: AtomicU64::new(0),
        new_thread_id: AtomicU64::new(0),
        should_requeue_old: AtomicBool::new(false),
    },
    InlineScheduleState {
        scheduler_ptr: AtomicUsize::new(0),
        old_thread_id: AtomicU64::new(0),
        new_thread_id: AtomicU64::new(0),
        should_requeue_old: AtomicBool::new(false),
    },
    InlineScheduleState {
        scheduler_ptr: AtomicUsize::new(0),
        old_thread_id: AtomicU64::new(0),
        new_thread_id: AtomicU64::new(0),
        should_requeue_old: AtomicBool::new(false),
    },
    InlineScheduleState {
        scheduler_ptr: AtomicUsize::new(0),
        old_thread_id: AtomicU64::new(0),
        new_thread_id: AtomicU64::new(0),
        should_requeue_old: AtomicBool::new(false),
    },
    InlineScheduleState {
        scheduler_ptr: AtomicUsize::new(0),
        old_thread_id: AtomicU64::new(0),
        new_thread_id: AtomicU64::new(0),
        should_requeue_old: AtomicBool::new(false),
    },
    InlineScheduleState {
        scheduler_ptr: AtomicUsize::new(0),
        old_thread_id: AtomicU64::new(0),
        new_thread_id: AtomicU64::new(0),
        should_requeue_old: AtomicBool::new(false),
    },
    InlineScheduleState {
        scheduler_ptr: AtomicUsize::new(0),
        old_thread_id: AtomicU64::new(0),
        new_thread_id: AtomicU64::new(0),
        should_requeue_old: AtomicBool::new(false),
    },
    InlineScheduleState {
        scheduler_ptr: AtomicUsize::new(0),
        old_thread_id: AtomicU64::new(0),
        new_thread_id: AtomicU64::new(0),
        should_requeue_old: AtomicBool::new(false),
    },
];

// =============================================================================
// Per-CPU dispatch trace ring buffer — diagnostic instrumentation
//
// Records the last DISPATCH_RING_SIZE dispatches per CPU. On crash, the
// exception handler calls dump_dispatch_trace() to show exactly what
// context was dispatched before the fault.
// =============================================================================

const DISPATCH_RING_SIZE: usize = 8;
const MAX_CPUS_TRACE: usize = 8;

/// One dispatch event — what was written to the exception frame.
#[repr(C)]
struct DispatchEntry {
    tid: u64,
    old_tid: u64,
    elr: u64,
    spsr: u64,
    x30: u64,
    sp: u64,
    path: u8, // K=kernel, U=userspace, I=idle, F=first_entry, B=BUG-terminated
    from_el0: u8,
}

/// Per-CPU ring buffer of dispatch events.
struct DispatchRing {
    entries: [DispatchEntry; DISPATCH_RING_SIZE],
    write_idx: usize,
    count: usize,
}

static mut DISPATCH_TRACE: [DispatchRing; MAX_CPUS_TRACE] = {
    const EMPTY_ENTRY: DispatchEntry = DispatchEntry {
        tid: 0,
        old_tid: 0,
        elr: 0,
        spsr: 0,
        x30: 0,
        sp: 0,
        path: 0,
        from_el0: 0,
    };
    const EMPTY_RING: DispatchRing = DispatchRing {
        entries: [EMPTY_ENTRY; DISPATCH_RING_SIZE],
        write_idx: 0,
        count: 0,
    };
    [EMPTY_RING; MAX_CPUS_TRACE]
};

/// Record a dispatch event. Called at the END of dispatch_thread_locked
/// after all frame writes are complete.
fn record_dispatch(
    cpu_id: usize,
    old_tid: u64,
    tid: u64,
    elr: u64,
    spsr: u64,
    x30: u64,
    sp: u64,
    path: u8,
    from_el0: bool,
) {
    if cpu_id >= MAX_CPUS_TRACE {
        return;
    }
    unsafe {
        let ring = &mut DISPATCH_TRACE[cpu_id];
        let idx = ring.write_idx;
        ring.entries[idx] = DispatchEntry {
            tid,
            old_tid,
            elr,
            spsr,
            x30,
            sp,
            path,
            from_el0: from_el0 as u8,
        };
        ring.write_idx = (idx + 1) % DISPATCH_RING_SIZE;
        if ring.count < DISPATCH_RING_SIZE {
            ring.count += 1;
        }
    }
}

/// Dump the dispatch trace for a specific CPU. Called from the crash handler.
pub fn dump_dispatch_trace(cpu_id: usize) {
    if cpu_id >= MAX_CPUS_TRACE {
        return;
    }
    unsafe {
        let ring = &DISPATCH_TRACE[cpu_id];
        let count = ring.count;
        if count == 0 {
            raw_uart_str("  (no dispatches recorded)\n");
            return;
        }
        // Print from oldest to newest
        let start = if count < DISPATCH_RING_SIZE {
            0
        } else {
            ring.write_idx
        };
        for i in 0..count {
            let idx = (start + i) % DISPATCH_RING_SIZE;
            let e = &ring.entries[idx];
            raw_uart_str("  [");
            raw_uart_dec(i as u64);
            raw_uart_str("] ");
            raw_uart_char(e.path);
            raw_uart_str(" old=");
            raw_uart_dec(e.old_tid);
            raw_uart_str("->tid=");
            raw_uart_dec(e.tid);
            raw_uart_str(" elr=");
            raw_uart_hex(e.elr);
            raw_uart_str(" spsr=");
            raw_uart_hex(e.spsr);
            raw_uart_str(" x30=");
            raw_uart_hex(e.x30);
            raw_uart_str(" sp=");
            raw_uart_hex(e.sp);
            if e.from_el0 != 0 {
                raw_uart_str(" EL0");
            }
            raw_uart_str("\n");
        }
    }
}

/// Raw serial debug output - single character, no locks, no allocations.
/// Use this for debugging context switch paths where any allocation/locking
/// could perturb timing or cause deadlocks.
#[inline(always)]
#[allow(dead_code)]
pub fn raw_uart_char(c: u8) {
    let addr = crate::platform_config::uart_virt() as *mut u8;
    unsafe {
        core::ptr::write_volatile(addr, c);
    }
}

/// Raw UART string output - no locks, no allocations.
#[inline(always)]
#[allow(dead_code)]
pub fn raw_uart_str(s: &str) {
    for byte in s.bytes() {
        raw_uart_char(byte);
    }
}

/// Raw UART hex output for a u64 value - no locks, no allocations.
#[inline(always)]
#[allow(dead_code)]
pub fn raw_uart_hex(val: u64) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    raw_uart_str("0x");
    // Skip leading zeros for readability but always print at least one digit
    let mut started = false;
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xF) as usize;
        if nibble != 0 || started || i == 0 {
            raw_uart_char(HEX[nibble]);
            started = true;
        }
    }
}

/// Raw UART decimal output for a u64 value - no locks, no allocations.
#[inline(always)]
#[allow(dead_code)]
pub fn raw_uart_dec(val: u64) {
    if val == 0 {
        raw_uart_char(b'0');
        return;
    }
    let mut buf = [0u8; 20]; // max u64 is 20 digits
    let mut pos = 0;
    let mut v = val;
    while v > 0 {
        buf[pos] = b'0' + (v % 10) as u8;
        v /= 10;
        pos += 1;
    }
    for i in (0..pos).rev() {
        raw_uart_char(buf[i]);
    }
}

/// Ensure user_rsp_scratch is set to kernel_stack_top when returning to EL0.
///
/// The boot.S ERET path sets SP from user_rsp_scratch before ERET. After ERET
/// to EL0, SP_EL1 retains this value. When the next exception fires from EL0,
/// hardware pushes the exception frame at SP_EL1. If user_rsp_scratch is wrong
/// (e.g., stale boot stack), frames get pushed on the wrong stack.
#[inline(always)]
fn ensure_user_rsp_scratch_for_el0() {
    let kst = Aarch64PerCpu::kernel_stack_top();
    if kst != 0 {
        unsafe {
            Aarch64PerCpu::set_user_rsp_scratch(kst);
        }
    }
}

#[inline(always)]
fn read_daif() -> u64 {
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
    }
    daif
}

#[inline(always)]
fn read_sp_el0() -> u64 {
    let sp_el0: u64;
    unsafe {
        core::arch::asm!("mrs {}, sp_el0", out(reg) sp_el0, options(nomem, nostack));
    }
    sp_el0
}

#[inline(always)]
fn read_tpidr_el0() -> u64 {
    let tpidr: u64;
    unsafe {
        core::arch::asm!("mrs {}, tpidr_el0", out(reg) tpidr, options(nomem, nostack));
    }
    tpidr
}

#[inline(always)]
fn scheduler_stack_top(cpu_id: usize) -> u64 {
    const STACK_SIZE: u64 = 0x20_0000;
    super::constants::percpu_stack_region_base() + ((cpu_id as u64) + 1) * STACK_SIZE
}

// =============================================================================
// Inline context save/restore helpers
//
// These functions take &mut Thread directly and perform register copies without
// any lock acquisition. They are called from within the single scheduler lock
// hold in check_need_resched_and_switch_arm64.
// =============================================================================

/// Save userspace context — called inside scheduler lock hold.
fn save_userspace_context_inline(thread: &mut Thread, frame: &Aarch64ExceptionFrame) {
    // Save ALL general-purpose registers from exception frame.
    // CRITICAL: When a userspace thread is context-switched (e.g., for blocking I/O
    // or preemption), its caller-saved registers (x1-x18) may contain important
    // values that must be preserved for correct execution when resumed.
    thread.context.x0 = frame.x0;
    thread.context.x1 = frame.x1;
    thread.context.x2 = frame.x2;
    thread.context.x3 = frame.x3;
    thread.context.x4 = frame.x4;
    thread.context.x5 = frame.x5;
    thread.context.x6 = frame.x6;
    thread.context.x7 = frame.x7;
    thread.context.x8 = frame.x8;
    thread.context.x9 = frame.x9;
    thread.context.x10 = frame.x10;
    thread.context.x11 = frame.x11;
    thread.context.x12 = frame.x12;
    thread.context.x13 = frame.x13;
    thread.context.x14 = frame.x14;
    thread.context.x15 = frame.x15;
    thread.context.x16 = frame.x16;
    thread.context.x17 = frame.x17;
    thread.context.x18 = frame.x18;
    thread.context.x19 = frame.x19;
    thread.context.x20 = frame.x20;
    thread.context.x21 = frame.x21;
    thread.context.x22 = frame.x22;
    thread.context.x23 = frame.x23;
    thread.context.x24 = frame.x24;
    thread.context.x25 = frame.x25;
    thread.context.x26 = frame.x26;
    thread.context.x27 = frame.x27;
    thread.context.x28 = frame.x28;
    thread.context.x29 = frame.x29;
    thread.context.x30 = frame.x30;

    // Save program counter and status
    thread.context.elr_el1 = frame.elr;
    thread.context.spsr_el1 = frame.spsr;

    // Clear inline-schedule flag (saved by exception path, needs ERET dispatch)
    thread.saved_by_inline_schedule = false;

    // Read and save SP_EL0 (user stack pointer)
    let sp_el0: u64;
    unsafe {
        core::arch::asm!("mrs {}, sp_el0", out(reg) sp_el0, options(nomem, nostack));
    }
    thread.context.sp_el0 = sp_el0;

    // Save TPIDR_EL0 (user TLS pointer) - critical for musl/libc TLS correctness
    let tpidr: u64;
    unsafe {
        core::arch::asm!("mrs {}, tpidr_el0", out(reg) tpidr, options(nomem, nostack));
    }
    thread.context.tpidr_el0 = tpidr;

    // CRITICAL: Save kernel stack pointer for blocked-in-syscall restoration.
    thread.context.sp = frame as *const _ as u64 + 272;
}

/// Save kernel context — called inside scheduler lock hold.
fn save_kernel_context_inline(thread: &mut Thread, frame: &Aarch64ExceptionFrame) {
    // Save ALL general-purpose registers, not just callee-saved.
    // This is critical for context switching: when a thread is preempted in the
    // middle of a loop (like kthread_join's WFI loop), its caller-saved registers
    // (x0-x18) contain important values (loop variables, pointers, etc.).
    thread.context.x0 = frame.x0;
    thread.context.x1 = frame.x1;
    thread.context.x2 = frame.x2;
    thread.context.x3 = frame.x3;
    thread.context.x4 = frame.x4;
    thread.context.x5 = frame.x5;
    thread.context.x6 = frame.x6;
    thread.context.x7 = frame.x7;
    thread.context.x8 = frame.x8;
    thread.context.x9 = frame.x9;
    thread.context.x10 = frame.x10;
    thread.context.x11 = frame.x11;
    thread.context.x12 = frame.x12;
    thread.context.x13 = frame.x13;
    thread.context.x14 = frame.x14;
    thread.context.x15 = frame.x15;
    thread.context.x16 = frame.x16;
    thread.context.x17 = frame.x17;
    thread.context.x18 = frame.x18;
    thread.context.x19 = frame.x19;
    thread.context.x20 = frame.x20;
    thread.context.x21 = frame.x21;
    thread.context.x22 = frame.x22;
    thread.context.x23 = frame.x23;
    thread.context.x24 = frame.x24;
    thread.context.x25 = frame.x25;
    thread.context.x26 = frame.x26;
    thread.context.x27 = frame.x27;
    thread.context.x28 = frame.x28;
    thread.context.x29 = frame.x29;
    thread.context.x30 = frame.x30;

    // Save program counter and processor state
    thread.context.elr_el1 = frame.elr;
    thread.context.spsr_el1 = kernel_dispatch_spsr(frame.spsr);

    // Clear inline-schedule flag: this thread was saved by the IRQ-return
    // exception path (full register set), so it must be re-dispatched via
    // ERET, not the ret-based path.
    thread.saved_by_inline_schedule = false;

    // Save the kernel stack pointer.
    // The exception frame is allocated on the stack, so the SP before the
    // exception was (frame_address + frame_size). The frame size is 272 bytes
    // (see boot.S irq_handler: sub sp, sp, #272).
    thread.context.sp = frame as *const _ as u64 + 272;

    // Also save SP_EL0 for userspace threads blocked in syscall.
    let sp_el0: u64;
    unsafe {
        core::arch::asm!("mrs {}, sp_el0", out(reg) sp_el0, options(nomem, nostack));
    }
    thread.context.sp_el0 = sp_el0;

    // Save TPIDR_EL0 (user TLS pointer) - critical for musl/libc TLS correctness
    let tpidr: u64;
    unsafe {
        core::arch::asm!("mrs {}, tpidr_el0", out(reg) tpidr, options(nomem, nostack));
    }
    thread.context.tpidr_el0 = tpidr;
}

/// Restore kernel thread context into frame — called inside scheduler lock hold.
///
/// Handles both first-run (has_started=false) and resume (has_started=true) cases.
/// Returns `true` if the context was valid and restored successfully.
/// Returns `false` if the context was corrupt — caller MUST redirect to idle
/// and update cpu_state to avoid saving garbage into this thread on next preemption.
fn restore_kernel_context_inline(
    thread: &mut Thread,
    frame: &mut Aarch64ExceptionFrame,
    thread_id: u64,
) -> bool {
    let has_started = thread.has_started;

    if !has_started {
        // First run - mark as started and set up entry point.
        // CRITICAL: Also initialize elr_el1 and spsr_el1 to safe values.
        thread.has_started = true;
        thread.context.elr_el1 = thread.context.x30; // Entry point
        thread.context.spsr_el1 = kernel_dispatch_spsr(thread.context.spsr_el1);
    }

    // Validate ELR before restoring any registers.
    // If the context is corrupt, return false immediately — the caller will
    // redirect to idle and update cpu_state so that the next preemption
    // doesn't save idle-loop registers into this thread's context.
    //
    // On QEMU, kernel code runs from HHDM (>= 0xFFFF_0000_0000_0000).
    // On Parallels, the UEFI loader jumps to kernel_main at a physical
    // address and the kernel runs identity-mapped, so function pointers
    // resolve to physical addresses in the RAM range (0x40080000+).
    const KERNEL_VIRT_BASE: u64 = 0xFFFF_0000_0000_0000;
    const KERNEL_PHYS_BASE: u64 = 0x4008_0000;
    const KERNEL_PHYS_LIMIT: u64 = 0xC000_0000;
    #[inline]
    fn is_kernel_addr(addr: u64) -> bool {
        addr >= KERNEL_VIRT_BASE || (addr >= KERNEL_PHYS_BASE && addr < KERNEL_PHYS_LIMIT)
    }
    let elr_valid = if !has_started {
        // First run: x30 must be a valid kernel address
        is_kernel_addr(thread.context.x30)
    } else {
        // Resume: elr_el1 must be in kernel space or zero (handled below)
        is_kernel_addr(thread.context.elr_el1) || thread.context.elr_el1 == 0
    };

    if !elr_valid {
        raw_uart_str("\n!!! BUG: invalid context for kernel dispatch tid=");
        raw_uart_dec(thread_id);
        raw_uart_str("\n  elr_el1=");
        raw_uart_hex(thread.context.elr_el1);
        raw_uart_str(" spsr_el1=");
        raw_uart_hex(thread.context.spsr_el1);
        raw_uart_str(" x30=");
        raw_uart_hex(thread.context.x30);
        raw_uart_str(" sp=");
        raw_uart_hex(thread.context.sp);
        raw_uart_str("\n  has_started=");
        raw_uart_char(if has_started { b'1' } else { b'0' });
        raw_uart_str(" priv=");
        raw_uart_char(match thread.privilege {
            ThreadPrivilege::Kernel => b'K',
            ThreadPrivilege::User => b'U',
        });
        raw_uart_str(" blocked_in_syscall=");
        raw_uart_char(if thread.blocked_in_syscall {
            b'1'
        } else {
            b'0'
        });
        raw_uart_str(" cpu=");
        raw_uart_dec(Aarch64PerCpu::cpu_id() as u64);
        raw_uart_str("\n");
        return false;
    }

    // Restore ALL general-purpose registers directly from thread.context.
    frame.x0 = thread.context.x0;
    frame.x1 = thread.context.x1;
    frame.x2 = thread.context.x2;
    frame.x3 = thread.context.x3;
    frame.x4 = thread.context.x4;
    frame.x5 = thread.context.x5;
    frame.x6 = thread.context.x6;
    frame.x7 = thread.context.x7;
    frame.x8 = thread.context.x8;
    frame.x9 = thread.context.x9;
    frame.x10 = thread.context.x10;
    frame.x11 = thread.context.x11;
    frame.x12 = thread.context.x12;
    frame.x13 = thread.context.x13;
    frame.x14 = thread.context.x14;
    frame.x15 = thread.context.x15;
    frame.x16 = thread.context.x16;
    frame.x17 = thread.context.x17;
    frame.x18 = thread.context.x18;
    frame.x19 = thread.context.x19;
    frame.x20 = thread.context.x20;
    frame.x21 = thread.context.x21;
    frame.x22 = thread.context.x22;
    frame.x23 = thread.context.x23;
    frame.x24 = thread.context.x24;
    frame.x25 = thread.context.x25;
    frame.x26 = thread.context.x26;
    frame.x27 = thread.context.x27;
    frame.x28 = thread.context.x28;
    frame.x29 = thread.context.x29;
    frame.x30 = thread.context.x30;

    // Set return address and SPSR
    if !has_started {
        frame.elr = thread.context.x30; // First run: jump to entry point
        frame.spsr = kernel_dispatch_spsr(thread.context.spsr_el1);
    } else if is_kernel_addr(thread.context.elr_el1) {
        // Resume: return to where we left off.
        // On QEMU, kernel addresses are >= KERNEL_VIRT_BASE (HHDM).
        // On Parallels, kernel runs identity-mapped at physical addresses
        // (KERNEL_PHYS_BASE..KERNEL_PHYS_LIMIT), so we must accept both.
        frame.elr = thread.context.elr_el1;
        thread.context.spsr_el1 = kernel_dispatch_spsr(thread.context.spsr_el1);
        frame.spsr = dispatch_spsr(thread.context.spsr_el1); // Restore saved processor state
    } else {
        // elr_el1 == 0 or not a valid kernel address — redirect to idle
        raw_uart_str("WARN: bad elr=");
        raw_uart_hex(thread.context.elr_el1);
        raw_uart_str(" for started kthread tid=");
        raw_uart_dec(thread_id);
        raw_uart_str(", redirecting to idle\n");
        return false;
    }

    // Store kernel SP for restoration after ERET
    unsafe {
        Aarch64PerCpu::set_user_rsp_scratch(thread.context.sp);
    }

    // CRITICAL: Restore SP_EL0 for userspace threads blocked in syscalls.
    if thread.context.sp_el0 != 0 {
        unsafe {
            core::arch::asm!(
                "msr sp_el0, {}",
                in(reg) thread.context.sp_el0,
                options(nomem, nostack)
            );
        }
    }

    // Restore TPIDR_EL0 (user TLS pointer) - critical for musl/libc TLS correctness
    unsafe {
        core::arch::asm!(
            "msr tpidr_el0, {}",
            in(reg) thread.context.tpidr_el0,
            options(nomem, nostack)
        );
    }

    // Memory barrier to ensure all writes are visible
    core::sync::atomic::fence(Ordering::SeqCst);
    true
}

/// Restore userspace context into frame — called inside scheduler lock hold.
fn restore_userspace_context_inline(
    thread: &mut Thread,
    frame: &mut Aarch64ExceptionFrame,
) {
    frame.x0 = thread.context.x0;
    frame.x1 = thread.context.x1;
    frame.x2 = thread.context.x2;
    frame.x3 = thread.context.x3;
    frame.x4 = thread.context.x4;
    frame.x5 = thread.context.x5;
    frame.x6 = thread.context.x6;
    frame.x7 = thread.context.x7;
    frame.x8 = thread.context.x8;
    frame.x9 = thread.context.x9;
    frame.x10 = thread.context.x10;
    frame.x11 = thread.context.x11;
    frame.x12 = thread.context.x12;
    frame.x13 = thread.context.x13;
    frame.x14 = thread.context.x14;
    frame.x15 = thread.context.x15;
    frame.x16 = thread.context.x16;
    frame.x17 = thread.context.x17;
    frame.x18 = thread.context.x18;
    frame.x19 = thread.context.x19;
    frame.x20 = thread.context.x20;
    frame.x21 = thread.context.x21;
    frame.x22 = thread.context.x22;
    frame.x23 = thread.context.x23;
    frame.x24 = thread.context.x24;
    frame.x25 = thread.context.x25;
    frame.x26 = thread.context.x26;
    frame.x27 = thread.context.x27;
    frame.x28 = thread.context.x28;
    frame.x29 = thread.context.x29;
    frame.x30 = thread.context.x30;

    // Restore program counter and status
    frame.elr = thread.context.elr_el1;
    frame.spsr = dispatch_spsr(thread.context.spsr_el1);

    // Restore SP_EL0 (user stack pointer)
    unsafe {
        core::arch::asm!(
            "msr sp_el0, {}",
            in(reg) thread.context.sp_el0,
            options(nomem, nostack)
        );
    }

    // Restore TPIDR_EL0 (user TLS pointer) - critical for musl/libc TLS correctness
    unsafe {
        core::arch::asm!(
            "msr tpidr_el0, {}",
            in(reg) thread.context.tpidr_el0,
            options(nomem, nostack)
        );
    }
}

/// Set up first userspace entry — called inside scheduler lock hold.
fn setup_first_entry_inline(thread: &mut Thread, frame: &mut Aarch64ExceptionFrame) {
    // Set return address to entry point
    frame.elr = thread.context.elr_el1;

    // SPSR for EL0t (userspace, interrupts enabled)
    frame.spsr = 0x0;

    // Set up user stack pointer
    unsafe {
        core::arch::asm!(
            "msr sp_el0, {}",
            in(reg) thread.context.sp_el0,
            options(nomem, nostack)
        );
    }

    // Clear all registers for security
    frame.x0 = 0;
    frame.x1 = 0;
    frame.x2 = 0;
    frame.x3 = 0;
    frame.x4 = 0;
    frame.x5 = 0;
    frame.x6 = 0;
    frame.x7 = 0;
    frame.x8 = 0;
    frame.x9 = 0;
    frame.x10 = 0;
    frame.x11 = 0;
    frame.x12 = 0;
    frame.x13 = 0;
    frame.x14 = 0;
    frame.x15 = 0;
    frame.x16 = 0;
    frame.x17 = 0;
    frame.x18 = 0;
    frame.x19 = 0;
    frame.x20 = 0;
    frame.x21 = 0;
    frame.x22 = 0;
    frame.x23 = 0;
    frame.x24 = 0;
    frame.x25 = 0;
    frame.x26 = 0;
    frame.x27 = 0;
    frame.x28 = 0;
    frame.x29 = 0;
    frame.x30 = 0;

    // Clear TPIDR_EL0 - musl will set it during __init_tls
    unsafe {
        core::arch::asm!("msr tpidr_el0, xzr", options(nomem, nostack));
    }
}

// =============================================================================
// Locked helper functions (called inside single scheduler lock hold)
// =============================================================================

/// Fix cpu_state mismatch — called inside scheduler lock hold.
///
/// If frame SPSR says EL0 but cpu_state says idle, we have a mismatch.
/// Fix cpu_state to reflect the actual running thread from the per-CPU pointer.
#[inline(always)]
fn fix_eret_cpu_state_locked(sched: &mut Scheduler, frame: &Aarch64ExceptionFrame) {
    let to_el0 = (frame.spsr & 0xF) == 0;
    if !to_el0 {
        return;
    }
    let cpu = Aarch64PerCpu::cpu_id() as usize;
    if let Some(tid) = sched.cpu_state[cpu].current_thread {
        if sched.is_idle_thread_inner(tid) {
            let real_thread_ptr = Aarch64PerCpu::current_thread_ptr();
            if !real_thread_ptr.is_null() {
                let real_thread = unsafe { &*(real_thread_ptr as *const Thread) };
                let real_tid = real_thread.id();
                if !sched.is_idle_thread_inner(real_tid) {
                    sched.commit_cpu_state_after_save(real_tid);
                }
            }
        }
    }
}

/// Set up exception frame to return to idle loop — called inside scheduler lock hold.
fn setup_idle_return_locked(
    sched: &mut Scheduler,
    frame: &mut Aarch64ExceptionFrame,
    cpu_id: usize,
) {
    // Set frame ELR and SPSR to safe values FIRST
    let idle_addr = idle_loop_arm64 as *const () as u64;
    frame.elr = idle_addr;
    frame.spsr = 0x5; // EL1h with interrupts enabled

    // Get idle thread's kernel stack
    let idle_id = sched.cpu_state[cpu_id].idle_thread;
    let idle_stack = sched
        .get_thread(idle_id)
        .and_then(|t| t.kernel_stack_top.map(|v| v.as_u64()))
        .unwrap_or_else(|| {
            let cpu_id64 = cpu_id as u64;
            super::constants::percpu_stack_region_base() + (cpu_id64 + 1) * 0x20_0000
        });

    // Clear all general purpose registers for clean state
    frame.x0 = 0;
    frame.x1 = 0;
    frame.x2 = 0;
    frame.x3 = 0;
    frame.x4 = 0;
    frame.x5 = 0;
    frame.x6 = 0;
    frame.x7 = 0;
    frame.x8 = 0;
    frame.x9 = 0;
    frame.x10 = 0;
    frame.x11 = 0;
    frame.x12 = 0;
    frame.x13 = 0;
    frame.x14 = 0;
    frame.x15 = 0;
    frame.x16 = 0;
    frame.x17 = 0;
    frame.x18 = 0;
    frame.x19 = 0;
    frame.x20 = 0;
    frame.x21 = 0;
    frame.x22 = 0;
    frame.x23 = 0;
    frame.x24 = 0;
    frame.x25 = 0;
    frame.x26 = 0;
    frame.x27 = 0;
    frame.x28 = 0;
    frame.x29 = 0;
    frame.x30 = 0;

    unsafe {
        Aarch64PerCpu::set_user_rsp_scratch(idle_stack);
        Aarch64PerCpu::set_kernel_stack_top(idle_stack);
        Aarch64PerCpu::set_current_thread_ptr(core::ptr::null_mut());
        Aarch64PerCpu::clear_preempt_active();
    }
}

/// Dispatch an idle thread — called inside scheduler lock hold.
fn dispatch_idle_locked(
    sched: &mut Scheduler,
    thread_id: u64,
    frame: &mut Aarch64ExceptionFrame,
    cpu_id: usize,
) {
    if cpu_id == 0 {
        // CPU 0 (boot thread): May be preempted while running meaningful kernel
        // code (e.g., kthread_join's polling loop during test execution). In that
        // case we need to restore the saved context so the boot thread resumes.
        let idle_loop_addr = idle_loop_arm64 as *const () as u64;
        const KERNEL_VIRT_BASE: u64 = 0xFFFF_0000_0000_0000;
        // Also accept physical kernel addresses on Parallels
        const KERNEL_PHYS_BASE: u64 = 0x4008_0000;
        const KERNEL_PHYS_LIMIT: u64 = 0xC000_0000;
        let has_saved_context = sched
            .get_thread(thread_id)
            .map(|thread| {
                let elr = thread.context.elr_el1;
                let sp = thread.context.sp;
                let spsr = thread.context.spsr_el1;
                let elr_is_kernel =
                    elr >= KERNEL_VIRT_BASE || (elr >= KERNEL_PHYS_BASE && elr < KERNEL_PHYS_LIMIT);
                let sp_is_kernel =
                    sp >= KERNEL_VIRT_BASE || (sp >= KERNEL_PHYS_BASE && sp < KERNEL_PHYS_LIMIT);
                let near_idle = elr >= idle_loop_addr && elr < idle_loop_addr + 16;
                elr_is_kernel && !near_idle && sp_is_kernel && (spsr & 0xF) != 0
            })
            .unwrap_or(false);

        if has_saved_context {
            let ok = sched
                .get_thread_mut(thread_id)
                .map(|thread| restore_kernel_context_inline(thread, frame, thread_id))
                .unwrap_or(false);
            if !ok {
                setup_idle_return_locked(sched, frame, cpu_id);
            }
        } else {
            setup_idle_return_locked(sched, frame, cpu_id);
        }
    } else {
        // Secondary CPUs: always use clean idle return
        setup_idle_return_locked(sched, frame, cpu_id);
    }
}

/// Dispatch a non-idle thread — called inside scheduler lock hold.
///
/// Handles kernel threads, userspace threads (first entry and resume),
/// and threads blocked in syscalls. Also handles TTBR0 setup for
/// userspace threads and the fallback-to-idle path when PM lock is contended.
fn dispatch_thread_locked(
    sched: &mut Scheduler,
    thread_id: u64,
    frame: &mut Aarch64ExceptionFrame,
    cpu_id: usize,
) {
    // Read all dispatch properties in a SINGLE borrow of the thread.
    // This eliminates TOCTOU races from separate lock acquisitions.
    let thread_info = sched.get_thread_mut(thread_id).map(|thread| {
        let state = thread.state;
        let privilege = thread.privilege;
        let blocked_in_syscall = thread.blocked_in_syscall;
        let has_started = thread.has_started;
        let elr = thread.context.elr_el1;
        let kernel_stack_top = thread.kernel_stack_top;
        let thread_ptr = thread as *const _ as *mut u8;
        (
            state,
            privilege,
            blocked_in_syscall,
            has_started,
            elr,
            kernel_stack_top,
            thread_ptr,
        )
    });

    let (state, privilege, blocked_in_syscall, has_started, elr, kernel_stack_top, thread_ptr) =
        match thread_info {
            Some(info) => info,
            None => {
                setup_idle_return_locked(sched, frame, cpu_id);
                return;
            }
        };

    // DEFENSE: Verify thread is not terminated before dispatch.
    if state == ThreadState::Terminated {
        setup_idle_return_locked(sched, frame, cpu_id);
        return;
    }

    // Update per-CPU current thread pointer (register writes, no lock needed)
    unsafe {
        Aarch64PerCpu::set_current_thread_ptr(thread_ptr);
    }
    if let Some(kst) = kernel_stack_top {
        unsafe {
            Aarch64PerCpu::set_kernel_stack_top(kst.as_u64());
        }
    }

    let is_idle = sched.is_idle_thread_inner(thread_id);
    let is_kernel = privilege == ThreadPrivilege::Kernel;
    const KERNEL_VIRT_BASE: u64 = 0xFFFF_0000_0000_0000;
    let is_in_kernel_mode = elr >= KERNEL_VIRT_BASE;

    if is_idle {
        dispatch_idle_locked(sched, thread_id, frame, cpu_id);
    } else if is_kernel || blocked_in_syscall || is_in_kernel_mode {
        // Kernel threads, userspace threads blocked in syscall, and userspace
        // threads interrupted while in kernel mode all need kernel context
        // restoration (they're running in kernel mode with a kernel SP)

        // CRITICAL: For userspace threads in kernel mode, set up TTBR0 so
        // the correct process page table is active when the syscall completes.
        // Must succeed BEFORE restoring context — if TTBR0 setup fails,
        // redirect to idle (same pattern as regular userspace threads).
        if (blocked_in_syscall || is_in_kernel_mode) && !is_kernel {
            let ttbr_result = set_next_ttbr0_for_thread(thread_id);
            match ttbr_result {
                TtbrResult::Ok => {
                    switch_ttbr0_if_needed(thread_id);
                }
                TtbrResult::PmLockBusy => {
                    TTBR_PM_LOCK_BUSY_COUNT.fetch_add(1, Ordering::Relaxed);
                    // PM lock still held after retries — redirect to idle and requeue.
                    // CRITICAL: Update cpu_state BEFORE requeue_thread_after_save,
                    // because requeue checks cpu_state[].current_thread and silently
                    // drops threads that appear to be running on any CPU.
                    if let Some(thread) = sched.get_thread_mut(thread_id) {
                        thread.state = ThreadState::Ready;
                    }
                    setup_idle_return_locked(sched, frame, cpu_id);
                    let idle_id = sched.cpu_state[cpu_id].idle_thread;
                    sched.cpu_state[cpu_id].current_thread = Some(idle_id);
                    sched.requeue_thread_after_save(thread_id);
                    sched.set_need_resched_inner();
                    return;
                }
                TtbrResult::ProcessGone => {
                    TTBR_PROCESS_GONE_COUNT.fetch_add(1, Ordering::Relaxed);
                    raw_uart_str("\n[TTBR_GONE_K] tid=");
                    raw_uart_dec(thread_id);
                    raw_uart_str(" elr=");
                    raw_uart_hex(frame.elr);
                    raw_uart_str(" cpu=");
                    raw_uart_dec(cpu_id as u64);
                    raw_uart_str("\n");
                    // Process no longer exists — terminate orphaned thread.
                    if let Some(thread) = sched.get_thread_mut(thread_id) {
                        thread.state = ThreadState::Terminated;
                    }
                    setup_idle_return_locked(sched, frame, cpu_id);
                    let idle_id = sched.cpu_state[cpu_id].idle_thread;
                    sched.cpu_state[cpu_id].current_thread = Some(idle_id);
                    return;
                }
            }
        }

        let restore_ok = sched
            .get_thread_mut(thread_id)
            .map(|thread| restore_kernel_context_inline(thread, frame, thread_id))
            .unwrap_or(false);

        if !restore_ok {
            // Context was corrupt or thread not found. Terminate the thread
            // and redirect to idle. CRITICAL: We must update cpu_state here,
            // otherwise the next timer preemption will save idle-loop registers
            // into this thread's context slot, compounding the corruption.
            if let Some(thread) = sched.get_thread_mut(thread_id) {
                thread.state = ThreadState::Terminated;
            }
            setup_idle_return_locked(sched, frame, cpu_id);
            let idle_id = sched.cpu_state[cpu_id].idle_thread;
            sched.cpu_state[cpu_id].current_thread = Some(idle_id);
            return;
        }
    } else {
        // Userspace thread
        if !has_started {
            if let Some(thread) = sched.get_thread_mut(thread_id) {
                thread.has_started = true;
                setup_first_entry_inline(thread, frame);
            }
        } else {
            if let Some(thread) = sched.get_thread_mut(thread_id) {
                restore_userspace_context_inline(thread, frame);
            }

            // SAFETY GUARD: Check for corrupted ELR before committing to dispatch.
            if frame.elr < 0x1000 || (frame.spsr & 0xF) != 0 {
                raw_uart_str("\n[BUG] dispatch_thread: bad context tid=");
                raw_uart_dec(thread_id);
                raw_uart_str(" elr=");
                raw_uart_hex(frame.elr);
                raw_uart_str(" spsr=");
                raw_uart_hex(frame.spsr);
                raw_uart_str(" cpu=");
                raw_uart_dec(cpu_id as u64);
                raw_uart_str(", terminating thread\n");

                // Terminate the thread — a corrupt context (ELR=0 or garbage SPSR)
                // is unrecoverable. Previously this requeued the thread, which
                // caused an infinite BUG loop as every CPU that dispatched it
                // would hit the same corrupt context. Termination lets the parent
                // process (init) detect the exit and respawn if needed.
                if let Some(thread) = sched.get_thread_mut(thread_id) {
                    thread.state = ThreadState::Terminated;
                }
                setup_idle_return_locked(sched, frame, cpu_id);
                let idle_id = sched.cpu_state[cpu_id].idle_thread;
                sched.cpu_state[cpu_id].current_thread = Some(idle_id);
                return;
            }
        }

        // Set TTBR0 target for this thread's process address space.
        // If PM lock is contended, redirect to idle and requeue. The thread
        // will be rescheduled on the next timer tick (~5ms). Spinning here
        // wastes CPU cycles that other threads need for fork/exec to complete.
        let ttbr_result = set_next_ttbr0_for_thread(thread_id);
        match ttbr_result {
            TtbrResult::Ok => {
                switch_ttbr0_if_needed(thread_id);
            }
            TtbrResult::PmLockBusy => {
                TTBR_PM_LOCK_BUSY_COUNT.fetch_add(1, Ordering::Relaxed);
                // PM lock still held after retries — redirect to idle and requeue.
                // CRITICAL: Update cpu_state BEFORE requeue_thread_after_save,
                // because requeue checks cpu_state[].current_thread and silently
                // drops threads that appear to be running on any CPU.
                if let Some(thread) = sched.get_thread_mut(thread_id) {
                    thread.state = ThreadState::Ready;
                }
                setup_idle_return_locked(sched, frame, cpu_id);
                let idle_id = sched.cpu_state[cpu_id].idle_thread;
                sched.cpu_state[cpu_id].current_thread = Some(idle_id);
                sched.requeue_thread_after_save(thread_id);
                sched.set_need_resched_inner();
                return;
            }
            TtbrResult::ProcessGone => {
                TTBR_PROCESS_GONE_COUNT.fetch_add(1, Ordering::Relaxed);
                raw_uart_str("\n[TTBR_GONE] tid=");
                raw_uart_dec(thread_id);
                raw_uart_str(" elr=");
                raw_uart_hex(frame.elr);
                raw_uart_str(" cpu=");
                raw_uart_dec(cpu_id as u64);
                raw_uart_str("\n");
                // Process no longer exists — terminate orphaned thread.
                if let Some(thread) = sched.get_thread_mut(thread_id) {
                    thread.state = ThreadState::Terminated;
                }
                setup_idle_return_locked(sched, frame, cpu_id);
                let idle_id = sched.cpu_state[cpu_id].idle_thread;
                sched.cpu_state[cpu_id].current_thread = Some(idle_id);
                return;
            }
        }

        // CRITICAL: Set user_rsp_scratch to this thread's kernel stack top.
        unsafe {
            Aarch64PerCpu::set_user_rsp_scratch(Aarch64PerCpu::kernel_stack_top());
        }
    }
}

// =============================================================================
// Main entry point — single lock hold architecture
// =============================================================================

/// Check if rescheduling is needed and perform context switch if necessary.
///
/// This is called from the exception return path. The ENTIRE scheduling decision,
/// context save, and context restore happen under a SINGLE scheduler lock hold,
/// eliminating TOCTOU races from the previous 15-22 separate lock acquisitions.
#[no_mangle]
pub extern "C" fn check_need_resched_and_switch_arm64(
    frame: &mut Aarch64ExceptionFrame,
    from_el0: bool,
) {
    // ── Lock-free pre-checks ──────────────────────────────────────
    let preempt_count = Aarch64PerCpu::preempt_count();

    if (preempt_count & 0x10000000) != 0 {
        // PREEMPT_ACTIVE: in the middle of returning from a previous
        // exception — don't context switch now.
        return;
    }

    // Read deferred requeue atomically (lock-free).
    // CRITICAL: This must happen BEFORE the preempt_count early return below.
    // When IRQs are enabled during syscalls (daifclr #3 in syscall_entry.S),
    // timer interrupts fire from EL1 with preempt_count > 0. The old early
    // return skipped deferred requeue processing, causing threads to be
    // permanently lost in the DEFERRED_REQUEUE slot. By processing deferred
    // requeues here, threads are returned to the ready queue even when we
    // can't do a full context switch.
    let cpu_id = Aarch64PerCpu::cpu_id() as usize;
    let deferred_tid = if cpu_id < DEFERRED_REQUEUE.len() {
        DEFERRED_REQUEUE[cpu_id].swap(0, Ordering::Acquire)
    } else {
        0
    };

    // Process deferred requeue BEFORE checking preempt_count.
    // This is safe even when preempt_count > 0 because we only add the
    // thread to the ready queue — we don't context switch.
    let deferred_already_processed = if deferred_tid != 0 {
        // Need the scheduler lock to process the requeue.
        let mut guard = crate::task::scheduler::lock_for_context_switch();
        if let Some(sched) = guard.as_mut() {
            sched.cpu_state[cpu_id].previous_thread = None;
            sched.requeue_thread_after_save(deferred_tid);
        }
        drop(guard);
        true
    } else {
        false
    };

    if !from_el0 && (preempt_count & 0xFF) > 0 {
        // Kernel code holding locks — not safe to preempt.
        // Deferred requeue was already processed above.
        return;
    }

    // Check if reschedule is needed (atomic, clears the flag)
    let need_resched = crate::task::scheduler::check_and_clear_need_resched();

    // Read real_tid_fixup for stale cpu_state detection (lock-free)
    let real_tid_fixup = if from_el0 && (frame.spsr & 0xF) == 0 {
        let real_thread_ptr = Aarch64PerCpu::current_thread_ptr();
        if !real_thread_ptr.is_null() {
            let real_thread = unsafe { &*(real_thread_ptr as *const Thread) };
            Some(real_thread.id())
        } else {
            None
        }
    } else {
        None
    };

    // ── Fast path: skip lock when no scheduling work needed ─────
    // On every IRQ exit we reach here. 95%+ of the time there is no
    // reschedule pending and the current thread is not blocked. Reading
    // the thread state from the per-CPU pointer is lock-free and safe:
    // only this CPU modifies its own thread's state during syscalls, and
    // remote unblock (ISR buffer drain) only transitions Blocked→Ready
    // which is a no-op for our decision (we'd still skip).
    // When deferred_tid was non-zero, we already processed it above under
    // the lock and can treat it as "done" for fast-path eligibility.
    if !need_resched && (deferred_tid == 0 || deferred_already_processed) {
        let current_blocked = {
            let thread_ptr = Aarch64PerCpu::current_thread_ptr();
            if !thread_ptr.is_null() {
                let thread = unsafe { &*(thread_ptr as *const Thread) };
                matches!(
                    thread.state,
                    ThreadState::Blocked
                        | ThreadState::BlockedOnSignal
                        | ThreadState::BlockedOnChildExit
                        | ThreadState::BlockedOnTimer
                        | ThreadState::BlockedOnIO
                        | ThreadState::Terminated
                )
            } else {
                false
            }
        };

        if !current_blocked {
            // No work to do — return without acquiring the global lock.
            // fix_eret_cpu_state_locked is skipped; any stale cpu_state
            // will be corrected on the next tick that does acquire the lock.
            if from_el0 {
                check_and_deliver_signals_for_current_thread_arm64(frame);
                ensure_user_rsp_scratch_for_el0();
            }
            return;
        }
    }

    // ── Single lock acquisition ───────────────────────────────────
    let mut guard = crate::task::scheduler::lock_for_context_switch();
    let sched = match guard.as_mut() {
        Some(s) => s,
        None => return,
    };

    // 1. Process deferred requeue from PREVIOUS context switch.
    //    May have already been processed above (for the preempt_count > 0 path).
    //    Clear previous_thread unconditionally. If deferred_tid was already
    //    processed, requeue_thread_after_save is a no-op (thread already in queue).
    sched.cpu_state[cpu_id].previous_thread = None;
    if deferred_tid != 0 {
        sched.requeue_thread_after_save(deferred_tid);
    }

    // 2. Check if current thread is blocked or terminated
    //
    // NOTE: BlockedOnIO is intentionally included here. A thread that called
    // block_current_for_io() and then had a timer fire before it could execute
    // WFI is still "current" on this CPU but needs to be switched out so another
    // CPU's AHCI ISR can unblock it. Without this, need_resched=false would
    // prevent the switch and the thread would resume in its WFI loop — correct
    // behaviour — but if another thread is waiting in the ready queue the
    // BlockedOnIO thread would monopolise the CPU until need_resched is set by
    // the ISR-triggered unblock_for_io(). Including BlockedOnIO here ensures
    // the scheduler switches away from a blocked thread even when need_resched
    // is not yet set, matching the behaviour of all other Blocked* states.
    let current_blocked_or_terminated = if let Some(current) = sched.current_thread_mut() {
        matches!(
            current.state,
            ThreadState::Blocked
                | ThreadState::BlockedOnSignal
                | ThreadState::BlockedOnChildExit
                | ThreadState::BlockedOnTimer
                | ThreadState::BlockedOnIO
                | ThreadState::Terminated
        )
    } else {
        false
    };

    if !need_resched && !current_blocked_or_terminated {
        // No reschedule needed — fix cpu_state and return
        fix_eret_cpu_state_locked(sched, frame);
        drop(guard);
        if from_el0 {
            check_and_deliver_signals_for_current_thread_arm64(frame);
            ensure_user_rsp_scratch_for_el0();
        }
        return;
    }

    // 3. Fix stale cpu_state if needed (atomically with the scheduling decision)
    if let Some(real_tid) = real_tid_fixup {
        sched.fix_stale_idle_cpu_state(real_tid);
    }

    // 4. Scheduling decision (deferred requeue — old thread stays out of queue)
    let schedule_result = sched.schedule_deferred_requeue();

    if schedule_result.is_none() {
        fix_eret_cpu_state_locked(sched, frame);
        drop(guard);
        if from_el0 {
            check_and_deliver_signals_for_current_thread_arm64(frame);
            ensure_user_rsp_scratch_for_el0();
        }
        return;
    }

    let (old_id, new_id, should_requeue_old) = schedule_result.unwrap();

    if old_id == new_id {
        // Same thread continues running — requeue immediately since
        // no context switch happens (context is already correct)
        if should_requeue_old {
            sched.requeue_thread_after_save(old_id);
        }
        fix_eret_cpu_state_locked(sched, frame);
        drop(guard);
        if from_el0 {
            check_and_deliver_signals_for_current_thread_arm64(frame);
            ensure_user_rsp_scratch_for_el0();
        }
        return;
    }

    // 5. Trace context switch + queue state + increment watchdog counter
    trace_ctx_switch(old_id, new_id);
    crate::tracing::providers::sched::trace_sched_queue_state(
        sched.ready_queue_length() as u16,
        new_id as u16,
    );
    crate::task::scheduler::increment_context_switch_count();

    // 6. Save old thread context (INLINE — no lock acquisition)
    let is_old_idle = sched.is_idle_thread_inner(old_id);
    if from_el0 {
        if !is_old_idle {
            if let Some(old_thread) = sched.get_thread_mut(old_id) {
                save_userspace_context_inline(old_thread, frame);
            }
        }
        // else: idle thread with EL0 frame — skip save to prevent contamination
    } else {
        let frame_says_el0 = (frame.spsr & 0xF) == 0;
        if frame_says_el0 {
            if !is_old_idle {
                if let Some(old_thread) = sched.get_thread_mut(old_id) {
                    save_kernel_context_inline(old_thread, frame);
                }
            }
            // else: idle thread with EL0 frame + from_el0=false → corrupted, skip
        } else {
            if !is_old_idle {
                if let Some(old_thread) = sched.get_thread_mut(old_id) {
                    save_kernel_context_inline(old_thread, frame);
                }
            }
        }
    }

    // 7. Commit cpu_state to reflect the new thread as "current" on this CPU
    sched.commit_cpu_state_after_save(new_id);

    // Mark the old thread as "switching out" — its kernel stack is still in use
    // by this CPU until ERET completes. This prevents wakeup paths (unblock,
    // wake_expired_timers, etc.) from adding the old thread to the ready_queue,
    // which would allow another CPU to dispatch it while this CPU still has
    // stack frames on the same kernel stack, causing register/stack corruption.
    // Cleared at the start of the NEXT context switch on this CPU (step 1).
    if !sched.is_idle_thread_inner(old_id) {
        sched.cpu_state[cpu_id].previous_thread = Some(old_id);
    }

    // 8. Store deferred requeue for NEXT switch
    //    The exception frame lives on the old thread's kernel stack and must
    //    not be overwritten until after ERET.
    if cpu_id < DEFERRED_REQUEUE.len() {
        let previous = DEFERRED_REQUEUE[cpu_id].swap(old_id, Ordering::AcqRel);
        if previous != 0 {
            // A previously deferred requeue is being evicted before it was processed.
            // This means two rapid context switches happened on the same CPU without an
            // intervening check_need_resched_and_switch_arm64 call to drain the slot.
            // Requeue the evicted thread now (under the scheduler lock) and log the event.
            raw_uart_str("[DEFER_EVICT] cpu=");
            raw_uart_dec(cpu_id as u64);
            raw_uart_str(" evicted=");
            raw_uart_dec(previous);
            raw_uart_str(" new=");
            raw_uart_dec(old_id);
            raw_uart_str("\n");
            sched.requeue_thread_after_save(previous);
        }
    }

    // 9. Dispatch new thread (all checks + restore inside lock hold)
    dispatch_thread_locked(sched, new_id, frame, cpu_id);

    // Record dispatch trace AFTER all frame writes are complete.
    // This captures EXACTLY what the assembly ERET path will read.
    let idle_addr = crate::arch_impl::aarch64::idle_loop_arm64 as *const () as u64;
    let dispatch_path = if frame.elr == idle_addr {
        b'I' // redirected to idle
    } else if (frame.spsr & 0xF) == 0 {
        b'U' // userspace (EL0)
    } else {
        b'K' // kernel (EL1)
    };
    let dispatch_sp = unsafe {
        let base: u64;
        core::arch::asm!("mrs {}, tpidr_el1", out(reg) base, options(nomem, nostack));
        core::ptr::read_volatile((base + 40) as *const u64) // user_rsp_scratch
    };
    record_dispatch(
        cpu_id,
        old_id,
        new_id,
        frame.elr,
        frame.spsr,
        frame.x30,
        dispatch_sp,
        dispatch_path,
        from_el0,
    );

    // Also store in per-CPU fields for crash diagnostics
    unsafe {
        Aarch64PerCpu::set_dispatch_elr(frame.elr);
        Aarch64PerCpu::set_dispatch_spsr(frame.spsr);
    }

    // ── Release lock ──────────────────────────────────────────────
    drop(guard);

    // ── Lock-free post-switch ─────────────────────────────────────
    unsafe {
        Aarch64PerCpu::clear_preempt_active();
    }
    crate::arch_impl::aarch64::timer_interrupt::reset_quantum();
    crate::arch_impl::aarch64::timer_interrupt::rearm_timer();
}

extern "C" fn inline_schedule_trampoline() -> ! {
    let cpu_id = Aarch64PerCpu::cpu_id() as usize;
    let state = &INLINE_SCHEDULE_STATE[cpu_id];
    let sched_ptr = state.scheduler_ptr.swap(0, Ordering::Relaxed) as *mut Scheduler;
    let old_id = state.old_thread_id.load(Ordering::Relaxed);
    let new_id = state.new_thread_id.load(Ordering::Relaxed);
    let should_requeue_old = state.should_requeue_old.swap(false, Ordering::Relaxed);

    if sched_ptr.is_null() {
        idle_loop_arm64();
    }

    let sched = unsafe { &mut *sched_ptr };

    if let Some(old_thread) = sched.get_thread_mut(old_id) {
        // Resume after the inline-switch helper call when this thread is
        // eventually scheduled again.
        old_thread.context.elr_el1 = old_thread.context.x30;
    }

    sched.commit_cpu_state_after_save(new_id);
    sched.cpu_state[cpu_id].previous_thread = None;
    if should_requeue_old {
        sched.requeue_thread_after_save(old_id);
    }

    // Determine dispatch mode for the new thread.
    // Kernel threads that have started (saved context exists) use ret-based
    // dispatch to avoid ERET restoring PSTATE.I from the saved SPSR. This
    // prevents CPU IRQ death when a thread was interrupted inside
    // without_interrupts. User threads, idle, and first-run use ERET.
    let is_idle = sched.is_idle_thread_inner(new_id);
    let ret_dispatch_info = if !is_idle {
        sched.get_thread_mut(new_id).and_then(|t| {
            let is_kernel = t.privilege == ThreadPrivilege::Kernel;
            let has_started = t.has_started;
            let blocked_in_syscall = t.blocked_in_syscall;
            let is_kernel_mode = t.context.elr_el1 >= 0xFFFF_0000_0000_0000
                || (t.context.elr_el1 >= 0x4008_0000 && t.context.elr_el1 < 0xC000_0000);

            // Use ret-based dispatch for kernel threads and userspace threads
            // that are currently executing in kernel mode (blocked in syscall
            // or preempted during kernel execution).
            if has_started && (is_kernel || blocked_in_syscall || is_kernel_mode) {
                t.saved_by_inline_schedule = false;
                let thread_ptr = t as *const _ as *mut u8;
                let ctx_ptr = &t.context as *const CpuContext;
                let resume_pc = t.context.elr_el1;
                let kst = t.kernel_stack_top;
                let sp_el0 = t.context.sp_el0;
                Some((thread_ptr, ctx_ptr, resume_pc, kst, sp_el0))
            } else {
                None
            }
        })
    } else {
        None
    };

    if let Some((thread_ptr, ctx_ptr, resume_pc, kst, sp_el0)) = ret_dispatch_info {
        // ret-based dispatch: restore callee-saved regs + SP, branch to
        // resume_pc (= elr_el1). No ERET, no SPSR, no DAIF from the thread.
        // IRQs are enabled by the assembly before branching.
        unsafe {
            Aarch64PerCpu::set_current_thread_ptr(thread_ptr);
        }
        if let Some(kst) = kst {
            unsafe {
                Aarch64PerCpu::set_kernel_stack_top(kst.as_u64());
            }
        }
        if sp_el0 != 0 {
            unsafe {
                core::arch::asm!(
                    "msr sp_el0, {}",
                    in(reg) sp_el0,
                    options(nomem, nostack)
                );
            }
        }

        unsafe {
            crate::task::scheduler::force_unlock_scheduler();
        }

        crate::arch_impl::aarch64::timer_interrupt::reset_quantum();
        crate::arch_impl::aarch64::timer_interrupt::rearm_timer();

        unsafe {
            aarch64_ret_to_kernel_context(ctx_ptr, resume_pc);
        }
    }

    // ERET-based dispatch: for idle threads, user threads, and first-run
    // kernel threads that haven't been context-switched yet.
    let mut frame = unsafe { MaybeUninit::<Aarch64ExceptionFrame>::zeroed().assume_init() };

    dispatch_thread_locked(sched, new_id, &mut frame, cpu_id);

    let idle_addr = crate::arch_impl::aarch64::idle_loop_arm64 as *const () as u64;
    let dispatch_path = if frame.elr == idle_addr {
        b'I'
    } else if (frame.spsr & 0xF) == 0 {
        b'U'
    } else {
        b'K'
    };
    let dispatch_sp = unsafe {
        let base: u64;
        core::arch::asm!("mrs {}, tpidr_el1", out(reg) base, options(nomem, nostack));
        core::ptr::read_volatile((base + 40) as *const u64)
    };
    record_dispatch(
        cpu_id,
        old_id,
        new_id,
        frame.elr,
        frame.spsr,
        frame.x30,
        dispatch_sp,
        dispatch_path,
        false,
    );

    unsafe {
        Aarch64PerCpu::set_dispatch_elr(frame.elr);
        Aarch64PerCpu::set_dispatch_spsr(frame.spsr);
        crate::task::scheduler::force_unlock_scheduler();
    }

    crate::arch_impl::aarch64::timer_interrupt::reset_quantum();
    crate::arch_impl::aarch64::timer_interrupt::rearm_timer();

    unsafe {
        aarch64_enter_exception_frame(&frame as *const Aarch64ExceptionFrame);
    }
}

pub fn schedule_from_kernel() {
    let saved_daif = read_daif();
    unsafe {
        crate::arch_impl::aarch64::cpu::disable_interrupts();
    }

    let cpu_id = Aarch64PerCpu::cpu_id() as usize;
    let mut guard = crate::task::scheduler::lock_for_context_switch();
    let sched = match guard.as_mut() {
        Some(s) => s,
        None => {
            unsafe {
                core::arch::asm!("msr daifclr, #3; isb", options(nomem, nostack));
            }
            return;
        }
    };

    let deferred_tid = if cpu_id < DEFERRED_REQUEUE.len() {
        DEFERRED_REQUEUE[cpu_id].swap(0, Ordering::Acquire)
    } else {
        0
    };
    sched.cpu_state[cpu_id].previous_thread = None;
    if deferred_tid != 0 {
        sched.requeue_thread_after_save(deferred_tid);
    }

    let real_thread_ptr = Aarch64PerCpu::current_thread_ptr();
    if !real_thread_ptr.is_null() {
        let real_tid = unsafe { &*(real_thread_ptr as *const Thread) }.id();
        sched.fix_stale_idle_cpu_state(real_tid);
    }

    let schedule_result = sched.schedule_deferred_requeue();
    let Some((old_id, new_id, should_requeue_old)) = schedule_result else {
        drop(guard);
        unsafe {
            core::arch::asm!("msr daifclr, #3; isb", options(nomem, nostack));
        }
        return;
    };

    if old_id == new_id {
        if should_requeue_old {
            sched.requeue_thread_after_save(old_id);
        }
        drop(guard);
        unsafe {
            core::arch::asm!("msr daifclr, #3; isb", options(nomem, nostack));
        }
        return;
    }

    trace_ctx_switch(old_id, new_id);
    crate::tracing::providers::sched::trace_sched_queue_state(
        sched.ready_queue_length() as u16,
        new_id as u16,
    );
    crate::task::scheduler::increment_context_switch_count();

    let old_context_ptr = match sched.get_thread_mut(old_id) {
        Some(old_thread) => {
            old_thread.context.sp_el0 = read_sp_el0();
            old_thread.context.tpidr_el0 = read_tpidr_el0();
            old_thread.context.spsr_el1 = kernel_dispatch_spsr(saved_daif & 0x3C0);
            // Mark that this thread was saved by inline schedule, so the
            // dispatcher uses ret-based restore instead of ERET. This avoids
            // the CPU 0 IRQ death bug (ERET into without_interrupts code).
            old_thread.saved_by_inline_schedule = true;
            &mut old_thread.context as *mut CpuContext
        }
        None => {
            drop(guard);
            unsafe {
                core::arch::asm!("msr daifclr, #3; isb", options(nomem, nostack));
            }
            return;
        }
    };

    INLINE_SCHEDULE_STATE[cpu_id]
        .scheduler_ptr
        .store(sched as *mut Scheduler as usize, Ordering::Relaxed);
    INLINE_SCHEDULE_STATE[cpu_id]
        .old_thread_id
        .store(old_id, Ordering::Relaxed);
    INLINE_SCHEDULE_STATE[cpu_id]
        .new_thread_id
        .store(new_id, Ordering::Relaxed);
    INLINE_SCHEDULE_STATE[cpu_id]
        .should_requeue_old
        .store(should_requeue_old, Ordering::Relaxed);

    let _ = spin::MutexGuard::leak(guard);

    unsafe {
        aarch64_inline_schedule_switch(
            old_context_ptr,
            scheduler_stack_top(cpu_id),
            inline_schedule_trampoline,
        );
    }

    unsafe {
        // Execution reaches here when this thread is re-dispatched via
        // aarch64_ret_to_kernel_context (ret to saved x30).
        //
        // Like Linux's finish_task_switch: unconditionally enable IRQs.
        // schedule_from_kernel() always returns with IRQs enabled regardless
        // of the caller's IRQ state. This prevents the CPU 0 IRQ death bug
        // where a thread resumes inside a without_interrupts block and
        // re-masks DAIF.I permanently.
        core::arch::asm!("msr daifclr, #3; isb", options(nomem, nostack));
    }
}

// =============================================================================
// setup_idle_return_arm64 — used by signal delivery path (outside lock hold)
// =============================================================================

/// Set up exception frame to return to idle loop.
///
/// This version acquires its own scheduler lock and is used by the signal
/// delivery path which operates outside the consolidated context switch lock.
fn setup_idle_return_arm64(frame: &mut Aarch64ExceptionFrame) {
    // CRITICAL: Set frame ELR and SPSR to safe values FIRST
    frame.elr = idle_loop_arm64 as *const () as u64;
    frame.spsr = 0x5;

    // Get idle thread's kernel stack
    let idle_stack = crate::task::scheduler::with_scheduler(|sched| {
        let idle_id = sched.idle_thread();
        sched
            .get_thread(idle_id)
            .and_then(|t| t.kernel_stack_top.map(|v| v.as_u64()))
    })
    .flatten()
    .unwrap_or_else(|| {
        let cpu_id = Aarch64PerCpu::cpu_id() as u64;
        let boot_stack_top =
            super::constants::percpu_stack_region_base() + (cpu_id + 1) * 0x20_0000;
        boot_stack_top
    });

    // Clear all general purpose registers for clean state
    frame.x0 = 0;
    frame.x1 = 0;
    frame.x2 = 0;
    frame.x3 = 0;
    frame.x4 = 0;
    frame.x5 = 0;
    frame.x6 = 0;
    frame.x7 = 0;
    frame.x8 = 0;
    frame.x9 = 0;
    frame.x10 = 0;
    frame.x11 = 0;
    frame.x12 = 0;
    frame.x13 = 0;
    frame.x14 = 0;
    frame.x15 = 0;
    frame.x16 = 0;
    frame.x17 = 0;
    frame.x18 = 0;
    frame.x19 = 0;
    frame.x20 = 0;
    frame.x21 = 0;
    frame.x22 = 0;
    frame.x23 = 0;
    frame.x24 = 0;
    frame.x25 = 0;
    frame.x26 = 0;
    frame.x27 = 0;
    frame.x28 = 0;
    frame.x29 = 0;
    frame.x30 = 0;

    unsafe {
        Aarch64PerCpu::set_user_rsp_scratch(idle_stack);
        Aarch64PerCpu::set_kernel_stack_top(idle_stack);
        Aarch64PerCpu::set_current_thread_ptr(core::ptr::null_mut());
        Aarch64PerCpu::clear_preempt_active();
    }
}

// =============================================================================
// TTBR0 management
// =============================================================================

/// Switch TTBR0_EL1 if the thread requires a different address space.
fn switch_ttbr0_if_needed(_thread_id: u64) {
    let next_ttbr0 = Aarch64PerCpu::next_cr3();

    if next_ttbr0 == 0 {
        return;
    }

    let current_ttbr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) current_ttbr0, options(nomem, nostack));
    }

    if current_ttbr0 != next_ttbr0 {
        unsafe {
            core::arch::asm!(
                "dsb ishst",
                "msr ttbr0_el1, {}",
                "isb",
                "tlbi vmalle1is",
                "dsb ish",
                "isb",
                in(reg) next_ttbr0,
                options(nomem, nostack)
            );
        }

        unsafe {
            Aarch64PerCpu::set_saved_process_cr3(next_ttbr0);
        }
    }

    unsafe {
        Aarch64PerCpu::set_next_cr3(0);
    }
}

/// Result of attempting to set TTBR0 for a thread.
#[derive(PartialEq)]
enum TtbrResult {
    /// TTBR0 was successfully set.
    Ok,
    /// PM lock contended — temporary failure, safe to retry next tick.
    PmLockBusy,
    /// Process not found or has no page table — thread is orphaned.
    ProcessGone,
}

/// Determine and set the next TTBR0 value for a userspace thread.
///
/// Returns `TtbrResult::Ok` on success, `PmLockBusy` if the PM lock is held
/// (temporary, retry later), or `ProcessGone` if the thread's process no
/// longer exists (permanent — thread should be terminated).
///
/// CRITICAL: Uses try_manager() (non-blocking) instead of manager() to prevent
/// an AB-BA deadlock between PROCESS_MANAGER and SCHEDULER locks.
fn set_next_ttbr0_for_thread(thread_id: u64) -> TtbrResult {
    let manager_guard = match crate::process::try_manager() {
        Some(guard) => guard,
        None => {
            return TtbrResult::PmLockBusy;
        }
    };

    let next_ttbr0 = if let Some(ref manager) = *manager_guard {
        if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
            process
                .page_table
                .as_ref()
                .map(|pt| pt.level_4_frame().start_address().as_u64())
                .or(process.inherited_cr3)
        } else {
            // Thread's process not found — orphaned thread.
            // Diagnostic: dump all process thread IDs to identify the mismatch.
            raw_uart_str("\n[TTBR_DIAG] wanted_tid=");
            raw_uart_dec(thread_id);
            raw_uart_str(" nproc=");
            raw_uart_dec(manager.process_count() as u64);
            for (pid, proc) in manager.iter_processes() {
                raw_uart_str(" p");
                raw_uart_dec(pid.as_u64());
                raw_uart_str(":t");
                match proc.main_thread.as_ref() {
                    Some(t) => raw_uart_dec(t.id),
                    None => raw_uart_str("X"),
                }
            }
            raw_uart_str("\n");
            drop(manager_guard);
            return TtbrResult::ProcessGone;
        }
    } else {
        // Process manager not initialized yet
        drop(manager_guard);
        return TtbrResult::ProcessGone;
    };

    drop(manager_guard);

    if let Some(ttbr0) = next_ttbr0 {
        // Tag TTBR0 with ASID=1 so stale boot identity map TLB entries
        // (ASID=0) don't match user VA accesses. Combined with nG bits on
        // process page table entries, this ensures ASID-based separation.
        let tagged_ttbr0 = ttbr0 | (1u64 << 48); // ASID=1 in bits [55:48]
        unsafe {
            Aarch64PerCpu::set_next_cr3(tagged_ttbr0);
        }
        TtbrResult::Ok
    } else {
        // Process exists but has no page table — shouldn't happen
        TtbrResult::ProcessGone
    }
}

// =============================================================================
// Idle loop and low-level context switch primitives
// =============================================================================

/// ARM64 idle loop - wait for interrupts.
#[no_mangle]
pub extern "C" fn idle_loop_arm64() -> ! {
    loop {
        unsafe {
            core::arch::asm!(
                "msr daifclr, #0xf", // Enable all interrupts
                "wfi",               // Wait for interrupt
                options(nomem, nostack)
            );
        }
    }
}

/// Perform a context switch between two threads using the low-level
/// assembly switch_context function.
#[allow(dead_code)]
pub unsafe fn perform_context_switch(old_context: &mut CpuContext, new_context: &CpuContext) {
    super::context::switch_context(
        old_context as *mut CpuContext,
        new_context as *const CpuContext,
    );
}

/// Switch to a new thread for the first time (doesn't save current context).
#[allow(dead_code)]
pub unsafe fn switch_to_new_thread(context: &CpuContext) -> ! {
    super::context::switch_to_thread(context as *const CpuContext)
}

/// Switch to userspace using ERET.
#[allow(dead_code)]
pub unsafe fn switch_to_user(context: &CpuContext) -> ! {
    super::context::switch_to_user(context as *const CpuContext)
}

// =============================================================================
// Boot markers
// =============================================================================

/// Marker for boot stage completion (mirrors x86_64 pattern).
static SCHEDULE_MARKER_EMITTED: AtomicBool = AtomicBool::new(false);

/// Emit one-time boot marker when scheduler first runs.
#[allow(dead_code)]
fn emit_schedule_boot_marker() {
    if !SCHEDULE_MARKER_EMITTED.swap(true, Ordering::Relaxed) {
        raw_uart_str("[ INFO] scheduler::schedule() returned (boot marker)\n");
    }
}

/// One-time EL0 entry marker.
static EMITTED_EL0_MARKER: AtomicBool = AtomicBool::new(false);

/// Emit one-time marker when first entering EL0 (userspace).
#[allow(dead_code)]
fn emit_el0_entry_marker() {
    if !EMITTED_EL0_MARKER.swap(true, Ordering::Relaxed) {
        raw_uart_str("EL0_ENTER: First userspace entry\n");
        raw_uart_str("[ OK ] EL0_SMOKE: userspace executed + syscall path verified\n");
    }
}

// =============================================================================
// ARM64 Signal Delivery
// =============================================================================

/// Check and deliver pending signals for the current thread (ARM64)
///
/// Called when returning to userspace (EL0) without a context switch.
/// This ensures signals are delivered promptly even when the same thread keeps running.
///
/// NOTE: This function acquires its own locks (SCHEDULER for current_thread_id,
/// PROCESS_MANAGER for signal delivery). It is called AFTER the consolidated
/// context switch lock is released.
fn check_and_deliver_signals_for_current_thread_arm64(frame: &mut Aarch64ExceptionFrame) {
    // Get current thread ID
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return,
    };

    // Thread 0 is the idle thread - it doesn't have a process with signals
    if current_thread_id == 0 {
        return;
    }

    // Try to acquire process manager lock
    let mut manager_guard = match crate::process::try_manager() {
        Some(guard) => guard,
        None => return, // Lock held, skip signal check this time
    };

    // Track if signal termination happened (for parent notification after borrow ends)
    let mut signal_termination_info: Option<crate::signal::delivery::ParentNotification> = None;
    let mut terminated_child_pid: Option<u64> = None;

    if let Some(ref mut manager) = *manager_guard {
        // Find the process for this thread
        if let Some((_pid, process)) = manager.find_process_by_thread_mut(current_thread_id) {
            // Check for expired timers
            crate::signal::delivery::check_and_fire_alarm(process);
            crate::signal::delivery::check_and_fire_itimer_real(process, 5000);

            if crate::signal::delivery::has_deliverable_signals(process) {
                // Read current SP_EL0 (user stack pointer)
                let sp_el0: u64;
                unsafe {
                    core::arch::asm!("mrs {}, sp_el0", out(reg) sp_el0, options(nomem, nostack));
                }

                // Switch to process's page table for signal delivery
                if let Some(ref page_table) = process.page_table {
                    let raw_ttbr0 = page_table.level_4_frame().start_address().as_u64();
                    unsafe {
                        core::arch::asm!(
                            "dsb ishst",
                            "msr ttbr0_el1, {}",
                            "isb",
                            "tlbi vmalle1is",
                            "dsb ish",
                            "isb",
                            in(reg) raw_ttbr0,
                            options(nomem, nostack)
                        );
                    }
                }

                // Create SavedRegisters from exception frame for signal delivery
                let mut saved_regs = create_saved_regs_from_frame(frame, sp_el0);

                // Deliver signals
                let signal_result = crate::signal::delivery::deliver_pending_signals(
                    process,
                    frame,
                    &mut saved_regs,
                );

                // If signals were delivered, update SP_EL0 with new stack pointer
                if !matches!(
                    signal_result,
                    crate::signal::delivery::SignalDeliveryResult::NoAction
                ) {
                    unsafe {
                        core::arch::asm!(
                            "msr sp_el0, {}",
                            in(reg) saved_regs.sp,
                            options(nomem, nostack)
                        );
                    }
                }

                match signal_result {
                    crate::signal::delivery::SignalDeliveryResult::Terminated(notification) => {
                        crate::task::scheduler::set_need_resched();
                        terminated_child_pid = Some(notification.child_pid.as_u64());
                        signal_termination_info = Some(notification);
                        setup_idle_return_arm64(frame);
                        crate::task::scheduler::switch_to_idle();
                    }
                    crate::signal::delivery::SignalDeliveryResult::Delivered => {
                        if process.is_terminated() {
                            terminated_child_pid = Some(process.id.as_u64());
                            crate::task::scheduler::set_need_resched();
                            setup_idle_return_arm64(frame);
                            crate::task::scheduler::switch_to_idle();
                        }
                    }
                    crate::signal::delivery::SignalDeliveryResult::NoAction => {}
                }
            }
        }

        // Drop manager guard first to avoid deadlock when notifying parent
        drop(manager_guard);

        // Notify parent if signal terminated a child
        if let Some(notification) = signal_termination_info {
            crate::signal::delivery::notify_parent_of_termination_deferred(&notification);
        }

        // Clean up window buffers so compositor stops reading freed pages
        if let Some(pid) = terminated_child_pid {
            crate::syscall::graphics::cleanup_windows_for_pid(pid);
        }
    }
}

/// Create SavedRegisters from an Aarch64ExceptionFrame and SP_EL0
pub fn create_saved_regs_from_frame(
    frame: &Aarch64ExceptionFrame,
    sp_el0: u64,
) -> crate::task::process_context::SavedRegisters {
    crate::task::process_context::SavedRegisters {
        x0: frame.x0,
        x1: frame.x1,
        x2: frame.x2,
        x3: frame.x3,
        x4: frame.x4,
        x5: frame.x5,
        x6: frame.x6,
        x7: frame.x7,
        x8: frame.x8,
        x9: frame.x9,
        x10: frame.x10,
        x11: frame.x11,
        x12: frame.x12,
        x13: frame.x13,
        x14: frame.x14,
        x15: frame.x15,
        x16: frame.x16,
        x17: frame.x17,
        x18: frame.x18,
        x19: frame.x19,
        x20: frame.x20,
        x21: frame.x21,
        x22: frame.x22,
        x23: frame.x23,
        x24: frame.x24,
        x25: frame.x25,
        x26: frame.x26,
        x27: frame.x27,
        x28: frame.x28,
        x29: frame.x29,
        x30: frame.x30,
        sp: sp_el0,
        elr: frame.elr,
        spsr: frame.spsr,
    }
}
