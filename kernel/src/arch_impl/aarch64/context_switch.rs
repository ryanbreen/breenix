//! ARM64 context switching logic.
//!
//! This module handles context switching on ARM64 (AArch64) when returning from
//! exceptions or performing explicit thread switches. It integrates with the
//! scheduler to perform preemptive multitasking.
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
use crate::task::thread::{CpuContext, ThreadPrivilege, ThreadState};

/// Raw serial debug output - single character, no locks, no allocations.
/// Use this for debugging context switch paths where any allocation/locking
/// could perturb timing or cause deadlocks.
#[inline(always)]
#[allow(dead_code)]
pub fn raw_uart_char(c: u8) {
    // QEMU virt machine UART via HHDM (TTBR1-mapped, safe during context switch)
    // Physical 0x0900_0000 is mapped at HHDM_BASE + 0x0900_0000
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    const UART_VIRT: u64 = HHDM_BASE + 0x0900_0000;
    unsafe {
        let ptr = UART_VIRT as *mut u8;
        core::ptr::write_volatile(ptr, c);
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

/// Check if rescheduling is needed and perform context switch if necessary.
///
/// This is called from the exception return path and is the CORRECT place
/// to handle context switching (not in the exception handler itself).
///
/// # Arguments
///
/// * `frame` - The exception frame containing saved registers
/// * `from_el0` - Whether the exception came from EL0 (userspace)
#[no_mangle]
pub extern "C" fn check_need_resched_and_switch_arm64(
    frame: &mut Aarch64ExceptionFrame,
    from_el0: bool,
) {
    // Check PREEMPT_ACTIVE flag (bit 28) and preempt count (bits 0-7)
    let preempt_count = Aarch64PerCpu::preempt_count();
    let preempt_active = (preempt_count & 0x10000000) != 0;
    let preempt_depth = preempt_count & 0xFF;

    if preempt_active {
        // We're in the middle of returning from a syscall or handling
        // another exception - don't context switch now
        return;
    }

    // For kernel mode (EL1) returns, only allow preemption if no locks are held.
    // preempt_depth > 0 means we're holding a spinlock - NOT safe to switch.
    // This allows kthreads (e.g., test threads, workqueue workers) to be scheduled
    // when they're in a safe state (like spinning in kthread_join with WFI).
    if !from_el0 && preempt_depth > 0 {
        // Kernel code holding locks - not safe to preempt
        return;
    }

    // Check if current thread is blocked or terminated
    let current_thread_blocked_or_terminated = crate::task::scheduler::with_scheduler(|sched| {
        if let Some(current) = sched.current_thread_mut() {
            matches!(
                current.state,
                ThreadState::Blocked
                    | ThreadState::BlockedOnSignal
                    | ThreadState::BlockedOnChildExit
                    | ThreadState::Terminated
            )
        } else {
            false
        }
    })
    .unwrap_or(false);

    // Check if reschedule is needed
    let need_resched = crate::task::scheduler::check_and_clear_need_resched();
    if !need_resched && !current_thread_blocked_or_terminated {
        // No reschedule needed
        if from_el0 {
            // Check for pending signals before returning to userspace
            check_and_deliver_signals_for_current_thread_arm64(frame);
        }
        return;
    }

    // Track reschedule attempts for diagnostics
    static RESCHED_COUNTER: AtomicU64 = AtomicU64::new(0);
    let _count = RESCHED_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Perform scheduling decision
    let schedule_result = crate::task::scheduler::schedule();

    // Handle "no switch needed" case
    if schedule_result.is_none() {
        // Even though no context switch happens, check for signals
        // when returning to userspace
        if from_el0 {
            check_and_deliver_signals_for_current_thread_arm64(frame);
        }
        return;
    }

    if let Some((old_thread_id, new_thread_id)) = schedule_result {
        if old_thread_id == new_thread_id {
            // Same thread continues running, but check for pending signals
            if from_el0 {
                check_and_deliver_signals_for_current_thread_arm64(frame);
            }
            return;
        }

        // Save current thread's context
        if from_el0 {
            save_userspace_context_arm64(old_thread_id, frame);
        } else {
            save_kernel_context_arm64(old_thread_id, frame);
        }

        // Switch to the new thread
        switch_to_thread_arm64(new_thread_id, frame);

        // Clear PREEMPT_ACTIVE after context switch
        unsafe {
            Aarch64PerCpu::clear_preempt_active();
        }

        // Reset timer quantum for the new thread
        crate::arch_impl::aarch64::timer_interrupt::reset_quantum();

        // NOTE: Do NOT use serial_println! here - causes deadlock if timer fires
        // while boot code holds serial lock. Use raw_uart_char for debugging only.
    }
}

/// Save userspace context for the current thread.
fn save_userspace_context_arm64(thread_id: u64, frame: &Aarch64ExceptionFrame) {
    crate::task::scheduler::with_thread_mut(thread_id, |thread| {
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

        // CRITICAL: Save kernel stack pointer for blocked-in-syscall restoration.
        // When a userspace thread later blocks in a syscall (e.g., read() waiting
        // for input), it transitions to kernel mode and sits in a WFI loop. If a
        // context switch occurs and the thread is later restored via
        // setup_kernel_thread_return_arm64, that function uses context.sp for
        // user_rsp_scratch (the kernel SP after ERET). Without saving context.sp
        // here, it retains its initial value (0 for user threads), causing SP
        // corruption and crashes when the thread is restored on another CPU.
        thread.context.sp = frame as *const _ as u64 + 272;
    });
}

/// Save kernel context for the current thread.
fn save_kernel_context_arm64(thread_id: u64, frame: &Aarch64ExceptionFrame) {
    crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        // Save ALL general-purpose registers, not just callee-saved.
        // This is critical for context switching: when a thread is preempted in the
        // middle of a loop (like kthread_join's WFI loop), its caller-saved registers
        // (x0-x18) contain important values (loop variables, pointers, etc.).
        // Without saving these, resuming the thread would have garbage in x0-x18.
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
        // This is critical for resuming the thread at the correct stack position.
        thread.context.sp = frame as *const _ as u64 + 272;

        // Also save SP_EL0 for userspace threads blocked in syscall.
        // SP_EL0 holds the user stack pointer even when in kernel mode.
        let sp_el0: u64;
        unsafe {
            core::arch::asm!("mrs {}, sp_el0", out(reg) sp_el0, options(nomem, nostack));
        }
        thread.context.sp_el0 = sp_el0;
    });
}

