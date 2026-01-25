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

        // Use raw pointers to avoid references to mutable statics
        let heap_ptr = &raw mut HEAP;
        let heap_pos_ptr = &raw mut HEAP_POS;

        // Align the current position
        let current_pos = *heap_pos_ptr;
        let aligned_pos = (current_pos + align - 1) & !(align - 1);
        let new_pos = aligned_pos + size;

        if new_pos > (*heap_ptr).len() {
            // Out of memory
            core::ptr::null_mut()
        } else {
            *heap_pos_ptr = new_pos;
            (*heap_ptr).as_mut_ptr().add(aligned_pos)
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
use kernel::arch_impl::aarch64::mmu;
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

    serial_println!("[boot] Initializing MMU...");
    mmu::init();
    serial_println!("[boot] MMU enabled");

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

    // Initialize device drivers (VirtIO MMIO enumeration)
    serial_println!("[boot] Initializing device drivers...");
    let device_count = kernel::drivers::init();
    serial_println!("[boot] Found {} devices", device_count);

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

    // Test syscall mechanism
    serial_println!();
    serial_println!("[test] Testing syscall mechanism...");
    test_syscalls();

    // Test userspace execution
    serial_println!();
    serial_println!("[test] Testing userspace execution...");
    test_userspace();
    // Note: test_userspace() never returns - the user program calls exit()

    serial_println!();
    serial_println!("Entering idle loop (WFI)...");

    // Halt loop
    loop {
        Aarch64Cpu::halt_with_interrupts();
    }
}

/// Test syscalls using SVC instruction from kernel mode.
/// This tests the basic exception handling and syscall dispatch.
fn test_syscalls() {
    // Test write syscall (syscall 1)
    // x8 = syscall number (1 = write)
    // x0 = fd (1 = stdout)
    // x1 = buffer pointer
    // x2 = count
    let message = b"[syscall] Hello from SVC!\n";
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "mov x8, #1",           // syscall number: write
            "mov x0, #1",           // fd: stdout
            "mov x1, {buf}",        // buffer
            "mov x2, {len}",        // count
            "svc #0",               // syscall!
            "mov {ret}, x0",        // return value
            buf = in(reg) message.as_ptr(),
            len = in(reg) message.len(),
            ret = out(reg) ret,
            out("x0") _,
            out("x1") _,
            out("x2") _,
            out("x8") _,
        );
    }
    serial_println!("[test] write() returned: {}", ret);

    // Test getpid syscall (syscall 39)
    let pid: i64;
    unsafe {
        core::arch::asm!(
            "mov x8, #39",          // syscall number: getpid
            "svc #0",               // syscall!
            "mov {pid}, x0",        // return value
            pid = out(reg) pid,
            out("x8") _,
        );
    }
    serial_println!("[test] getpid() returned: {}", pid);

    // Test clock_gettime syscall (syscall 228)
    let mut timespec: [u64; 2] = [0, 0];
    let clock_ret: i64;
    unsafe {
        core::arch::asm!(
            "mov x8, #228",         // syscall number: clock_gettime
            "mov x0, #0",           // CLOCK_REALTIME
            "mov x1, {ts}",         // timespec pointer
            "svc #0",               // syscall!
            "mov {ret}, x0",        // return value
            ts = in(reg) timespec.as_mut_ptr(),
            ret = out(reg) clock_ret,
            out("x0") _,
            out("x1") _,
            out("x8") _,
        );
    }
    if clock_ret == 0 {
        serial_println!("[test] clock_gettime() returned: {}.{:09} seconds", timespec[0], timespec[1]);
    } else {
        serial_println!("[test] clock_gettime() failed with: {}", clock_ret);
    }

    // Test unknown syscall (should return -ENOSYS)
    let enosys: i64;
    unsafe {
        core::arch::asm!(
            "mov x8, #9999",        // invalid syscall number
            "svc #0",               // syscall!
            "mov {ret}, x0",        // return value
            ret = out(reg) enosys,
            out("x8") _,
        );
    }
    serial_println!("[test] unknown syscall returned: {} (expected -38 ENOSYS)", enosys);

    serial_println!("[test] Syscall tests complete!");
}

