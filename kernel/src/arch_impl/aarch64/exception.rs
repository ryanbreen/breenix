//! ARM64 exception handlers.
//!
//! These handlers are called from the assembly exception vector table.
//! They process synchronous exceptions (syscalls, page faults, etc.) and IRQs.
//!
//! For syscalls (SVC from EL0), the handler delegates to the dedicated
//! syscall entry module (`syscall_entry.rs`) which provides preemption
//! handling, signal delivery, and context switch support.

#![allow(dead_code)]

use crate::arch_impl::aarch64::constants;
use crate::arch_impl::aarch64::gic;
use crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame;
use crate::arch_impl::aarch64::syscall_entry::rust_syscall_handler_aarch64;
use crate::arch_impl::traits::SyscallFrame;
use crate::arch_impl::traits::PerCpuOps;

/// Set the per-CPU idle/boot stack in `user_rsp_scratch` so the assembly ERET
/// path restores SP to a safe stack when redirecting to idle_loop_arm64.
///
/// CRITICAL: Always use the CPU's boot stack (computed from CPU ID), NOT
/// `Aarch64PerCpu::kernel_stack_top()`. The per-CPU kernel_stack_top holds
/// the LAST DISPATCHED thread's kernel stack — when called from an exception
/// handler after a user process crash, this would be the crashed process's
/// kernel stack, causing the idle thread to run on a stack that may be freed
/// during process cleanup.
#[inline(always)]
fn set_idle_stack_for_eret() {
    use crate::arch_impl::aarch64::percpu::Aarch64PerCpu;

    // Boot stack: HHDM_BASE + 0x4100_0000 + (cpu_id + 1) * 0x20_0000
    let cpu_id = Aarch64PerCpu::cpu_id() as u64;
    let idle_stack = 0xFFFF_0000_0000_0000u64 + 0x4100_0000 + (cpu_id + 1) * 0x20_0000;
    unsafe {
        Aarch64PerCpu::set_user_rsp_scratch(idle_stack);
    }
}

/// Switch TTBR0 to the kernel page table and flush the TLB.
///
/// This ensures we don't return to userspace with a stale/terminated address space.
#[inline(always)]
fn switch_ttbr0_to_kernel() {
    let mut kernel_ttbr0 = crate::per_cpu_aarch64::get_kernel_cr3();
    if kernel_ttbr0 == 0 {
        // Fallback to boot TTBR0 table if per-CPU kernel TTBR0 is unavailable.
        kernel_ttbr0 = 0x4200_0000;
    }

    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "msr ttbr0_el1, {}",
            "isb",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            in(reg) kernel_ttbr0,
            options(nomem, nostack)
        );
    }
}

/// Mark the current thread as Terminated in the scheduler and remove from ready queue.
///
/// Called from exception handlers after `pm.exit_process()` to prevent the
/// scheduler from re-dispatching a thread whose process has been terminated
/// (page tables freed, FDs closed, etc.). Without this, other CPUs can pick
/// up the "still Ready" thread and ERET into a freed address space.
///
/// Must be called BEFORE `switch_to_idle()`, because after that call
/// `current_thread_id()` returns the idle thread ID.
fn terminate_current_scheduler_thread() {
    if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
        crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.get_thread_mut(thread_id) {
                thread.set_terminated();
            }
            sched.remove_from_ready_queue(thread_id);
        });
    }
}

/// ARM64 syscall result type (mirrors x86_64 version)
#[derive(Debug)]
pub enum SyscallResult {
    Ok(u64),
    Err(u64),
}

/// Exception Syndrome Register (ESR_EL1) exception class values
mod exception_class {
    pub const UNKNOWN: u32 = 0b000000;
    pub const SVC_AARCH64: u32 = 0b010101;  // SVC instruction (syscall)
    pub const INSTRUCTION_ABORT_LOWER: u32 = 0b100000;
    pub const INSTRUCTION_ABORT_SAME: u32 = 0b100001;
    pub const DATA_ABORT_LOWER: u32 = 0b100100;
    pub const DATA_ABORT_SAME: u32 = 0b100101;
    pub const SP_ALIGNMENT: u32 = 0b100110;
    pub const FP_EXCEPTION: u32 = 0b101100;
    pub const SERROR: u32 = 0b101111;
    pub const BREAKPOINT_LOWER: u32 = 0b110000;
    pub const BREAKPOINT_SAME: u32 = 0b110001;
    pub const SOFTWARE_STEP_LOWER: u32 = 0b110010;
    pub const SOFTWARE_STEP_SAME: u32 = 0b110011;
    pub const WATCHPOINT_LOWER: u32 = 0b110100;
    pub const WATCHPOINT_SAME: u32 = 0b110101;
    pub const BRK_AARCH64: u32 = 0b111100;  // BRK instruction
}