/// Switch to a different thread.
fn switch_to_thread_arm64(thread_id: u64, frame: &mut Aarch64ExceptionFrame) {
    // Update per-CPU current thread pointer
    crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        let thread_ptr = thread as *const _ as *mut crate::task::thread::Thread;
        unsafe {
            Aarch64PerCpu::set_current_thread_ptr(thread_ptr as *mut u8);
        }

        // Update kernel stack for exceptions
        if let Some(kernel_stack_top) = thread.kernel_stack_top {
            unsafe {
                Aarch64PerCpu::set_kernel_stack_top(kernel_stack_top.as_u64());
            }
        }
    });

    // Check thread properties
    let is_idle = crate::task::scheduler::with_scheduler(|sched| thread_id == sched.idle_thread())
        .unwrap_or(false);

    let is_kernel_thread =
        crate::task::scheduler::with_thread_mut(thread_id, |thread| {
            thread.privilege == ThreadPrivilege::Kernel
        })
        .unwrap_or(false);

    // CRITICAL: Check if thread is blocked in a syscall.
    // A userspace thread blocked in syscall is temporarily in kernel mode (EL1).
    // We must restore kernel context (SP, ELR pointing to kernel code), NOT
    // userspace context. Using restore_userspace_context_arm64 would:
    // 1. Restore SP_EL0 (wrong - we need kernel SP)
    // 2. Not set user_rsp_scratch for kernel SP restoration
    // This causes stack corruption and crashes when the syscall eventually returns.
    let is_blocked_in_syscall =
        crate::task::scheduler::with_thread_mut(thread_id, |thread| {
            thread.blocked_in_syscall
        })
        .unwrap_or(false);

    // CRITICAL: Detect User threads that were interrupted while executing in
    // kernel mode (e.g., in the SVC return path after a blocking syscall cleared
    // blocked_in_syscall but before ERET). If context.elr is in kernel space
    // (>= 0xFFFF000000000000), the thread must be dispatched via
    // setup_kernel_thread_return_arm64 which uses context.sp (the actual kernel
    // stack position) for user_rsp_scratch. Without this, restore_userspace_context
    // sets user_rsp_scratch = kernel_stack_top, causing SP to be 272 bytes above
    // the SVC frame, and the SVC return path reads zeroed memory for ELR/SPSR.
    const KERNEL_VIRT_BASE: u64 = 0xFFFF_0000_0000_0000;
    let is_in_kernel_mode =
        crate::task::scheduler::with_thread_mut(thread_id, |thread| {
            thread.context.elr_el1 >= KERNEL_VIRT_BASE
        })
        .unwrap_or(false);

    if is_idle {
        // Check if idle thread has a saved context to restore
        // If it was preempted while running actual code (not idle_loop_arm64), restore that context
        //
        // This is critical for kthread_join(): when the boot thread (which IS the idle task)
        // calls kthread_join() and waits, it gets preempted. When all kthreads finish and we
        // switch back to idle, we need to restore the boot thread's context (sitting in
        // kthread_join's WFI loop) rather than jumping to idle_loop_arm64.
        let idle_loop_addr = idle_loop_arm64 as *const () as u64;
        let has_saved_context = crate::task::scheduler::with_thread_mut(thread_id, |thread| {
            // Has saved context if ELR is non-zero AND not pointing to idle_loop_arm64
            thread.context.elr_el1 != 0 && thread.context.elr_el1 != idle_loop_addr
        }).unwrap_or(false);

        if has_saved_context {
            // Restore idle thread's saved context (like a kthread)
            setup_kernel_thread_return_arm64(thread_id, frame);
        } else {
            // No saved context or was in idle_loop - go to idle loop
            setup_idle_return_arm64(frame);
        }
    } else if is_kernel_thread || is_blocked_in_syscall || is_in_kernel_mode {
        // Kernel threads, userspace threads blocked in syscall, and userspace
        // threads interrupted while in kernel mode all need kernel context
        // restoration (they're running in kernel mode with a kernel SP)
        setup_kernel_thread_return_arm64(thread_id, frame);

        // CRITICAL: For userspace threads in kernel mode, set up TTBR0 so
        // the correct process page table is active when the syscall completes
        // and returns to userspace. Without this, TTBR0 retains the previously-
        // running process's page table, causing instruction aborts when the
        // thread returns to EL0 with the wrong address space.
        if (is_blocked_in_syscall || is_in_kernel_mode) && !is_kernel_thread {
            set_next_ttbr0_for_thread(thread_id);
            switch_ttbr0_if_needed(thread_id);
        }
    } else {
        restore_userspace_context_arm64(thread_id, frame);
    }
}

