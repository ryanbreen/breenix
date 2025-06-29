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
    
    // Run tests if testing feature is enabled
    #[cfg(feature = "testing")]
    {
        log::info!("Running kernel tests...");
        gdt_tests::run_all_tests();
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
    
    log::info!("Press keys to see their scancodes...");
    
    let mut key_count = 0;
    
    loop {
        // Check for keyboard input
        if let Some(scancode) = keyboard::read_scancode() {
            if scancode < 0x80 {
                // Key press
                key_count += 1;
                log::info!("Main loop: Key #{} pressed, scancode=0x{:02x}", key_count, scancode);
            } else {
                // Key release
                log::info!("Main loop: Key released, scancode=0x{:02x}", scancode);
            }
        }
        
        // Use hlt to wait for next interrupt
        x86_64::instructions::hlt();
    }
}

use core::panic::PanicInfo;

/// This function is called on panic.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}