/// Handle synchronous exceptions (syscalls, page faults, etc.)
///
/// Called from assembly with:
/// - x0 = pointer to Aarch64ExceptionFrame
/// - x1 = ESR_EL1 (Exception Syndrome Register)
/// - x2 = FAR_EL1 (Fault Address Register)
#[no_mangle]
pub extern "C" fn handle_sync_exception(frame: *mut Aarch64ExceptionFrame, esr: u64, far: u64) {
    let ec = ((esr >> 26) & 0x3F) as u32;  // Exception Class
    let iss = (esr & 0x1FFFFFF) as u32;    // Instruction Specific Syndrome

    match ec {
        exception_class::SVC_AARCH64 => {
            // Syscall - ARM64 ABI: X8=syscall number, X0-X5=args, X0=return
            // Delegate to the dedicated syscall entry module which handles:
            // - Preemption counting
            // - EL0_CONFIRMED marker
            // - Signal delivery on return
            // - Context switch checking
            let frame = unsafe { &mut *frame };

            // Check if from EL0 (userspace) - use full handler with preemption/signals
            let from_el0 = (frame.spsr & 0xF) == 0;
            if from_el0 {
                rust_syscall_handler_aarch64(frame);
            } else {
                // From EL1 (kernel) - use simple handler (shouldn't happen normally)
                handle_syscall(frame);
            }
        }

        exception_class::DATA_ABORT_LOWER | exception_class::DATA_ABORT_SAME => {
            // Try to handle as CoW fault first
            if handle_cow_fault_arm64(far, iss) {
                // CoW fault handled successfully, return to userspace
                return;
            }

            // Not a CoW fault or couldn't be handled
            let frame_ref = unsafe { &mut *frame };
            let dfsc = (iss & 0x3F) as u16;

            // Check if from userspace (EL0) - SPSR[3:0] indicates source EL
            let from_el0 = (frame_ref.spsr & 0xF) == 0;
            let ttbr0: u64;
            unsafe {
                core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack));
            }

            // Lock-free trace: data abort event
            crate::tracing::providers::process::trace_data_abort(0, dfsc);

            // Lock-free diagnostic — serial_println! acquires SERIAL lock which
            // can deadlock on SMP if another CPU holds SCHEDULER.
            {
                use crate::arch_impl::aarch64::context_switch::{raw_uart_str, raw_uart_hex, raw_uart_char, raw_uart_dec};
                raw_uart_str("\n[DATA_ABORT] FAR=");
                raw_uart_hex(far);
                raw_uart_str(" ELR=");
                raw_uart_hex(frame_ref.elr);
                raw_uart_str(" ESR=");
                raw_uart_hex(esr);
                raw_uart_str(" DFSC=");
                raw_uart_hex(dfsc as u64);
                raw_uart_str(" TTBR0=");
                raw_uart_hex(ttbr0);
                raw_uart_str(" from_el0=");
                raw_uart_char(if from_el0 { b'1' } else { b'0' });

                // For kernel-mode faults, dump extra diagnostic info to identify
                // the faulting code path (null deref, wild pointer, use-after-free)
                if !from_el0 {
                    let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id();
                    raw_uart_str(" cpu=");
                    raw_uart_dec(cpu_id as u64);
                    raw_uart_str("\n  x19=");
                    raw_uart_hex(frame_ref.x19);
                    raw_uart_str(" x20=");
                    raw_uart_hex(frame_ref.x20);
                    raw_uart_str(" x29=");
                    raw_uart_hex(frame_ref.x29);
                    raw_uart_str(" x30=");
                    raw_uart_hex(frame_ref.x30);
                    // SP at crash = frame address + 272 (exception frame size)
                    raw_uart_str(" sp=");
                    raw_uart_hex(frame as u64 + 272);
                    if let Some(tid) = crate::task::scheduler::current_thread_id() {
                        raw_uart_str(" tid=");
                        raw_uart_dec(tid);
                        crate::task::scheduler::with_thread_mut(tid, |thread| {
                            raw_uart_str(" name=");
                            raw_uart_str(&thread.name);
                        });
                    }

                    // Per-CPU state diagnostic: shows whether kernel_stack_top and
                    // user_rsp_scratch are correct for the current thread.
                    let percpu_kst = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::kernel_stack_top();
                    raw_uart_str("\n  percpu_kst=");
                    raw_uart_hex(percpu_kst);
                    let user_rsp: u64;
                    unsafe {
                        let percpu_base: u64;
                        core::arch::asm!("mrs {}, tpidr_el1", out(reg) percpu_base, options(nomem, nostack));
                        user_rsp = if percpu_base != 0 {
                            core::ptr::read_volatile((percpu_base + 40) as *const u64)
                        } else {
                            0
                        };
                    }
                    raw_uart_str(" user_rsp_scratch=");
                    raw_uart_hex(user_rsp);

                    // Check thread's expected kernel_stack_top from scheduler
                    if let Some(tid) = crate::task::scheduler::current_thread_id() {
                        let thread_kst = crate::task::scheduler::with_thread_mut(tid, |thread| {
                            thread.kernel_stack_top.map(|v| v.as_u64()).unwrap_or(0)
                        });
                        if let Some(kst) = thread_kst {
                            raw_uart_str(" thread_kst=");
                            raw_uart_hex(kst);
                        }
                    }

                    // Classify which stack region the frame is on
                    let frame_addr = frame as u64;
                    const HHDM_BASE_DIAG: u64 = 0xFFFF_0000_0000_0000;
                    const BOOT_STACK_BASE: u64 = HHDM_BASE_DIAG + 0x4100_0000;
                    const BOOT_STACK_END: u64 = HHDM_BASE_DIAG + 0x4200_0000;
                    const KSTACK_BASE: u64 = HHDM_BASE_DIAG + 0x5200_0000;
                    const KSTACK_END: u64 = HHDM_BASE_DIAG + 0x5400_0000;
                    if frame_addr >= BOOT_STACK_BASE && frame_addr < BOOT_STACK_END {
                        raw_uart_str("\n  STACK=boot_cpu");
                        let offset_from_base = frame_addr - BOOT_STACK_BASE;
                        let boot_cpu = offset_from_base / 0x20_0000;
                        raw_uart_dec(boot_cpu);
                    } else if frame_addr >= KSTACK_BASE && frame_addr < KSTACK_END {
                        raw_uart_str("\n  STACK=alloc_kstack");
                    } else {
                        raw_uart_str("\n  STACK=unknown");
                    }
                }
                raw_uart_str("\n");
            }

            if from_el0 {
                // From userspace - terminate the process with SIGSEGV
                // Get current TTBR0 to find the process
                let page_table_phys = ttbr0 & !0xFFFF_0000_0000_0FFF;

                // Find and terminate the process
                let mut terminated = false;
                let mut already_terminated = false;
                crate::process::with_process_manager(|pm| {
                    if let Some((pid, _process)) = pm.find_process_by_cr3_mut(page_table_phys) {
                        if _process.is_terminated() {
                            already_terminated = true;
                            return;
                        }
                        crate::tracing::providers::process::trace_process_exit(pid.as_u64() as u16, (-11i16) as u16);
                        pm.exit_process(pid, -11); // SIGSEGV exit code
                        terminated = true;
                    } else {
                        // trace_data_abort already captured the fault
                    }
                });

                if terminated || already_terminated {
                    // CRITICAL: Mark the scheduler's thread as Terminated BEFORE
                    // switch_to_idle(). Without this, the scheduler still thinks
                    // the thread is Ready/Running and will re-dispatch it on
                    // another CPU, causing ERET to a freed address space.
                    terminate_current_scheduler_thread();
                    switch_ttbr0_to_kernel();
                    crate::task::scheduler::set_need_resched();

                    // CRITICAL: Set frame values BEFORE switch_to_idle() —
                    // if switch_to_idle hits a nested exception, the frame
                    // must already have safe ELR/SPSR for the assembly ERET path.
                    frame_ref.elr = crate::arch_impl::aarch64::idle_loop_arm64 as *const () as u64;
                    frame_ref.spsr = 0x5; // EL1h, DAIF clear (interrupts enabled)

                    set_idle_stack_for_eret();
                    crate::task::scheduler::switch_to_idle();
                    return;
                }
            }

            // From kernel or couldn't terminate — try to terminate the user
            // process (best effort, using try_lock to avoid deadlock) then
            // redirect to idle loop.
            {
                use crate::arch_impl::aarch64::context_switch::raw_uart_str;
                raw_uart_str("[DATA_ABORT] kernel-mode fault, attempting process cleanup\n");
            }

            // Best-effort process termination: use try_manager() to avoid
            // deadlock if another CPU holds PROCESS_MANAGER.
            let page_table_phys = ttbr0 & !0xFFFF_0000_0000_0FFF;
            if let Some(mut guard) = crate::process::try_manager() {
                if let Some(pm) = guard.as_mut() {
                    if let Some((pid, process)) = pm.find_process_by_cr3_mut(page_table_phys) {
                        if !process.is_terminated() {
                            crate::tracing::providers::process::trace_process_exit(
                                pid.as_u64() as u16, (-11i16) as u16,
                            );
                            pm.exit_process(pid, -11); // SIGSEGV
                        }
                    }
                }
                drop(guard);
            }

            // Mark scheduler thread as terminated (best effort)
            terminate_current_scheduler_thread();
            switch_ttbr0_to_kernel();

            // CRITICAL: Set frame values BEFORE switch_to_idle_best_effort() —
            // if switch_to_idle panics or hits a nested exception, the frame
            // must already have safe ELR/SPSR for the assembly ERET path.
            frame_ref.elr =
                crate::arch_impl::aarch64::idle_loop_arm64 as *const () as u64;
            frame_ref.spsr = 0x5; // EL1h, interrupts enabled

            set_idle_stack_for_eret();
            // Use best_effort (try_lock) to avoid deadlock if SCHEDULER is held.
            crate::task::scheduler::switch_to_idle_best_effort();
        }

        exception_class::INSTRUCTION_ABORT_LOWER | exception_class::INSTRUCTION_ABORT_SAME => {
            let frame_ref = unsafe { &mut *frame };
            let ifsc = (iss & 0x3F) as u16;
            let from_el0 = (frame_ref.spsr & 0xF) == 0;

            let ttbr0: u64;
            unsafe {
                core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack));
            }

            // Use raw UART for ALL output — serial_println! acquires a spin lock
            // that may already be held by this or another CPU, causing deadlock.
            {
                use crate::arch_impl::aarch64::context_switch::{raw_uart_char, raw_uart_str, raw_uart_hex, raw_uart_dec};
                raw_uart_str("\n[INSTRUCTION_ABORT] FAR=");
                raw_uart_hex(far);
                raw_uart_str(" ELR=");
                raw_uart_hex(frame_ref.elr);
                raw_uart_str(" ESR=");
                raw_uart_hex(esr);
                raw_uart_str(" IFSC=");
                raw_uart_hex(ifsc as u64);
                raw_uart_str(" TTBR0=");
                raw_uart_hex(ttbr0);
                raw_uart_str(" from_el0=");
                raw_uart_char(if from_el0 { b'1' } else { b'0' });

                if !from_el0 && frame_ref.elr < 0x1000 {
                    let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id();
                    raw_uart_str("\n[DIAG] ELR=");
                    raw_uart_hex(frame_ref.elr);
                    raw_uart_str(" from EL1 cpu=");
                    raw_uart_dec(cpu_id as u64);
                    // Identify current thread
                    // CRITICAL: Print name directly inside closure to avoid
                    // heap allocation (String::clone) in exception context.
                    if let Some(tid) = crate::task::scheduler::current_thread_id() {
                        raw_uart_str(" tid=");
                        raw_uart_dec(tid);
                        crate::task::scheduler::with_thread_mut(tid, |thread| {
                            raw_uart_str(" name=");
                            raw_uart_str(&thread.name);
                        });
                    }
                    raw_uart_str("\n  x0=");
                    raw_uart_hex(frame_ref.x0);
                    raw_uart_str(" x19=");
                    raw_uart_hex(frame_ref.x19);
                    raw_uart_str(" x29=");
                    raw_uart_hex(frame_ref.x29);
                    raw_uart_str(" x30=");
                    raw_uart_hex(frame_ref.x30);
                    raw_uart_str("\n  spsr=");
                    raw_uart_hex(frame_ref.spsr);
                    raw_uart_str(" sp_at_frame=");
                    raw_uart_hex(frame_ref as *const _ as u64);

                    // Per-CPU state diagnostic: shows whether kernel_stack_top and
                    // user_rsp_scratch are correct for the current thread.
                    let percpu_kst = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::kernel_stack_top();
                    raw_uart_str("\n  percpu_kst=");
                    raw_uart_hex(percpu_kst);
                    // Read user_rsp_scratch from per-CPU data
                    let user_rsp: u64;
                    unsafe {
                        let percpu_base: u64;
                        core::arch::asm!("mrs {}, tpidr_el1", out(reg) percpu_base, options(nomem, nostack));
                        user_rsp = if percpu_base != 0 {
                            core::ptr::read_volatile((percpu_base + 40) as *const u64)
                        } else {
                            0
                        };
                    }
                    raw_uart_str(" user_rsp_scratch=");
                    raw_uart_hex(user_rsp);

                    // Check thread's expected kernel_stack_top from scheduler
                    if let Some(tid) = crate::task::scheduler::current_thread_id() {
                        let thread_kst = crate::task::scheduler::with_thread_mut(tid, |thread| {
                            thread.kernel_stack_top.map(|v| v.as_u64()).unwrap_or(0)
                        });
                        if let Some(kst) = thread_kst {
                            raw_uart_str(" thread_kst=");
                            raw_uart_hex(kst);
                        }
                    }

                    // Classify which stack region the frame is on
                    let frame_addr = frame_ref as *const _ as u64;
                    const HHDM_BASE_DIAG: u64 = 0xFFFF_0000_0000_0000;
                    const BOOT_STACK_BASE: u64 = HHDM_BASE_DIAG + 0x4100_0000;
                    const BOOT_STACK_END: u64 = HHDM_BASE_DIAG + 0x4200_0000;
                    const KSTACK_BASE: u64 = HHDM_BASE_DIAG + 0x5200_0000;
                    const KSTACK_END: u64 = HHDM_BASE_DIAG + 0x5400_0000;
                    if frame_addr >= BOOT_STACK_BASE && frame_addr < BOOT_STACK_END {
                        raw_uart_str("\n  STACK=boot_cpu");
                        // Calculate which CPU's boot stack
                        let offset_from_base = frame_addr - BOOT_STACK_BASE;
                        let boot_cpu = offset_from_base / 0x20_0000;
                        raw_uart_dec(boot_cpu);
                    } else if frame_addr >= KSTACK_BASE && frame_addr < KSTACK_END {
                        raw_uart_str("\n  STACK=alloc_kstack");
                    } else {
                        raw_uart_str("\n  STACK=unknown");
                    }

                    // OUTER FRAME: Read the frame 272 bytes above (if on a valid stack)
                    let outer_frame_addr = frame_addr + 272;
                    if outer_frame_addr + 272 <= BOOT_STACK_END
                        || (outer_frame_addr >= KSTACK_BASE && outer_frame_addr + 272 <= KSTACK_END)
                    {
                        let outer = outer_frame_addr as *const u64;
                        unsafe {
                            let outer_x30 = core::ptr::read_volatile(outer.add(30));
                            let outer_elr = core::ptr::read_volatile(outer.add(31));
                            let outer_spsr = core::ptr::read_volatile(outer.add(32));
                            raw_uart_str("\n  OUTER_FRAME(stale?): elr=");
                            raw_uart_hex(outer_elr);
                            raw_uart_str(" x30=");
                            raw_uart_hex(outer_x30);
                            raw_uart_str(" spsr=");
                            raw_uart_hex(outer_spsr);
                        }
                    }
                    raw_uart_str("\n");
                }
            }

            if from_el0 {
                // From userspace - terminate the process with SIGSEGV
                let page_table_phys = ttbr0 & !0xFFFF_0000_0000_0FFF;

                let mut terminated = false;
                let mut already_terminated = false;
                let mut killed_pid: u64 = 0;
                crate::process::with_process_manager(|pm| {
                    if let Some((pid, _process)) = pm.find_process_by_cr3_mut(page_table_phys) {
                        if _process.is_terminated() {
                            already_terminated = true;
                            return;
                        }
                        killed_pid = pid.as_u64();
                        crate::tracing::providers::process::trace_process_exit(
                            pid.as_u64() as u16,
                            (-11i16) as u16,
                        );
                        pm.exit_process(pid, -11); // SIGSEGV
                        terminated = true;
                    }
                });
                // Lock-free diagnostic AFTER releasing process manager lock
                if terminated {
                    use crate::arch_impl::aarch64::context_switch::{raw_uart_str, raw_uart_dec};
                    raw_uart_str("[INSTRUCTION_ABORT] Terminating PID ");
                    raw_uart_dec(killed_pid);
                    raw_uart_str(" (SIGSEGV)\n");
                }

                if terminated || already_terminated {
                    terminate_current_scheduler_thread();
                    switch_ttbr0_to_kernel();
                    crate::task::scheduler::set_need_resched();
                    // CRITICAL: Set frame values BEFORE switch_to_idle()
                    frame_ref.elr =
                        crate::arch_impl::aarch64::idle_loop_arm64 as *const () as u64;
                    frame_ref.spsr = 0x5; // EL1h, DAIF clear (interrupts enabled)

                    set_idle_stack_for_eret();
                    crate::task::scheduler::switch_to_idle();
                    return;
                }
            }

            // From kernel or couldn't terminate — redirect to idle loop.
            // CRITICAL: Set frame values BEFORE switch_to_idle_best_effort() —
            // if switch_to_idle panics or hits a nested exception, the frame
            // must already have safe ELR/SPSR for the assembly ERET path.
            {
                use crate::arch_impl::aarch64::context_switch::raw_uart_str;
                raw_uart_str("[INSTRUCTION_ABORT] redirecting to idle\n");
            }
            frame_ref.elr =
                crate::arch_impl::aarch64::idle_loop_arm64 as *const () as u64;
            frame_ref.spsr = 0x5; // EL1h, DAIF clear (interrupts enabled)

            set_idle_stack_for_eret();
            // Use best_effort (try_lock) to avoid deadlock if SCHEDULER is held.
            crate::task::scheduler::switch_to_idle_best_effort();
        }

        exception_class::BRK_AARCH64 => {
            let frame = unsafe { &mut *frame };
            let imm = iss & 0xFFFF;
            crate::serial_println!("[exception] Breakpoint (BRK #{}) at {:#x}", imm, frame.elr);
            // Skip the BRK instruction
            frame.elr += 4;
        }

        exception_class::SP_ALIGNMENT => {
            // SP alignment fault — redirect to idle to avoid hang.
            let frame_ref = unsafe { &mut *frame };
            let from_el0 = (frame_ref.spsr & 0xF) == 0;
            {
                use crate::arch_impl::aarch64::context_switch::{raw_uart_str, raw_uart_hex, raw_uart_char};
                raw_uart_str("\n[SP_ALIGN] ELR=");
                raw_uart_hex(frame_ref.elr);
                raw_uart_str(" FAR=");
                raw_uart_hex(far);
                raw_uart_str(" from_el0=");
                raw_uart_char(if from_el0 { b'1' } else { b'0' });
                raw_uart_str("\n");
            }
            if from_el0 {
                let ttbr0: u64;
                unsafe { core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack)); }
                let page_table_phys = ttbr0 & !0xFFFF_0000_0000_0FFF;
                crate::process::with_process_manager(|pm| {
                    if let Some((pid, process)) = pm.find_process_by_cr3_mut(page_table_phys) {
                        if !process.is_terminated() {
                            pm.exit_process(pid, -11);
                        }
                    }
                });
                terminate_current_scheduler_thread();
                switch_ttbr0_to_kernel();
            }
            // CRITICAL: Set frame values BEFORE switch_to_idle_best_effort()
            frame_ref.elr = crate::arch_impl::aarch64::idle_loop_arm64 as *const () as u64;
            frame_ref.spsr = 0x5;

            set_idle_stack_for_eret();
            crate::task::scheduler::switch_to_idle_best_effort();
        }

        // PC alignment fault (EC=0x22) — CPU tried to execute at a misaligned address.
        // This happens when a thread's ELR is corrupted to a non-4-byte-aligned value
        // (e.g., 0x3). Redirect to idle instead of hanging.
        0x22 => {
            let frame_ref = unsafe { &mut *frame };
            let from_el0 = (frame_ref.spsr & 0xF) == 0;
            {
                use crate::arch_impl::aarch64::context_switch::{raw_uart_str, raw_uart_hex, raw_uart_char};
                raw_uart_str("\n[PC_ALIGN] ELR=");
                raw_uart_hex(frame_ref.elr);
                raw_uart_str(" FAR=");
                raw_uart_hex(far);
                raw_uart_str(" from_el0=");
                raw_uart_char(if from_el0 { b'1' } else { b'0' });
                raw_uart_str("\n");
            }
            if from_el0 {
                let ttbr0: u64;
                unsafe { core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack)); }
                let page_table_phys = ttbr0 & !0xFFFF_0000_0000_0FFF;
                crate::process::with_process_manager(|pm| {
                    if let Some((pid, process)) = pm.find_process_by_cr3_mut(page_table_phys) {
                        if !process.is_terminated() {
                            pm.exit_process(pid, -11);
                        }
                    }
                });
                terminate_current_scheduler_thread();
                switch_ttbr0_to_kernel();
            }
            // CRITICAL: Set frame values BEFORE switch_to_idle_best_effort()
            frame_ref.elr = crate::arch_impl::aarch64::idle_loop_arm64 as *const () as u64;
            frame_ref.spsr = 0x5;

            set_idle_stack_for_eret();
            crate::task::scheduler::switch_to_idle_best_effort();
        }

        _ => {
            // Mask all interrupts to prevent cascading exceptions on SMP
            // (timer IRQs cause context switches that schedule more threads
            // onto this CPU, each hitting the same unhandled exception)
            unsafe { core::arch::asm!("msr daifset, #0xf", options(nomem, nostack)); }
            let frame_ref = unsafe { &mut *frame };
            crate::serial_println!("[exception] Unhandled sync exception");
            crate::serial_println!("  EC: {:#x}, ISS: {:#x}", ec, iss);
            crate::serial_println!("  ELR: {:#x}, FAR: {:#x}", frame_ref.elr, far);
            crate::serial_println!("  SPSR: {:#x}", frame_ref.spsr);
            // Redirect to idle instead of hanging — allows system to recover.
            // CRITICAL: Set frame values BEFORE switch_to_idle_best_effort()
            frame_ref.elr = crate::arch_impl::aarch64::idle_loop_arm64 as *const () as u64;
            frame_ref.spsr = 0x5;

            set_idle_stack_for_eret();
            crate::task::scheduler::switch_to_idle_best_effort();
        }
    }
}