/// Set up exception frame to return to idle loop.
fn setup_idle_return_arm64(frame: &mut Aarch64ExceptionFrame) {
    // Get idle thread's kernel stack (its per-CPU boot stack).
    // CRITICAL: Must NOT fall back to Aarch64PerCpu::kernel_stack_top() —
    // that value belongs to the last dispatched thread and would cause the idle
    // loop to run on that thread's kernel stack, corrupting its SVC frame.
    let idle_stack = crate::task::scheduler::with_scheduler(|sched| {
        let idle_id = sched.idle_thread();
        sched
            .get_thread(idle_id)
            .and_then(|t| t.kernel_stack_top.map(|v| v.as_u64()))
    })
    .flatten()
    .unwrap_or_else(|| {
        // Fallback: compute this CPU's boot stack from CPU ID.
        // Boot stacks: HHDM_BASE + 0x4100_0000 + (cpu_id + 1) * 0x20_0000
        let cpu_id = Aarch64PerCpu::cpu_id() as u64;
        let boot_stack_top = 0xFFFF_0000_0000_0000u64 + 0x4100_0000 + (cpu_id + 1) * 0x20_0000;
        raw_uart_char(b'!'); // Should not normally reach here
        boot_stack_top
    });

    // Set up exception return to idle_loop
    frame.elr = idle_loop_arm64 as *const () as u64;
    // SPSR for EL1h with interrupts enabled
    // M[3:0] = 0b0101 (EL1h), DAIF = 0 (interrupts enabled)
    frame.spsr = 0x5;

    // Kernel stack pointer will be restored from exception frame
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

    // Store idle stack in SP_EL0 scratch area for use after ERET
    unsafe {
        Aarch64PerCpu::set_user_rsp_scratch(idle_stack);
    }

    // Clear PREEMPT_ACTIVE when switching to idle
    unsafe {
        Aarch64PerCpu::clear_preempt_active();
    }

    // NOTE: No logging here - context switch path must be lock-free
}