/// Test userspace execution by transitioning to EL0.
///
/// This creates a minimal ARM64 program in RAM (user-accessible region)
/// that immediately makes a syscall back to the kernel.
fn test_userspace() {
    use kernel::arch_impl::aarch64::context;

    // User program code - a minimal program that:
    // 1. Prints a message via write syscall
    // 2. Exits via exit syscall
    //
    // ARM64 assembly (little-endian encoding):
    //   mov x8, #1           // syscall: write
    //   mov x0, #1           // fd: stdout
    //   adr x1, msg          // buffer: message
    //   mov x2, #28          // count: message length
    //   svc #0               // syscall!
    //   mov x8, #0           // syscall: exit
    //   mov x0, #42          // exit code: 42
    //   svc #0               // syscall!
    // msg:
    //   .ascii "[user] Hello from EL0!\n"
    //
    // Note: We need to carefully craft the message reference since adr uses PC-relative
    // addressing. Instead, we'll embed the message address directly.

    #[repr(align(4))]
    #[allow(dead_code)]  // Fields are used via write_volatile
    struct UserProgram {
        code: [u32; 16],
        message: [u8; 32],
    }

    // Place user program in the user-accessible region (0x4100_0000+)
    // This region has AP=0b01, allowing EL0 to read/write/execute
    // (Note: EL1 cannot execute here due to ARM64 implicit PXN with AP=0b01)
    let user_code_addr: u64 = 0x4100_0000;
    let user_stack_top: u64 = 0x4101_0000; // 64KB above code for stack

    // The message is at offset 0x40 (64 bytes) from code start
    // So full address = 0x4100_0000 + 0x40 = 0x4100_0040
    let program = UserProgram {
        code: [
            // Load message address 0x41000040 into x1
            // movz x1, #0x0040    (x1 = 0x40)
            0xd2800801,
            // movk x1, #0x4100, lsl #16    (x1 = 0x41000040)
            0xf2a82001,

            // mov x8, #1 (write syscall)
            0xd2800028,
            // mov x0, #1 (fd = stdout)
            0xd2800020,
            // mov x2, #24 (message length)
            0xd2800302,
            // svc #0
            0xd4000001,

            // mov x8, #0 (exit syscall)
            0xd2800008,
            // mov x0, #42 (exit code)
            0xd2800540,
            // svc #0
            0xd4000001,

            // Just in case exit doesn't work, spin forever
            // b . (branch to self)
            0x14000000,
            0x14000000,
            0x14000000,
            0x14000000,
            0x14000000,
            0x14000000,
            0x14000000, // 16th element
        ],
        message: *b"[user] Hello from EL0!\n\0\0\0\0\0\0\0\0\0",
    };

    // Copy program to user memory
    unsafe {
        let dst = user_code_addr as *mut UserProgram;
        core::ptr::write_volatile(dst, program);

        // Ensure instruction cache coherency
        // Clean and invalidate data cache, then invalidate instruction cache
        core::arch::asm!(
            "dc cvau, {addr}",        // Clean data cache by VA to PoU
            "dsb ish",                 // Data synchronization barrier
            "ic ivau, {addr}",        // Invalidate instruction cache by VA to PoU
            "dsb ish",                 // Data synchronization barrier
            "isb",                     // Instruction synchronization barrier
            addr = in(reg) user_code_addr,
            options(nostack)
        );
    }

    serial_println!("[test] User program placed at {:#x}", user_code_addr);
    serial_println!("[test] User stack at {:#x}", user_stack_top);
    serial_println!("[test] Transitioning to EL0...");

    // Jump to userspace!
    // Note: return_to_userspace never returns - it uses ERET
    // The user program will exit via syscall, which we handle in exception.rs
    unsafe {
        context::return_to_userspace(user_code_addr, user_stack_top);
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