/// Syscall numbers (Linux/Breenix ABI compatible)
mod syscall_nums {
    // Core syscalls
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const YIELD: u64 = 3;        // Breenix: sched_yield
    pub const GET_TIME: u64 = 4;     // Breenix: get_time (deprecated)
    pub const CLOSE: u64 = 6;        // Breenix: close
    pub const BRK: u64 = 12;         // Linux: brk

    // Process syscalls
    pub const GETPID: u64 = 39;
    pub const GETTID: u64 = 186;
    pub const CLOCK_GETTIME: u64 = 228;
}

/// Handle a syscall from userspace (or kernel for testing)
///
/// Uses the SyscallFrame trait to extract arguments in an arch-agnostic way.
/// ARM64-native implementation handles syscalls directly.
fn handle_syscall(frame: &mut Aarch64ExceptionFrame) {
    let syscall_num = frame.syscall_number();
    let arg1 = frame.arg1();
    let arg2 = frame.arg2();
    let arg3 = frame.arg3();

    let result = match syscall_num {
        syscall_nums::EXIT => {
            let exit_code = arg1 as i32;
            crate::serial_println!("[syscall] exit({})", exit_code);
            crate::serial_println!();
            crate::serial_println!("========================================");
            crate::serial_println!("  Userspace Test Complete!");
            crate::serial_println!("  Exit code: {}", exit_code);
            crate::serial_println!("========================================");
            crate::serial_println!();

            // For now, just halt - real implementation would terminate the process
            loop {
                unsafe { core::arch::asm!("wfi"); }
            }
        }

        syscall_nums::WRITE => {
            sys_write(arg1, arg2, arg3)
        }

        syscall_nums::READ => {
            // For now, read is not implemented
            SyscallResult::Err(38) // ENOSYS
        }

        syscall_nums::YIELD => {
            // Yield does nothing for single-process kernel
            SyscallResult::Ok(0)
        }

        syscall_nums::GET_TIME => {
            // Legacy get_time syscall - return milliseconds
            let ms = crate::time::get_monotonic_time();
            SyscallResult::Ok(ms)
        }

        syscall_nums::CLOSE => {
            // Close syscall - no file descriptors yet, just succeed
            SyscallResult::Ok(0)
        }

        syscall_nums::BRK => {
            // brk syscall - memory management
            // For now, return success with same address (no-op)
            SyscallResult::Ok(arg1)
        }

        syscall_nums::GETPID => {
            // Return a fixed PID for now (1 = init)
            SyscallResult::Ok(1)
        }

        syscall_nums::GETTID => {
            // Return a fixed TID for now (1 = main thread)
            SyscallResult::Ok(1)
        }

        syscall_nums::CLOCK_GETTIME => {
            sys_clock_gettime(arg1 as u32, arg2 as *mut Timespec)
        }

        _ => {
            crate::serial_println!("[syscall] ENOSYS for syscall {}", syscall_num);
            SyscallResult::Err(38) // ENOSYS
        }
    };

    // Convert SyscallResult to i64 return value
    let return_value: i64 = match result {
        SyscallResult::Ok(val) => val as i64,
        SyscallResult::Err(errno) => -(errno as i64),
    };

    // Set return value (negative values indicate errors in Linux convention)
    frame.set_return_value(return_value as u64);
}