/// Set up exception frame to return to kernel thread.
fn setup_kernel_thread_return_arm64(thread_id: u64, frame: &mut Aarch64ExceptionFrame) {
    // Check if this thread has run before
    let has_started = crate::task::scheduler::with_thread_mut(thread_id, |thread| thread.has_started)
        .unwrap_or(false);

    if !has_started {
        // First run - mark as started and set up entry point.
        // CRITICAL: Also initialize elr_el1 and spsr_el1 to safe values.
        // Without this, if the thread parks and is re-dispatched before
        // save_kernel_context_arm64 runs (SMP race: another CPU unblocks
        // the thread before this CPU's timer fires), elr_el1 remains 0
        // from CpuContext::new_kernel_thread, causing ERET to address 0x0
        // and an INSTRUCTION_ABORT.
        crate::task::scheduler::with_thread_mut(thread_id, |thread| {
            thread.has_started = true;
            thread.context.elr_el1 = thread.context.x30;  // Entry point
            thread.context.spsr_el1 = 0x5;  // EL1h, interrupts enabled
        });
    }

    let thread_info = crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        (thread.name.clone(), thread.context.clone())
    });

    if let Some((_name, context)) = thread_info {
        // Restore ALL general-purpose registers for resumed threads.
        // For first-time threads, x0 contains the entry argument and other registers
        // are undefined (will be initialized by the thread function).
        // For resumed threads, ALL registers must be restored exactly as they were
        // when the thread was preempted (e.g., loop variables, pointers, etc.).
        frame.x0 = context.x0;
        frame.x1 = context.x1;
        frame.x2 = context.x2;
        frame.x3 = context.x3;
        frame.x4 = context.x4;
        frame.x5 = context.x5;
        frame.x6 = context.x6;
        frame.x7 = context.x7;
        frame.x8 = context.x8;
        frame.x9 = context.x9;
        frame.x10 = context.x10;
        frame.x11 = context.x11;
        frame.x12 = context.x12;
        frame.x13 = context.x13;
        frame.x14 = context.x14;
        frame.x15 = context.x15;
        frame.x16 = context.x16;
        frame.x17 = context.x17;
        frame.x18 = context.x18;
        frame.x19 = context.x19;
        frame.x20 = context.x20;
        frame.x21 = context.x21;
        frame.x22 = context.x22;
        frame.x23 = context.x23;
        frame.x24 = context.x24;
        frame.x25 = context.x25;
        frame.x26 = context.x26;
        frame.x27 = context.x27;
        frame.x28 = context.x28;
        frame.x29 = context.x29;
        frame.x30 = context.x30;

        // Set return address:
        // - For first run: use x30 (which is kthread_entry)
        // - For resumed thread: use elr_el1 (saved PC from when interrupted)
        //
        // Defensive check: if elr_el1 is 0 for a started thread, fall back
        // to x30 (entry point) to avoid ERET to address 0x0. This can happen
        // if a kernel thread parks and is re-dispatched before
        // save_kernel_context_arm64 saves its context.
        if !has_started {
            frame.elr = context.x30;  // First run: jump to entry point
        } else if context.elr_el1 != 0 {
            frame.elr = context.elr_el1;  // Resume: return to where we left off
        } else {
            // BUG: Thread was dispatched with unsaved context. Fall back to
            // entry point rather than crashing at address 0x0.
            raw_uart_str("WARN: elr=0 for started kthread, using x30\n");
            frame.elr = context.x30;
        }

        // SPSR for EL1h with interrupts ENABLED so kthreads can be preempted
        // For first run, enable interrupts; for resumed, use saved SPSR
        if !has_started {
            frame.spsr = 0x5;  // EL1h, DAIF clear (interrupts enabled)
        } else if context.elr_el1 != 0 {
            frame.spsr = context.spsr_el1;  // Restore saved processor state
        } else {
            frame.spsr = 0x5;  // EL1h, DAIF clear (fallback for unsaved context)
        }

        // Store kernel SP for restoration after ERET
        unsafe {
            Aarch64PerCpu::set_user_rsp_scratch(context.sp);
        }

        // CRITICAL: Restore SP_EL0 for userspace threads blocked in syscalls.
        // Without this, SP_EL0 retains whatever the previously-running thread
        // set (e.g., a child process after exec), causing signal delivery to
        // read the wrong user stack pointer and crash.
        if context.sp_el0 != 0 {
            unsafe {
                core::arch::asm!(
                    "msr sp_el0, {}",
                    in(reg) context.sp_el0,
                    options(nomem, nostack)
                );
            }
        }

        // Memory barrier to ensure all writes are visible
        core::sync::atomic::fence(Ordering::SeqCst);

        // DIAGNOSTIC: Catch ELR=0 before ERET — dump full state for debugging
        if frame.elr == 0 {
            raw_uart_str("\n\n!!! FATAL: frame.elr=0 in setup_kernel_thread_return !!!\n");
            raw_uart_str("  thread_id=");
            raw_uart_dec(thread_id);
            raw_uart_str("  name=");
            raw_uart_str(&_name);
            raw_uart_str("\n  has_started=");
            raw_uart_char(if has_started { b'1' } else { b'0' });
            raw_uart_str("  ctx.elr_el1=");
            raw_uart_hex(context.elr_el1);
            raw_uart_str("\n  ctx.x30=");
            raw_uart_hex(context.x30);
            raw_uart_str("  ctx.sp=");
            raw_uart_hex(context.sp);
            raw_uart_str("\n  ctx.x0=");
            raw_uart_hex(context.x0);
            raw_uart_str("  ctx.x19=");
            raw_uart_hex(context.x19);
            raw_uart_str("\n  ctx.spsr_el1=");
            raw_uart_hex(context.spsr_el1);
            raw_uart_str("  ctx.sp_el0=");
            raw_uart_hex(context.sp_el0);
            raw_uart_str("\n  frame.spsr=");
            raw_uart_hex(frame.spsr);
            // Also dump the CPU ID
            let cpu_id = Aarch64PerCpu::cpu_id() as usize;
            raw_uart_str("  cpu=");
            raw_uart_dec(cpu_id as u64);
            raw_uart_str("\n");

            // Safety: redirect to idle loop to prevent ERET to address 0x0
            frame.elr = idle_loop_arm64 as *const () as u64;
            frame.spsr = 0x5;  // EL1h, interrupts enabled
        }
    } else {
        // NOTE: No logging - context switch path must be lock-free
        // This indicates a serious bug but we can't safely log here
        raw_uart_str("KTHREAD_ERR\n");
    }
}

