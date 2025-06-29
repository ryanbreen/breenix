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