/// Timespec structure for clock_gettime
#[repr(C)]
#[derive(Copy, Clone)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// ARM64 sys_write implementation
fn sys_write(fd: u64, buf: u64, count: u64) -> SyscallResult {
    // Only support stdout (1) and stderr (2) for now
    if fd != 1 && fd != 2 {
        return SyscallResult::Err(9); // EBADF
    }

    // Validate buffer pointer (basic check)
    if buf == 0 {
        return SyscallResult::Err(14); // EFAULT
    }

    // Write each byte to serial
    for i in 0..count {
        let byte = unsafe { *((buf + i) as *const u8) };
        crate::serial_print!("{}", byte as char);
    }

    SyscallResult::Ok(count)
}

/// ARM64 sys_clock_gettime implementation
fn sys_clock_gettime(clock_id: u32, user_timespec_ptr: *mut Timespec) -> SyscallResult {
    // Validate pointer
    if user_timespec_ptr.is_null() {
        return SyscallResult::Err(14); // EFAULT
    }

    // Get time from the arch-agnostic time module
    let (tv_sec, tv_nsec) = match clock_id {
        0 => { // CLOCK_REALTIME
            crate::time::get_real_time_ns()
        }
        1 => { // CLOCK_MONOTONIC
            let (secs, nanos) = crate::time::get_monotonic_time_ns();
            (secs as i64, nanos as i64)
        }
        _ => {
            return SyscallResult::Err(22); // EINVAL
        }
    };

    // Write to userspace
    unsafe {
        (*user_timespec_ptr).tv_sec = tv_sec;
        (*user_timespec_ptr).tv_nsec = tv_nsec;
    }

    SyscallResult::Ok(0)
}