/// Restore userspace context for a thread.
fn restore_userspace_context_arm64(thread_id: u64, frame: &mut Aarch64ExceptionFrame) {
    // NOTE: No logging here - context switch path must be lock-free

    // Check if this thread has ever run
    let has_started = crate::task::scheduler::with_thread_mut(thread_id, |thread| thread.has_started)
        .unwrap_or(false);

    if !has_started {
        // First run for this thread - no logging (use raw_uart_char for debugging)

        // Mark thread as started
        crate::task::scheduler::with_thread_mut(thread_id, |thread| {
            thread.has_started = true;
        });

        setup_first_userspace_entry_arm64(thread_id, frame);
        return;
    }

    // Restore saved context
    crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        // Restore ALL general-purpose registers
        // CRITICAL: For forked children, the caller-saved registers (x1-x18) contain
        // important values from the parent's execution state that must be preserved.
        // Only restoring callee-saved registers (x19-x30) would leave x1-x18 with
        // garbage values from the previous thread's exception frame, causing crashes.
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
    });

    // Set TTBR0 target for this thread's process address space
    set_next_ttbr0_for_thread(thread_id);

    // Switch TTBR0 if needed for different address space
    switch_ttbr0_if_needed(thread_id);

    // CRITICAL: Set user_rsp_scratch to this thread's kernel stack top.
    // The IRQ return path (boot.S) uses `mov sp, [user_rsp_scratch]` before ERET.
    // Without this, SP_EL1 retains the switching-out thread's stack pointer,
    // causing the next IRQ from EL0 to allocate its exception frame on the
    // wrong kernel stack — corrupting memory and other threads' SVC frames.
    unsafe {
        Aarch64PerCpu::set_user_rsp_scratch(Aarch64PerCpu::kernel_stack_top());
    }
}

