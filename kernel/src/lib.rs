#![no_std]
#![cfg_attr(test, no_main)]
#![feature(custom_test_frameworks)]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

#[cfg(target_arch = "x86_64")]
pub mod serial;
#[cfg(target_arch = "aarch64")]
pub mod serial_aarch64;
#[cfg(target_arch = "aarch64")]
pub use serial_aarch64 as serial;
pub mod drivers;
pub mod memory;
pub mod arch_impl;
#[cfg(target_arch = "x86_64")]
pub mod gdt;
#[cfg(target_arch = "x86_64")]
pub mod interrupts;
#[cfg(target_arch = "x86_64")]
pub mod per_cpu;
#[cfg(target_arch = "aarch64")]
pub mod per_cpu_aarch64;
#[cfg(target_arch = "aarch64")]
pub use per_cpu_aarch64 as per_cpu;
pub mod process;
pub mod task;
pub mod signal;
#[cfg(target_arch = "x86_64")]
pub mod tls;
#[cfg(target_arch = "x86_64")]
pub mod elf;
#[cfg(target_arch = "aarch64")]
pub use arch_impl::aarch64::elf;
pub mod ipc;
#[cfg(target_arch = "x86_64")]
pub mod keyboard;
pub mod tty;
#[cfg(target_arch = "x86_64")]
pub mod irq_log;
#[cfg(target_arch = "x86_64")]
pub mod userspace_test;
// Syscall module - enabled for both architectures
// Individual submodules have cfg guards for arch-specific code
pub mod syscall;
// Socket module - enabled for both architectures
// Unix domain sockets are fully arch-independent
pub mod socket;
#[cfg(target_arch = "x86_64")]
pub mod test_exec;
pub mod time;
pub mod net;
// Block and filesystem modules - enabled for both architectures
// ARM64 uses VirtIO MMIO block driver, x86_64 uses VirtIO PCI
pub mod block;
pub mod fs;
pub mod logger;
#[cfg(target_arch = "x86_64")]
pub mod framebuffer;
// Graphics module: available on x86_64 with "interactive" feature, or always on ARM64
#[cfg(any(feature = "interactive", target_arch = "aarch64"))]
pub mod graphics;
// Shell module: ARM64-only for now (kernel-mode shell)
#[cfg(target_arch = "aarch64")]
pub mod shell;
// Boot utilities (test disk loader, etc.)
pub mod boot;
// Lock-free tracing for critical paths (interrupt handlers, context switch, etc.)
pub mod trace;
// DTrace-style tracing framework with per-CPU ring buffers
pub mod tracing;
// Kernel log ring buffer for /proc/kmsg
pub mod log_buffer;
// Parallel boot test framework and BTRT
#[cfg(any(feature = "boot_tests", feature = "btrt"))]
pub mod test_framework;

// =========================================================================
// Modules migrated from main.rs for unified crate structure (Phase 2A)
// These are x86_64-only modules that were previously declared only in main.rs.
// #[allow(dead_code)] is applied because these modules export symbols consumed
// by main.rs (the binary crate), not by lib.rs itself.
// =========================================================================
#[cfg(target_arch = "x86_64")]
#[macro_use]
#[allow(dead_code)]
pub mod macros;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub mod clock_gettime_test;

#[cfg(all(target_arch = "x86_64", feature = "interactive"))]
#[allow(dead_code)]
pub mod terminal_emulator;

#[cfg(all(target_arch = "x86_64", feature = "testing"))]
#[allow(dead_code)]
pub mod gdt_tests;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub mod test_checkpoints;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub mod rtc_test;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub mod spinlock;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub mod time_test;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub mod userspace_fault_tests;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub mod preempt_count_test;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub mod stack_switch;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub mod test_userspace;

#[cfg(all(target_arch = "x86_64", feature = "testing"))]
#[allow(dead_code)]
pub mod contracts;

#[cfg(all(target_arch = "x86_64", feature = "testing"))]
#[allow(dead_code)]
pub mod contract_runner;

#[cfg(test)]
use bootloader_api::{entry_point, BootInfo};

#[cfg(test)]
entry_point!(test_kernel_main);

#[cfg(test)]
fn test_kernel_main(_boot_info: &'static mut BootInfo) -> ! {
    serial::init();
    test_main();
    hlt_loop();
}

pub fn test_runner(tests: &[&dyn Testable]) {
    serial_println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }
    exit_qemu(QemuExitCode::Success);
}

pub trait Testable {
    fn run(&self) -> ();
}

