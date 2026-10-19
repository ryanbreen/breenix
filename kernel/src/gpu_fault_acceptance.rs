//! ARM64 fault-routing acceptance, compiled only in its dedicated boot profile.
//! The userspace driver faults through the existing CoW-stats syscall, reaps
//! SIGSEGV, then asks this module to fault a real kernel thread after handoff.

use crate::syscall::SyscallResult;
use crate::task::thread::ThreadPrivilege;

// Private to this build profile and userspace/programs/src/gpu_fault_acceptance.rs.
pub const SYSCALL: u64 = 0x7fff_0000;

pub fn dispatch(op: u64) -> SyscallResult {
    if !crate::gpu::console::active() || !crate::gpu::panic::test_ready() {
        return SyscallResult::Err(crate::syscall::errno::EAGAIN as u64);
    }
    match op {
        0 => SyscallResult::Ok(0),
        1 => match crate::task::kthread_run(fault_kernel_thread, "gpu-fault-test") {
            Ok(_) => SyscallResult::Ok(0),
            Err(_) => SyscallResult::Err(crate::syscall::errno::ENOMEM as u64),
        },
        _ => SyscallResult::Err(crate::syscall::errno::EINVAL as u64),
    }
}

fn fault_kernel_thread() {
    let kernel_thread = crate::arch_without_interrupts(|| {
        let ptr =
            crate::per_cpu_aarch64::current_thread_ptr() as *const crate::task::thread::Thread;
        // IRQ masking keeps this CPU's current thread alive for this read.
        !ptr.is_null() && unsafe { (*ptr).privilege == ThreadPrivilege::Kernel }
    });
    if !kernel_thread || !crate::gpu::console::active() {
        crate::serial_line::Line::unattributed().text("[GPU_FAULT_TEST invalid kernel context]\n");
        return;
    }
    crate::serial_line::Line::unattributed()
        .text("[GPU_FAULT_TEST kernel privilege=kernel handoff=1]\n");
    // A real EL1 translation fault, with a different FAR from the user's syscall.
    // This is outside uaccess fixups and leaves exception routing unmodified.
    unsafe {
        core::arch::asm!("ldr xzr, [{address}]", address = in(reg) 0x7000_0000_1000u64,
            options(nostack));
    }
    crate::serial_line::Line::unattributed().text("[GPU_FAULT_TEST kernel unexpectedly resumed]\n");
}
