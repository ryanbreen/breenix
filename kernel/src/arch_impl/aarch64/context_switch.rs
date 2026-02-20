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

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::exception_frame::Aarch64ExceptionFrame;
use super::percpu::Aarch64PerCpu;
use crate::arch_impl::traits::PerCpuOps;
use crate::task::scheduler::Scheduler;
use crate::task::thread::{CpuContext, Thread, ThreadPrivilege, ThreadState};
use crate::tracing::providers::sched::trace_ctx_switch;

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
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
];

// =============================================================================
// Per-CPU dispatch trace ring buffer — diagnostic instrumentation
//
// Records the last DISPATCH_RING_SIZE dispatches per CPU. On crash, the
// exception handler calls dump_dispatch_trace() to show exactly what
// context was dispatched before the fault.
// =============================================================================

const DISPATCH_RING_SIZE: usize = 8;
const MAX_CPUS_TRACE: usize = 4;

/// One dispatch event — what was written to the exception frame.
#[repr(C)]
struct DispatchEntry {
    tid: u64,
    old_tid: u64,
    elr: u64,
    spsr: u64,
    x30: u64,
    sp: u64,
    path: u8,    // K=kernel, U=userspace, I=idle, F=first_entry, B=BUG-terminated
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
        tid: 0, old_tid: 0, elr: 0, spsr: 0, x30: 0, sp: 0, path: 0, from_el0: 0,
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
fn record_dispatch(cpu_id: usize, old_tid: u64, tid: u64, elr: u64, spsr: u64, x30: u64, sp: u64, path: u8, from_el0: bool) {
    if cpu_id >= MAX_CPUS_TRACE { return; }
    unsafe {
        let ring = &mut DISPATCH_TRACE[cpu_id];
        let idx = ring.write_idx;
        ring.entries[idx] = DispatchEntry {
            tid, old_tid, elr, spsr, x30, sp, path, from_el0: from_el0 as u8,
        };
        ring.write_idx = (idx + 1) % DISPATCH_RING_SIZE;
        if ring.count < DISPATCH_RING_SIZE { ring.count += 1; }
    }
}

/// Dump the dispatch trace for a specific CPU. Called from the crash handler.
pub fn dump_dispatch_trace(cpu_id: usize) {
    if cpu_id >= MAX_CPUS_TRACE { return; }
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
    thread.context.spsr_el1 = frame.spsr;

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
        thread.context.elr_el1 = thread.context.x30;  // Entry point
        thread.context.spsr_el1 = 0x5;  // EL1h, interrupts enabled
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
        addr >= KERNEL_VIRT_BASE
            || (addr >= KERNEL_PHYS_BASE && addr < KERNEL_PHYS_LIMIT)
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
        raw_uart_char(if thread.blocked_in_syscall { b'1' } else { b'0' });
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
        frame.elr = thread.context.x30;  // First run: jump to entry point
        frame.spsr = 0x5;  // EL1h, DAIF clear (interrupts enabled)
    } else if is_kernel_addr(thread.context.elr_el1) {
        // Resume: return to where we left off.
        // On QEMU, kernel addresses are >= KERNEL_VIRT_BASE (HHDM).
        // On Parallels, kernel runs identity-mapped at physical addresses
        // (KERNEL_PHYS_BASE..KERNEL_PHYS_LIMIT), so we must accept both.
        frame.elr = thread.context.elr_el1;
        frame.spsr = thread.context.spsr_el1;  // Restore saved processor state
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
fn restore_userspace_context_inline(thread: &mut Thread, frame: &mut Aarch64ExceptionFrame) {
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
    frame.spsr = thread.context.spsr_el1;

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
        core::arch::asm!(
            "msr tpidr_el0, xzr",
            options(nomem, nostack)
        );
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
    let idle_stack = sched.get_thread(idle_id)
        .and_then(|t| t.kernel_stack_top.map(|v| v.as_u64()))
        .unwrap_or_else(|| {
            let cpu_id64 = cpu_id as u64;
            0xFFFF_0000_0000_0000u64 + 0x4100_0000 + (cpu_id64 + 1) * 0x20_0000
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
        let has_saved_context = sched.get_thread(thread_id).map(|thread| {
            let elr = thread.context.elr_el1;
            let sp = thread.context.sp;
            let spsr = thread.context.spsr_el1;
            let elr_is_kernel = elr >= KERNEL_VIRT_BASE
                || (elr >= KERNEL_PHYS_BASE && elr < KERNEL_PHYS_LIMIT);
            let sp_is_kernel = sp >= KERNEL_VIRT_BASE
                || (sp >= KERNEL_PHYS_BASE && sp < KERNEL_PHYS_LIMIT);
            let near_idle = elr >= idle_loop_addr && elr < idle_loop_addr + 16;
            elr_is_kernel
                && !near_idle
                && sp_is_kernel
                && (spsr & 0xF) != 0
        }).unwrap_or(false);

        if has_saved_context {
            let ok = sched.get_thread_mut(thread_id)
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
        (state, privilege, blocked_in_syscall, has_started, elr, kernel_stack_top, thread_ptr)
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

        let restore_ok = sched.get_thread_mut(thread_id)
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

    if !from_el0 && (preempt_count & 0xFF) > 0 {
        // Kernel code holding locks — not safe to preempt
        return;
    }

    // Read deferred requeue atomically (lock-free)
    let cpu_id = Aarch64PerCpu::cpu_id() as usize;
    let deferred_tid = if cpu_id < DEFERRED_REQUEUE.len() {
        DEFERRED_REQUEUE[cpu_id].swap(0, Ordering::Acquire)
    } else {
        0
    };

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

    // ── Single lock acquisition ───────────────────────────────────
    let mut guard = crate::task::scheduler::lock_for_context_switch();
    let sched = match guard.as_mut() {
        Some(s) => s,
        None => return,
    };

    // 1. Process deferred requeue from PREVIOUS context switch.
    //    Safe because we're now on the current thread's kernel stack
    //    (ERET from the previous switch has completed).
    //    Clear previous_thread FIRST so wakeup paths know the old thread's
    //    kernel stack is free and the thread can be safely dispatched.
    sched.cpu_state[cpu_id].previous_thread = None;
    if deferred_tid != 0 {
        sched.requeue_thread_after_save(deferred_tid);
    }

    // 2. Check if current thread is blocked or terminated
    let current_blocked_or_terminated = if let Some(current) = sched.current_thread_mut() {
        matches!(
            current.state,
            ThreadState::Blocked
                | ThreadState::BlockedOnSignal
                | ThreadState::BlockedOnChildExit
                | ThreadState::BlockedOnTimer
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

    // 5. Trace context switch + increment watchdog counter
    trace_ctx_switch(old_id, new_id);
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
            if let Some(old_thread) = sched.get_thread_mut(old_id) {
                save_kernel_context_inline(old_thread, frame);
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
    record_dispatch(cpu_id, old_id, new_id, frame.elr, frame.spsr, frame.x30, dispatch_sp, dispatch_path, from_el0);

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
        let boot_stack_top = 0xFFFF_0000_0000_0000u64 + 0x4100_0000 + (cpu_id + 1) * 0x20_0000;
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
                "msr daifclr, #0xf",  // Enable all interrupts
                "wfi",                 // Wait for interrupt
                options(nomem, nostack)
            );
        }
    }
}

/// Perform a context switch between two threads using the low-level
/// assembly switch_context function.
#[allow(dead_code)]
pub unsafe fn perform_context_switch(
    old_context: &mut CpuContext,
    new_context: &CpuContext,
) {
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
                if !matches!(signal_result, crate::signal::delivery::SignalDeliveryResult::NoAction) {
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
                        signal_termination_info = Some(notification);
                        setup_idle_return_arm64(frame);
                        crate::task::scheduler::switch_to_idle();
                    }
                    crate::signal::delivery::SignalDeliveryResult::Delivered => {
                        if process.is_terminated() {
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