/// PL011 UART IRQ number (SPI 1, which is IRQ 33)
const UART0_IRQ: u32 = 33;

/// Raw serial write - write a string without locks, for use in interrupt handlers
#[inline(always)]
fn raw_serial_str(s: &[u8]) {
    crate::serial_aarch64::raw_serial_str(s);
}

/// Handle IRQ interrupts
///
/// Called from assembly after saving registers.
/// This is the main IRQ dispatch point for ARM64.
#[no_mangle]
pub extern "C" fn handle_irq() {
    crate::tracing::providers::counters::count_irq();

    // Acknowledge the interrupt from GIC
    if let Some(irq_id) = gic::acknowledge_irq() {
        // Handle the interrupt based on ID
        match irq_id {
            // Virtual timer interrupt (PPI 27)
            // This is the scheduling timer - calls into scheduler
            crate::arch_impl::aarch64::timer_interrupt::TIMER_IRQ => {
                // Call the timer interrupt handler which handles:
                // - Re-arming the timer
                // - Updating global time
                // - Decrementing time quantum
                // - Setting need_resched flag
                crate::arch_impl::aarch64::timer_interrupt::timer_interrupt_handler();
            }

            // UART0 receive interrupt (SPI 1 = IRQ 33)
            UART0_IRQ => {
                handle_uart_interrupt();
            }

            // SGIs (0-15) - Inter-processor interrupts
            0..=15 => {
                if irq_id == constants::SGI_RESCHEDULE {
                    // IPI reschedule: another CPU unblocked a thread and wants us to pick it up
                    crate::per_cpu_aarch64::set_need_resched(true);
                }
            }

            // PPIs (16-31) - Private peripheral interrupts (excluding timer)
            16..=31 => {}

            // SPIs (32-1019) - Shared peripheral interrupts
            // Note: No logging here - interrupt handlers must be < 1000 cycles
            32..=1019 => {
                // VirtIO input (keyboard) interrupt dispatch
                if let Some(input_irq) = crate::drivers::virtio::input_mmio::get_irq() {
                    if irq_id == input_irq {
                        crate::drivers::virtio::input_mmio::handle_interrupt();
                    }
                }
                // VirtIO tablet (pointer) interrupt dispatch
                if let Some(tablet_irq) = crate::drivers::virtio::input_mmio::get_tablet_irq() {
                    if irq_id == tablet_irq {
                        crate::drivers::virtio::input_mmio::handle_tablet_interrupt();
                    }
                }
                // VirtIO network interrupt dispatch
                if let Some(net_irq) = crate::drivers::virtio::net_mmio::get_irq() {
                    if irq_id == net_irq {
                        crate::drivers::virtio::net_mmio::handle_interrupt();
                    }
                }
            }

            // Should not happen - GIC filters invalid IDs (1020+)
            _ => {}
        }

        // Signal end of interrupt
        gic::end_of_interrupt(irq_id);

        // Process pending softirqs (deferred work from interrupt handlers)
        // This must happen after EOI but before rescheduling, while still
        // in the IRQ exit path. Network RX processing runs here.
        crate::task::softirqd::do_softirq();

        // Check if we need to reschedule after handling the interrupt
        // This is the ARM64 equivalent of x86's check_need_resched_and_switch
        check_need_resched_on_irq_exit();
    }
}

