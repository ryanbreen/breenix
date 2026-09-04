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
pub mod arch_impl;
pub mod drivers;
#[cfg(target_arch = "x86_64")]
pub mod gdt;
#[cfg(target_arch = "x86_64")]
pub mod interrupts;
pub mod memory;
#[cfg(target_arch = "x86_64")]
pub mod per_cpu;
#[cfg(target_arch = "aarch64")]
pub mod per_cpu_aarch64;
#[cfg(target_arch = "aarch64")]
pub use per_cpu_aarch64 as per_cpu;
#[cfg(target_arch = "x86_64")]
pub mod elf;
pub mod process;
pub mod signal;
pub mod task;
#[cfg(target_arch = "x86_64")]
pub mod tls;
#[cfg(target_arch = "aarch64")]
pub use arch_impl::aarch64::elf;
pub mod ipc;
pub mod irq_safe_mutex;
#[cfg(target_arch = "x86_64")]
pub mod irq_log;
#[cfg(target_arch = "x86_64")]
pub mod keyboard;
pub mod tty;
#[cfg(target_arch = "x86_64")]
pub mod userspace_test;
// Syscall module - enabled for both architectures
// Individual submodules have cfg guards for arch-specific code
pub mod syscall;
// Socket module - enabled for both architectures
// Unix domain sockets are fully arch-independent
pub mod net;
pub mod socket;
#[cfg(target_arch = "x86_64")]
pub mod test_exec;
pub mod time;
// Block and filesystem modules - enabled for both architectures
// ARM64 uses VirtIO MMIO block driver, x86_64 uses VirtIO PCI
pub mod block;
#[cfg(target_arch = "x86_64")]
pub mod framebuffer;
pub mod fs;
pub mod logger;
#[cfg(target_arch = "aarch64")]
pub mod platform_config;
// Graphics module: available on x86_64 with "interactive" feature, or always on ARM64
#[cfg(any(feature = "interactive", target_arch = "aarch64"))]
pub mod graphics;
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

// ---------------------------------------------------------------------------
// Core-proof harness (R60 pilot).
//
// The module itself is test-profile only. The `proof_point!` seam macro is
// declared HERE, outside the cfg, because a seam has to be writable at a
// production call site: without the feature the macro exists and expands to
// LITERALLY NOTHING -- no call, no argument evaluation, no token -- so a
// production build cannot carry a byte of it. That is the language-level form
// of the non-negotiable, rather than a promise that the optimiser will inline
// an empty function away.
//
// The pilot's driver, stimulus battery and pen are AArch64-only: they reach the
// GIC, the virtual timer and `kthread_run_on_cpu_for_test`, none of which the
// x86 side offers today. The seams themselves live in arch-shared
// `task/scheduler.rs`, so `proof_point!` must still COMPILE on x86 — hence the
// arch in the cfg on both the module and the macro's active arm rather than on
// the feature alone. An x86 driver is the #608 hunt's own work and is not in
// this pilot; saying so here is cheaper than a build that fails for whoever
// first types `--features coreproof` on the x86 target.
// ---------------------------------------------------------------------------
#[cfg(all(feature = "coreproof", target_arch = "aarch64"))]
pub mod proof;

/// A labelled perturbation seam.
///
/// `proof_point!(SITE)` names a semantically meaningful point in a protocol --
/// "after the state store, before the departure" -- so the harness can place a
/// perturbation there instead of sampling the instruction stream uniformly and
/// hoping to land in a window a few instructions wide.
///
/// Under `coreproof` the fast path is one relaxed load of this CPU's armed-site
/// word plus a compare; the perturbation itself is out of line. Without
/// `coreproof` the macro expands to nothing at all.
#[cfg(all(feature = "coreproof", target_arch = "aarch64"))]
#[macro_export]
macro_rules! proof_point {
    ($site:ident) => {
        $crate::proof::seam($crate::proof::SiteId::$site)
    };
}

#[cfg(not(all(feature = "coreproof", target_arch = "aarch64")))]
#[macro_export]
macro_rules! proof_point {
    ($($ignored:tt)*) => {};
}

/// Count execution of a mutation-hosting region during the measured window.
///
/// This has the same two polarities as `proof_point!`: on the AArch64
/// core-proof profile it reaches the relaxed coverage fast path, and in every
/// other build it expands to literally nothing, including its argument.
#[cfg(all(feature = "coreproof", target_arch = "aarch64"))]
#[macro_export]
macro_rules! proof_cover {
    ($site:ident) => {
        $crate::proof::coverage::note($crate::proof::coverage::MutSite::$site)
    };
}

#[cfg(not(all(feature = "coreproof", target_arch = "aarch64")))]
#[macro_export]
macro_rules! proof_cover {
    ($($ignored:tt)*) => {};
}

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
        unsafe {
            core::arch::asm!("wfi");
        }
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
    {
        arch_impl::x86_64::cpu::X86Cpu::without_interrupts(f)
    }
    #[cfg(target_arch = "aarch64")]
    {
        arch_impl::aarch64::cpu::Aarch64Cpu::without_interrupts(f)
    }
}

/// Enable interrupts.
///
/// # Safety
/// Enabling interrupts can cause immediate preemption.
#[inline(always)]
pub unsafe fn arch_enable_interrupts() {
    #[cfg(target_arch = "x86_64")]
    {
        arch_impl::x86_64::cpu::X86Cpu::enable_interrupts()
    }
    #[cfg(target_arch = "aarch64")]
    {
        arch_impl::aarch64::cpu::Aarch64Cpu::enable_interrupts()
    }
}