/// Set up exception frame for first entry to userspace.
fn setup_first_userspace_entry_arm64(thread_id: u64, frame: &mut Aarch64ExceptionFrame) {
    crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        // Set return address to entry point
        frame.elr = thread.context.elr_el1;

        // SPSR for EL0t (userspace, interrupts enabled)
        // M[3:0] = 0b0000, DAIF = 0
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

        // NOTE: No logging - context switch path must be lock-free
        // Use raw_uart_char for debugging if needed
    });

    // Set TTBR0 target for this thread's process address space
    set_next_ttbr0_for_thread(thread_id);

    // Switch TTBR0 for this thread's address space
    switch_ttbr0_if_needed(thread_id);

    // CRITICAL: Set user_rsp_scratch to this thread's kernel stack top.
    // Same as restore_userspace_context_arm64 — the IRQ return path uses
    // user_rsp_scratch for SP after ERET. Without this, SP_EL1 is wrong.
    unsafe {
        Aarch64PerCpu::set_user_rsp_scratch(Aarch64PerCpu::kernel_stack_top());
    }
}

/// Switch TTBR0_EL1 if the thread requires a different address space.
///
/// On ARM64, TTBR0 holds the user page table base and TTBR1 holds the kernel
/// page table base. We only need to switch TTBR0 when switching between
/// processes with different address spaces.
fn switch_ttbr0_if_needed(_thread_id: u64) {
    // TODO: Integrate with process management to get page table base
    // For now, we assume all userspace threads share the same page table

    // Get the next TTBR0 value from per-CPU data
    let next_ttbr0 = Aarch64PerCpu::next_cr3();

    if next_ttbr0 == 0 {
        // No TTBR0 switch needed
        return;
    }

    // Read current TTBR0
    let current_ttbr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) current_ttbr0, options(nomem, nostack));
    }

    if current_ttbr0 != next_ttbr0 {
        // NOTE: No logging - context switch path must be lock-free

        unsafe {
            // CRITICAL: On ARM64, changing TTBR0 does NOT automatically flush the TLB
            // (unlike x86-64's CR3). We MUST flush the TLB after switching TTBR0,
            // otherwise the parent process may use stale TLB entries from the child
            // after a fork/exit cycle, causing CoW memory corruption.
            core::arch::asm!(
                "dsb ishst",           // Ensure previous stores complete
                "msr ttbr0_el1, {}",   // Set new page table
                "isb",                 // Synchronize context
                "tlbi vmalle1is",      // FLUSH ENTIRE TLB - critical for CoW correctness
                "dsb ish",             // Ensure TLB flush completes
                "isb",                 // Synchronize instruction stream
                in(reg) next_ttbr0,
                options(nomem, nostack)
            );
        }

        // Update saved process TTBR0
        unsafe {
            Aarch64PerCpu::set_saved_process_cr3(next_ttbr0);
        }
    }

    // Clear next_cr3 to indicate switch is done
    unsafe {
        Aarch64PerCpu::set_next_cr3(0);
    }
}

/// Determine and set the next TTBR0 value for a userspace thread.
fn set_next_ttbr0_for_thread(thread_id: u64) {
    let next_ttbr0 = {
        let manager_guard = crate::process::manager();
        if let Some(ref manager) = *manager_guard {
            if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                process
                    .page_table
                    .as_ref()
                    .map(|pt| pt.level_4_frame().start_address().as_u64())
            } else {
                None
            }
        } else {
            None
        }
    };

    if let Some(ttbr0) = next_ttbr0 {
        unsafe {
            Aarch64PerCpu::set_next_cr3(ttbr0);
        }
    } else {
        // NOTE: No logging - context switch path must be lock-free
        // This indicates a bug but we can't safely log here
        raw_uart_str("TTBR0_ERR\n");
    }
}

