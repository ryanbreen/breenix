//! ARM64 kernel entry point and initialization.
//!
//! This file contains the AArch64-specific kernel entry point.
//! It's completely separate from the x86_64 boot path which uses
//! the rust-osdev bootloader.
//!
//! Boot sequence:
//! 1. _start (assembly) - Set up stack, zero BSS, jump to kernel_main
//! 2. kernel_main - Initialize serial, timer, GIC, print "Hello"
//! 3. Eventually: Set up MMU, exceptions, userspace

#![no_std]
#![no_main]
#![cfg(target_arch = "aarch64")]
#![feature(alloc_error_handler)]

extern crate alloc;
extern crate rlibc; // Provides memcpy, memset, etc.

use core::panic::PanicInfo;
use core::alloc::{GlobalAlloc, Layout};

// Import the kernel library macros and modules
#[macro_use]
extern crate kernel;

// =============================================================================
// Simple bump allocator for early boot
// This is temporary - will be replaced by proper heap allocator later
// =============================================================================

/// Simple bump allocator that uses a fixed buffer
struct BumpAllocator;

/// 256KB heap buffer for early boot allocations
static mut HEAP: [u8; 256 * 1024] = [0; 256 * 1024];
static mut HEAP_POS: usize = 0;

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();

        // Align the current position
        let aligned_pos = (HEAP_POS + align - 1) & !(align - 1);
        let new_pos = aligned_pos + size;

        if new_pos > HEAP.len() {
            // Out of memory
            core::ptr::null_mut()
        } else {
            HEAP_POS = new_pos;
            HEAP.as_mut_ptr().add(aligned_pos)
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator doesn't support deallocation
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

#[alloc_error_handler]
fn alloc_error_handler(layout: Layout) -> ! {
    panic!("allocation error: {:?}", layout)
}

use kernel::serial;
use kernel::arch_impl::aarch64::timer;
use kernel::arch_impl::aarch64::cpu::Aarch64Cpu;
use kernel::arch_impl::aarch64::gic::Gicv2;
use kernel::arch_impl::traits::{CpuOps, InterruptController};

/// Kernel entry point called from assembly boot code.
///
/// At this point:
/// - We're running at EL1 (or need to drop from EL2)
/// - Stack is set up
/// - BSS is zeroed
/// - MMU is off (identity mapped by UEFI or running physical)
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    // Initialize serial output first so we can print
    serial::init_serial();

    serial_println!();
    serial_println!("========================================");
    serial_println!("  Breenix ARM64 Kernel Starting");
    serial_println!("========================================");
    serial_println!();

    // Print CPU info
    let el = current_exception_level();
    serial_println!("[boot] Current exception level: EL{}", el);

    // Initialize timer
    serial_println!("[boot] Initializing Generic Timer...");
    timer::calibrate();
    let freq = timer::frequency_hz();
    serial_println!("[boot] Timer frequency: {} Hz ({} MHz)", freq, freq / 1_000_000);

    // Read current timestamp
    let ts = timer::rdtsc();
    serial_println!("[boot] Current timestamp: {}", ts);

    // Initialize GIC
    serial_println!("[boot] Initializing GICv2...");
    Gicv2::init();
    serial_println!("[boot] GIC initialized");

    // Enable interrupts
    serial_println!("[boot] Enabling interrupts...");
    unsafe { Aarch64Cpu::enable_interrupts(); }
    let irq_enabled = Aarch64Cpu::interrupts_enabled();
    serial_println!("[boot] Interrupts enabled: {}", irq_enabled);

    serial_println!();
    serial_println!("========================================");
    serial_println!("  Breenix ARM64 Boot Complete!");
    serial_println!("========================================");
    serial_println!();
    serial_println!("Hello from ARM64!");
    serial_println!();

    // Show time passing
    let start = timer::rdtsc();
    for i in 0..5 {
        // Busy wait approximately 1 second
        let target = start + (i + 1) * freq;
        while timer::rdtsc() < target {
            core::hint::spin_loop();
        }
        if let Some((secs, nanos)) = timer::monotonic_time() {
            serial_println!("[{}] Uptime: {}.{:09} seconds", i + 1, secs, nanos);
        }
    }

    serial_println!();
    serial_println!("Entering idle loop (WFI)...");

    // Halt loop
    loop {
        Aarch64Cpu::halt_with_interrupts();
    }
}

/// Read current exception level from CurrentEL register
fn current_exception_level() -> u8 {
    let el: u64;
    unsafe {
        core::arch::asm!("mrs {}, currentel", out(reg) el, options(nomem, nostack));
    }
    ((el >> 2) & 0x3) as u8
}

/// Panic handler
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!();
    serial_println!("========================================");
    serial_println!("  KERNEL PANIC!");
    serial_println!("========================================");
    serial_println!("{}", info);
    serial_println!();

    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
    }
}