/// Check if rescheduling is needed and perform context switch if necessary
///
/// This is called at the end of IRQ handling, before returning via ERET.
/// It checks the need_resched flag and performs a context switch if needed.
///
/// Note: This is a simplified version that only handles the scheduling decision.
/// The actual context switch happens when the exception handler returns and
/// the assembly code uses the modified exception frame.
fn check_need_resched_on_irq_exit() {
    // Check if per-CPU data is initialized
    if !crate::per_cpu_aarch64::is_initialized() {
        return;
    }

    // Check if we're still in interrupt context (nested IRQs)
    // Note: Timer interrupt already decremented HARDIRQ count before we get here
    if crate::per_cpu_aarch64::in_interrupt() {
        return;
    }

    // Check if rescheduling is needed (don't clear yet - context_switch does that)
    if !crate::task::scheduler::is_need_resched() {
        return;
    }

    // The actual context switch will be performed by check_need_resched_and_switch_arm64
    // which is called from the exception return path with access to the exception frame.
    // Here we just signal that a reschedule is pending.
    //
    // The flow is:
    // 1. Timer IRQ fires -> timer_interrupt_handler() sets need_resched
    // 2. IRQ handler returns
    // 3. Assembly exception return path calls check_need_resched_and_switch_arm64
    // 4. Context switch happens if needed
    // 5. ERET returns to new thread
}