impl<T> Testable for T
where
    T: Fn(),
{
    fn run(&self) {
        serial_print!("{}...\t", core::any::type_name::<T>());
        self();
        serial_println!("[ok]");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

pub fn exit_qemu(exit_code: QemuExitCode) {
    #[cfg(target_arch = "x86_64")]
    {
        use x86_64::instructions::port::Port;
        unsafe {
            let mut port = Port::new(0xf4);
            port.write(exit_code as u32);
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        let _ = exit_code;
        // PSCI SYSTEM_OFF causes QEMU to exit cleanly with -no-reboot
        unsafe {
            core::arch::asm!(
                "hvc #0",
                in("x0") 0x8400_0008u64,
                options(nomem, nostack, noreturn),
            );
        }
    }
}

// Re-export x86_64 for tests (x86_64 only)
#[cfg(target_arch = "x86_64")]
pub use x86_64;

#[cfg(test)]
pub fn test_panic_handler(info: &core::panic::PanicInfo) -> ! {
    serial_println!("[failed]\n");
    serial_println!("Error: {}\n", info);
    exit_qemu(QemuExitCode::Failed);
    hlt_loop();
}

#[cfg(target_arch = "x86_64")]
pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

#[cfg(target_arch = "aarch64")]
pub fn hlt_loop() -> ! {
    loop {
        // WFI (Wait For Interrupt) is ARM64 equivalent of HLT
        unsafe { core::arch::asm!("wfi"); }
    }
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    test_panic_handler(info)
}

// ============================================================
// Architecture-generic HAL wrappers
// These dispatch to the correct CpuOps/TimerOps implementation
// so shared kernel code doesn't need #[cfg(target_arch)] blocks.
// ============================================================

use arch_impl::traits::{CpuOps, TimerOps};

/// Disable interrupts, execute `f`, then restore previous interrupt state.
#[inline(always)]
pub fn arch_without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    #[cfg(target_arch = "x86_64")]
    { arch_impl::x86_64::cpu::X86Cpu::without_interrupts(f) }
    #[cfg(target_arch = "aarch64")]
    { arch_impl::aarch64::cpu::Aarch64Cpu::without_interrupts(f) }
}

/// Enable interrupts.
///
/// # Safety
/// Enabling interrupts can cause immediate preemption.
#[inline(always)]
pub unsafe fn arch_enable_interrupts() {
    #[cfg(target_arch = "x86_64")]
    { arch_impl::x86_64::cpu::X86Cpu::enable_interrupts() }
    #[cfg(target_arch = "aarch64")]
    { arch_impl::aarch64::cpu::Aarch64Cpu::enable_interrupts() }
}

/// Disable interrupts.
///
/// # Safety
/// Disabling interrupts can cause deadlocks if not re-enabled.
#[inline(always)]
pub unsafe fn arch_disable_interrupts() {
    #[cfg(target_arch = "x86_64")]
    { arch_impl::x86_64::cpu::X86Cpu::disable_interrupts() }
    #[cfg(target_arch = "aarch64")]
    { arch_impl::aarch64::cpu::Aarch64Cpu::disable_interrupts() }
}

/// Check if interrupts are currently enabled.
#[inline(always)]
pub fn arch_interrupts_enabled() -> bool {
    #[cfg(target_arch = "x86_64")]
    { arch_impl::x86_64::cpu::X86Cpu::interrupts_enabled() }
    #[cfg(target_arch = "aarch64")]
    { arch_impl::aarch64::cpu::Aarch64Cpu::interrupts_enabled() }
}

/// Halt the CPU until the next interrupt.
#[inline(always)]
pub fn arch_halt() {
    #[cfg(target_arch = "x86_64")]
    { arch_impl::x86_64::cpu::X86Cpu::halt() }
    #[cfg(target_arch = "aarch64")]
    { arch_impl::aarch64::cpu::Aarch64Cpu::halt() }
}

/// Enable interrupts and halt (atomic on x86_64).
#[inline(always)]
pub fn arch_halt_with_interrupts() {
    #[cfg(target_arch = "x86_64")]
    { arch_impl::x86_64::cpu::X86Cpu::halt_with_interrupts() }
    #[cfg(target_arch = "aarch64")]
    { arch_impl::aarch64::cpu::Aarch64Cpu::halt_with_interrupts() }
}

/// Read the CPU timestamp counter (raw ticks).
#[inline(always)]
pub fn arch_read_timestamp() -> u64 {
    #[cfg(target_arch = "x86_64")]
    { arch_impl::x86_64::timer::X86Timer::read_timestamp() }
    #[cfg(target_arch = "aarch64")]
    { arch_impl::aarch64::timer::Aarch64Timer::read_timestamp() }
}

/// Convert raw timer ticks to nanoseconds.
#[inline(always)]
pub fn arch_ticks_to_nanos(ticks: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    { arch_impl::x86_64::timer::X86Timer::ticks_to_nanos(ticks) }
    #[cfg(target_arch = "aarch64")]
    { arch_impl::aarch64::timer::Aarch64Timer::ticks_to_nanos(ticks) }
}

#[test_case]
fn trivial_assertion() {
    assert_eq!(1, 1);
}
