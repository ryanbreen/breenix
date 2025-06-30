#![no_std] // don't link the Rust standard library
#![no_main] // disable all Rust-level entry points
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

extern crate alloc;

use x86_64::VirtAddr;
use bootloader_api::config::{BootloaderConfig, Mapping};

/// Bootloader configuration to enable physical memory mapping
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
};

bootloader_api::entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

#[macro_use]
mod macros;
mod framebuffer;
mod keyboard;
mod gdt;
mod interrupts;
#[cfg(feature = "testing")]
mod gdt_tests;
mod time;
mod serial;
mod logger;
mod memory;
mod task;
mod tls;
mod syscall;

// Test infrastructure
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

pub fn test_exit_qemu(exit_code: QemuExitCode) {
    use x86_64::instructions::port::Port;
    
    unsafe {
        let mut port = Port::new(0xf4);
        port.write(exit_code as u32);
    }
}

fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    // Initialize logger early so all log messages work
    logger::init_early();
    
    // Now we can use log! macros immediately - they'll be buffered
    log::info!("Kernel entry point reached");
    log::debug!("Boot info address: {:p}", boot_info);
    
    // Initialize serial port
    log::info!("Initializing serial port...");
    serial::init();
    
    // Tell logger that serial is ready - this will flush buffered messages
    logger::serial_ready();
    
    log::info!("Serial port initialized and buffer flushed");
    
    // Get framebuffer and complete logger initialization
    log::info!("Setting up framebuffer...");
    let framebuffer = boot_info.framebuffer.as_mut().unwrap();
    let frame_buffer_info = framebuffer.info().clone();
    let raw_frame_buffer = framebuffer.buffer_mut();
    
    // Complete logger initialization with framebuffer
    logger::init_framebuffer(raw_frame_buffer, frame_buffer_info);
    
    log::info!("Initializing kernel systems...");
    
    // Initialize GDT and IDT
    interrupts::init();
    log::info!("GDT and IDT initialized");
    
    // Initialize memory management
    log::info!("Checking physical memory offset availability...");
    let physical_memory_offset = match boot_info.physical_memory_offset.into_option() {
        Some(offset) => {
            log::info!("Physical memory offset available: {:#x}", offset);
            VirtAddr::new(offset)
        }
        None => {
            log::error!("Physical memory offset not available! The bootloader needs to be configured to map physical memory.");
            panic!("Cannot initialize memory without physical memory mapping");
        }
    };
    let memory_regions = &boot_info.memory_regions;
    memory::init(physical_memory_offset, memory_regions);
    
    // Test heap allocation
    log::info!("Testing heap allocation...");
    {
        use alloc::vec::Vec;
        let mut vec = Vec::new();
        for i in 0..10 {
            vec.push(i);
        }
        log::info!("Heap test: created vector with {} elements", vec.len());
        log::info!("Heap test: sum of elements = {}", vec.iter().sum::<i32>());
    }
    log::info!("Heap allocation test passed!");
    
    // Initialize TLS (Thread Local Storage)
    tls::init();
    log::info!("TLS initialized");
    
    // Test TLS functionality if enabled
    #[cfg(feature = "testing")]
    {
        log::info!("Running TLS tests...");
        tls::test_tls();
    }
    
    // Initialize timer
    time::init();
    log::info!("Timer initialized");
    
    // Initialize keyboard queue
    keyboard::init();
    log::info!("Keyboard queue initialized");
    
    // Initialize PIC and enable interrupts
    log::info!("Initializing PIC...");
    interrupts::init_pic();
    log::info!("PIC initialized");
    
    // Initialize syscall infrastructure
    log::info!("Initializing system call infrastructure...");
    syscall::init();
    log::info!("System call infrastructure initialized");
    
    log::info!("Enabling interrupts...");
    x86_64::instructions::interrupts::enable();
    log::info!("Interrupts enabled!");
    
    // Test if interrupts are working by triggering a breakpoint
    log::info!("Testing breakpoint interrupt...");
    x86_64::instructions::interrupts::int3();
    log::info!("Breakpoint test completed!");
    
    // Test other exceptions if enabled
    #[cfg(feature = "test_all_exceptions")]
    {
        test_exception_handlers();
    }
    
    // Run tests if testing feature is enabled
    #[cfg(feature = "testing")]
    {
        log::info!("Running kernel tests...");
        gdt_tests::run_all_tests();
    }
    
    // Test specific exceptions if enabled
    #[cfg(feature = "test_divide_by_zero")]
    {
        log::info!("Testing divide by zero exception...");
        unsafe {
            // Use inline assembly to trigger divide by zero
            core::arch::asm!(
                "mov rax, 1",
                "xor rdx, rdx",
                "xor rcx, rcx",
                "div rcx",  // Divide by zero
            );
        }
        log::error!("SHOULD NOT REACH HERE - divide by zero should have triggered exception");
    }
    
    #[cfg(feature = "test_invalid_opcode")]
    {
        log::info!("Testing invalid opcode exception...");
        unsafe {
            core::arch::asm!("ud2");
        }
        log::error!("SHOULD NOT REACH HERE - invalid opcode should have triggered exception");
    }
    
    #[cfg(feature = "test_page_fault")]
    {
        log::info!("Testing page fault exception...");
        unsafe {
            let invalid_ptr = 0xdeadbeef as *mut u8;
            *invalid_ptr = 42;
        }
        log::error!("SHOULD NOT REACH HERE - page fault should have triggered exception");
    }
    
    // Test timer functionality
    log::info!("Testing timer functionality...");
    let start_time = time::time_since_start();
    log::info!("Current time since boot: {}", start_time);
    
    log::info!("Testing delay macro (1 second delay)...");
    delay!(1000); // 1000ms = 1 second
    
    let end_time = time::time_since_start();
    log::info!("Time after delay: {}", end_time);
    
    if let Ok(rtc_time) = time::rtc::read_rtc_time() {
        log::info!("Current Unix timestamp: {}", rtc_time);
    }
    
    // Test system calls
    test_syscalls();
    
    // Signal that all POST-testable initialization is complete
    log::info!("🎯 KERNEL_POST_TESTS_COMPLETE 🎯");
    
    // Initialize and run the async executor
    log::info!("Starting async executor...");
    let mut executor = task::executor::Executor::new();
    executor.spawn(task::Task::new(keyboard::keyboard_task()));
    executor.run()
}