/// Handle UART receive interrupt
///
/// Read all available bytes from the UART and push to stdin buffer.
/// Echo is handled by the consumer (kernel shell or userspace tty driver).
fn handle_uart_interrupt() {
    use crate::serial_aarch64;

    // Read all available bytes from the UART FIFO
    while let Some(byte) = serial_aarch64::get_received_byte() {
        // Push to stdin buffer for kernel shell or userspace read() syscall
        // This wakes any blocked readers waiting for input
        crate::ipc::stdin::push_byte_from_irq(byte);
    }

    // Clear the interrupt
    serial_aarch64::clear_rx_interrupt();
}

/// Get exception class name for debugging
#[allow(dead_code)]
fn exception_class_name(ec: u32) -> &'static str {
    match ec {
        exception_class::UNKNOWN => "Unknown",
        exception_class::SVC_AARCH64 => "SVC (syscall)",
        exception_class::INSTRUCTION_ABORT_LOWER => "Instruction abort (lower EL)",
        exception_class::INSTRUCTION_ABORT_SAME => "Instruction abort (same EL)",
        exception_class::DATA_ABORT_LOWER => "Data abort (lower EL)",
        exception_class::DATA_ABORT_SAME => "Data abort (same EL)",
        exception_class::SP_ALIGNMENT => "SP alignment fault",
        exception_class::BRK_AARCH64 => "BRK (breakpoint)",
        _ => "Other",
    }
}