/// ARM64 idle loop - wait for interrupts.
///
/// This function runs when no other threads are ready. It uses WFI
/// (Wait For Interrupt) to put the CPU in a low-power state.
#[no_mangle]
pub extern "C" fn idle_loop_arm64() -> ! {
    loop {
        // Enable interrupts and wait for interrupt
        // On ARM64, WFI will wait until an interrupt occurs
        unsafe {
            core::arch::asm!(
                "msr daifclr, #0xf",  // Enable all interrupts (clear DAIF)
                "wfi",                 // Wait for interrupt
                options(nomem, nostack)
            );
        }
    }
}

/// Perform a context switch between two threads using the low-level
/// assembly switch_context function.
///
/// This is used for explicit context switches (e.g., yield, blocking syscalls)
/// rather than interrupt-driven preemption.
///
/// # Safety
///
/// Both contexts must be valid and properly initialized.
#[allow(dead_code)]
pub unsafe fn perform_context_switch(
    old_context: &mut CpuContext,
    new_context: &CpuContext,
) {
    // Use the assembly context switch from context.rs
    super::context::switch_context(
        old_context as *mut CpuContext,
        new_context as *const CpuContext,
    );
}

/// Switch to a new thread for the first time (doesn't save current context).
///
/// # Safety
///
/// The context must be valid and properly initialized.
#[allow(dead_code)]
pub unsafe fn switch_to_new_thread(context: &CpuContext) -> ! {
    super::context::switch_to_thread(context as *const CpuContext)
}

/// Switch to userspace using ERET.
///
/// # Safety
///
/// The context must have valid userspace addresses.
#[allow(dead_code)]
pub unsafe fn switch_to_user(context: &CpuContext) -> ! {
    super::context::switch_to_user(context as *const CpuContext)
}

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
/// Key differences from x86_64:
/// - User stack pointer is in SP_EL0, not in the exception frame
/// - Uses TTBR0_EL1 instead of CR3 for page table switching
/// - SPSR contains processor state instead of RFLAGS
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
                // On ARM64, this is TTBR0_EL1
                if let Some(ref page_table) = process.page_table {
                    let ttbr0_value = page_table.level_4_frame().start_address().as_u64();
                    unsafe {
                        // CRITICAL: Flush TLB after TTBR0 switch for CoW correctness
                        core::arch::asm!(
                            "dsb ishst",           // Ensure previous stores complete
                            "msr ttbr0_el1, {}",   // Set new page table
                            "isb",                 // Synchronize context
                            "tlbi vmalle1is",      // FLUSH ENTIRE TLB
                            "dsb ish",             // Ensure TLB flush completes
                            "isb",                 // Synchronize instruction stream
                            in(reg) ttbr0_value,
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
                // The signal frame was pushed onto the user stack
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
                        // Signal terminated the process
                        crate::task::scheduler::set_need_resched();
                        signal_termination_info = Some(notification);
                        setup_idle_return_arm64(frame);
                        crate::task::scheduler::switch_to_idle();
                        // Don't return here - fall through to handle notification
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
        // process borrow has ended here

        // Drop manager guard first to avoid deadlock when notifying parent
        drop(manager_guard);

        // Notify parent if signal terminated a child
        if let Some(notification) = signal_termination_info {
            crate::signal::delivery::notify_parent_of_termination_deferred(&notification);
        }
    }
}

/// Create SavedRegisters from an Aarch64ExceptionFrame and SP_EL0
///
/// This is needed because the signal delivery code operates on SavedRegisters,
/// which includes the stack pointer that isn't in the exception frame on ARM64.
///
/// This function is the core of ARM64 signal delivery - it converts the
/// exception frame (used by hardware) to SavedRegisters (used by the signal
/// infrastructure). This conversion is ARM64-specific because:
/// - ARM64 exception frames don't include SP_EL0 (user stack pointer)
/// - The register mapping is completely different from x86_64
/// - SPSR/ELR have ARM64-specific semantics
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