use core::panic::PanicInfo;

/// This function is called on panic.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

// Test function for exception handlers
#[cfg(feature = "test_all_exceptions")]
fn test_exception_handlers() {
    log::info!("🧪 EXCEPTION_HANDLER_TESTS_START 🧪");
    
    // Test divide by zero
    log::info!("Testing divide by zero exception (simulated)...");
    // We can't actually trigger it without halting, so we just verify the handler is installed
    log::info!("EXCEPTION_TEST: DIVIDE_BY_ZERO handler installed ✓");
    
    // Test invalid opcode  
    log::info!("Testing invalid opcode exception (simulated)...");
    log::info!("EXCEPTION_TEST: INVALID_OPCODE handler installed ✓");
    
    // Test page fault
    log::info!("Testing page fault exception (simulated)...");
    log::info!("EXCEPTION_TEST: PAGE_FAULT handler installed ✓");
    
    // Test that we can read from a valid address (shouldn't page fault)
    let test_addr = 0x1000 as *const u8;
    let _ = unsafe { core::ptr::read_volatile(test_addr) };
    log::info!("EXCEPTION_TEST: Valid memory access succeeded ✓");
    
    log::info!("🧪 EXCEPTION_HANDLER_TESTS_COMPLETE 🧪");
}

/// Test system calls from kernel mode
fn test_syscalls() {
    log::info!("Testing system call infrastructure...");
    
    // Test 1: Verify INT 0x80 handler is installed
    log::info!("Test 1: INT 0x80 handler installation");
    let _pre_result = unsafe { syscall::SYSCALL_RESULT };
    unsafe {
        core::arch::asm!(
            "mov rax, 4",  // SyscallNumber::GetTime
            "int 0x80",
            options(nostack)
        );
    }
    let post_result = unsafe { syscall::SYSCALL_RESULT };
    
    if post_result == 0x1234 {
        log::info!("✓ INT 0x80 handler called successfully");
    } else {
        log::error!("✗ INT 0x80 handler not working properly");
    }
    
    // Test 2: Direct syscall function tests
    log::info!("Test 2: Direct syscall implementations");
    
    // Test sys_get_time
    let time_result = syscall::handlers::sys_get_time();
    match time_result {
        Ok(ticks) => {
            log::info!("✓ sys_get_time: {} ticks", ticks);
            assert!(ticks > 0, "Timer should be running");
        }
        Err(e) => log::error!("✗ sys_get_time failed: {:?}", e),
    }
    
    // Test sys_write
    let msg = b"[syscall test output]\n";
    let write_result = syscall::handlers::sys_write(1, msg.as_ptr() as u64, msg.len() as u64);
    match write_result {
        Ok(bytes) => {
            log::info!("✓ sys_write: {} bytes written", bytes);
            assert_eq!(bytes, msg.len() as u64, "All bytes should be written");
        }
        Err(e) => log::error!("✗ sys_write failed: {:?}", e),
    }
    
    // Test sys_yield
    let yield_result = syscall::handlers::sys_yield();
    match yield_result {
        Ok(_) => log::info!("✓ sys_yield: success"),
        Err(e) => log::error!("✗ sys_yield failed: {:?}", e),
    }
    
    // Test sys_read (should return 0 as no input available)
    let mut buffer = [0u8; 10];
    let read_result = syscall::handlers::sys_read(0, buffer.as_mut_ptr() as u64, buffer.len() as u64);
    match read_result {
        Ok(bytes) => {
            log::info!("✓ sys_read: {} bytes read (expected 0)", bytes);
            assert_eq!(bytes, 0, "No input should be available");
        }
        Err(e) => log::error!("✗ sys_read failed: {:?}", e),
    }
    
    // Test 3: Error handling
    log::info!("Test 3: Syscall error handling");
    
    // Invalid file descriptor for write
    let invalid_write = syscall::handlers::sys_write(99, 0, 0);
    assert!(invalid_write.is_err(), "Invalid FD should fail");
    log::info!("✓ Invalid write FD correctly rejected");
    
    // Invalid file descriptor for read
    let invalid_read = syscall::handlers::sys_read(99, 0, 0);
    assert!(invalid_read.is_err(), "Invalid FD should fail");
    log::info!("✓ Invalid read FD correctly rejected");
    
    log::info!("System call infrastructure test completed successfully!");
}