/// Handle CoW (Copy-on-Write) page fault for ARM64
///
/// Returns true if the fault was handled (page was copied or made writable)
/// Returns false if this wasn't a CoW fault or couldn't be handled
fn handle_cow_fault_arm64(far: u64, iss: u32) -> bool {
    use crate::memory::arch_stub::{VirtAddr, Page, Size4KiB};
    use crate::memory::cow_stats;
    use crate::memory::frame_allocator::allocate_frame;
    use crate::memory::frame_metadata::{frame_decref, frame_is_shared, frame_register};
    use crate::memory::process_memory::{is_cow_page, make_private_flags};

    // Check if this is a CoW fault:
    // - WnR bit (bit 6) = 1 (caused by write)
    // - DFSC (bits 5:0) = 0x0D/0x0E/0x0F (Permission fault at level 1/2/3)
    let is_write = (iss >> 6) & 1 == 1;
    let dfsc = iss & 0x3F;
    let is_permission_fault = dfsc == 0x0D || dfsc == 0x0E || dfsc == 0x0F;

    if !is_write || !is_permission_fault {
        return false;
    }

    // Track CoW fault count
    cow_stats::TOTAL_FAULTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    let faulting_addr = VirtAddr::new(far);

    // Get current TTBR0 (user page table base)
    let ttbr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack));
    }

    // Mask off ASID to get physical address
    let page_table_phys = ttbr0 & !0xFFFF_0000_0000_0FFF;

    // Lock-free trace: CoW fault entry (pid unknown yet, page index from far)
    crate::tracing::providers::process::trace_cow_fault(0, (far >> 12) as u16);

    // Acquire process manager lock (blocking, with interrupts disabled on ARM64).
    // This is safe because CoW faults from EL0 guarantee the current CPU doesn't
    // hold PROCESS_MANAGER — we were in userspace when the fault occurred.
    // Using try_manager() (non-blocking) would fail under SMP contention when
    // another CPU holds the lock during fork/exec, killing the faulting process.
    let mut guard = crate::process::manager();
    cow_stats::MANAGER_PATH.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let pm = match guard.as_mut() {
        Some(pm) => pm,
        None => return false,
    };

    // Find process by page table
    let (_pid, process) = match pm.find_process_by_cr3_mut(page_table_phys) {
        Some(p) => p,
        None => {
            return false;
        }
    };

    let page_table = match &mut process.page_table {
        Some(pt) => pt,
        None => return false,
    };

    let page: Page<Size4KiB> = Page::containing_address(faulting_addr);

    // Get current page info
    let (old_frame, old_flags) = match page_table.get_page_info(page) {
        Some(info) => info,
        None => {
            return false;
        }
    };

    // Check if this is a CoW page
    if !is_cow_page(old_flags) {
        return false;
    }

    // Lock-free trace: CoW handling with known PID
    crate::tracing::providers::process::trace_cow_fault(_pid.as_u64() as u16, (far >> 12) as u16);

    // If we're the sole owner, just make it writable
    if !frame_is_shared(old_frame) {
        let new_flags = make_private_flags(old_flags);
        if page_table.update_page_flags(page, new_flags).is_err() {
            return false;
        }
        // Flush TLB
        unsafe {
            let va_for_tlbi = faulting_addr.as_u64() >> 12;
            core::arch::asm!(
                "dsb ishst",
                "tlbi vale1is, {0}",
                "dsb ish",
                "isb",
                in(reg) va_for_tlbi,
                options(nostack)
            );
        }
        cow_stats::SOLE_OWNER_OPT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        crate::tracing::providers::process::trace_cow_copy(_pid.as_u64() as u16, (far >> 12) as u16);
        return true;
    }

    // Need to copy the page
    let new_frame = match allocate_frame() {
        Some(f) => f,
        None => {
            return false;
        }
    };

    // Register the new frame so it's tracked for cleanup on process exit
    frame_register(new_frame);

    // Copy page contents via HHDM
    let hhdm_base = crate::arch_impl::aarch64::constants::HHDM_BASE;
    let src = (hhdm_base + old_frame.start_address().as_u64()) as *const u8;
    let dst = (hhdm_base + new_frame.start_address().as_u64()) as *mut u8;

    unsafe {
        core::ptr::copy_nonoverlapping(src, dst, 4096);
    }

    // Unmap old page and map new one with write permissions
    let new_flags = make_private_flags(old_flags);
    if page_table.unmap_page(page).is_err() {
        return false;
    }
    if page_table.map_page(page, new_frame, new_flags).is_err() {
        return false;
    }

    // Decrement reference count on old frame
    frame_decref(old_frame);

    // Flush TLB
    unsafe {
        let va_for_tlbi = faulting_addr.as_u64() >> 12;
        core::arch::asm!(
            "dsb ishst",
            "tlbi vale1is, {0}",
            "dsb ish",
            "isb",
            in(reg) va_for_tlbi,
            options(nostack)
        );
    }

    cow_stats::PAGES_COPIED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    crate::tracing::providers::process::trace_cow_copy(_pid.as_u64() as u16, (far >> 12) as u16);

    true
}