/// Disable interrupts.
///
/// # Safety
/// Disabling interrupts can cause deadlocks if not re-enabled.
#[inline(always)]
pub unsafe fn arch_disable_interrupts() {
    #[cfg(target_arch = "x86_64")]
    {
        arch_impl::x86_64::cpu::X86Cpu::disable_interrupts()
    }
    #[cfg(target_arch = "aarch64")]
    {
        arch_impl::aarch64::cpu::Aarch64Cpu::disable_interrupts()
    }
}

/// Check if interrupts are currently enabled.
#[inline(always)]
pub fn arch_interrupts_enabled() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        arch_impl::x86_64::cpu::X86Cpu::interrupts_enabled()
    }
    #[cfg(target_arch = "aarch64")]
    {
        arch_impl::aarch64::cpu::Aarch64Cpu::interrupts_enabled()
    }
}

/// Halt the CPU until the next interrupt.
///
/// A park primitive, so the per-thread park count the #772 revisit oracle
/// reads is bumped here as well as in `arch_halt_with_interrupts` below. The
/// two together are why the oracle's sentence can be checked by grep: see that
/// function's doc for the full park census, including what is deliberately
/// left out of it.
///
/// Idle and terminal halt loops that reach this bump the idle thread's own
/// count. That is harmless: the count has one reader, the identical-frame
/// split at the save site, and it compares a thread's count only against a
/// stamp taken from the same thread at its own dispatch.
// claim-lint:ok: 1 of 1 load of `Thread::wait_loop_iters` outside
// `Thread::clone` is `per_cpu::current_wait_loop_iters`, whose 2 of 2 callers
// are the dispatch stamp and `classify_no_progress_kind`; counted by grep in
// this slot.
#[inline(always)]
pub fn arch_halt() {
    per_cpu::note_wait_loop_park();
    #[cfg(target_arch = "x86_64")]
    {
        arch_impl::x86_64::cpu::X86Cpu::halt()
    }
    #[cfg(target_arch = "aarch64")]
    {
        arch_impl::aarch64::cpu::Aarch64Cpu::halt()
    }
}

/// Enable interrupts and halt (atomic on x86_64).
///
/// The park primitive the blocking-in-syscall wait loops use, and where the
/// per-thread park count the #772 revisit oracle reads is bumped. Ruling R113
/// (2026-09-03) retired the proxy; this oracle is its replacement.
///
/// Counting inside the primitive rather than at each call site is deliberate:
/// a site added later is counted with no edit, and a site cannot drift out of
/// the census by being missed. The bump is immediately before the halt, so the
/// count a thread carries while parked already includes the park it is sitting
/// in.
///
/// What is counted, and what is not. The park census is
/// `grep -rn "arch_halt()\|arch_halt_with_interrupts()\|enable_and_hlt\|wfi()"
/// kernel/src --include=*.rs`, minus the primitive definitions themselves and
/// the lines that only name a primitive in prose.
/// Counted: all 25 call sites of this primitive, all 24 call sites of
/// `arch_halt`, the two call sites of the private halt primitive in
/// `graphics/render_task.rs` (which bumps in the same shape), and the two
/// loops that park on a raw `enable_and_hlt`/`wfi` of their own and call
/// `per_cpu::note_wait_loop_park` directly -- `task/executor.rs`'s
/// `sleep_if_idle` and `task/spawn.rs`'s `idle_thread_fn`.
/// NOT counted: the five idle and terminal halt loops that park on a raw
/// `enable_and_hlt` -- four in `main.rs` and `idle_loop` in
/// `interrupts/context_switch.rs`. None of the five is a blocking wait loop
/// waiting on a condition, but the omission is not free: a thread saved at a
/// byte-identical frame while parked in one of them reads ZERO_ITER even
/// though it re-parked. The idle thread is the only thread that reaches them.
/// The census grep above also does not reach a raw `asm!("wfi")`, which is how
/// `hlt_loop`, the aarch64 boot and panic paths in `main_aarch64.rs`, and the
/// `ec0_fault_inject` thread park; none of those is a blocking wait loop
/// either, and none is counted.
// claim-lint:ok: 25 of 25 arch_halt_with_interrupts call sites and 24 of 24
// arch_halt call sites under kernel/src reach a bump, counted by grep in this
// slot.
///
/// One relaxed atomic add on a path that is about to halt the CPU. No lock, no
/// allocation, no formatting, and no control flow reads the value.
// claim-lint:ok: docs/planning/green-program/sockets/772-DIAG-2026-09-03.md
#[inline(always)]
pub fn arch_halt_with_interrupts() {
    per_cpu::note_wait_loop_park();
    #[cfg(target_arch = "x86_64")]
    {
        arch_impl::x86_64::cpu::X86Cpu::halt_with_interrupts()
    }
    #[cfg(target_arch = "aarch64")]
    {
        arch_impl::aarch64::cpu::Aarch64Cpu::halt_with_interrupts()
    }
}

/// Read the CPU timestamp counter (raw ticks).
#[inline(always)]
pub fn arch_read_timestamp() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        arch_impl::x86_64::timer::X86Timer::read_timestamp()
    }
    #[cfg(target_arch = "aarch64")]
    {
        arch_impl::aarch64::timer::Aarch64Timer::read_timestamp()
    }
}

/// Convert raw timer ticks to nanoseconds.
#[inline(always)]
pub fn arch_ticks_to_nanos(ticks: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        arch_impl::x86_64::timer::X86Timer::ticks_to_nanos(ticks)
    }
    #[cfg(target_arch = "aarch64")]
    {
        arch_impl::aarch64::timer::Aarch64Timer::ticks_to_nanos(ticks)
    }
}

#[test_case]
fn trivial_assertion() {
    assert_eq!(1, 1);
}
