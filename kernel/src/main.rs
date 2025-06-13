#![no_std] // don't link the Rust standard library
#![no_main] // disable all Rust-level entry points
#![feature(abi_x86_interrupt)]

bootloader_api::entry_point!(kernel_main);

mod framebuffer;
mod keyboard;
mod interrupts;

use conquer_once::spin::OnceCell;
use bootloader_x86_64_common::logger::LockedLogger;

pub(crate) static LOGGER: OnceCell<LockedLogger> = OnceCell::uninit();
use bootloader_api::info::FrameBufferInfo;

pub(crate) fn init_logger(buffer: &'static mut [u8], info: FrameBufferInfo) {
    let logger = LOGGER.get_or_init(move || LockedLogger::new(buffer, info, true, false));
    log::set_logger(logger).expect("Logger already set");
    log::set_max_level(log::LevelFilter::Trace);
    log::info!("Hello, Kernel Mode!");
}

fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    // Get framebuffer and initialize logger
    let framebuffer = boot_info.framebuffer.as_mut().unwrap();
    let frame_buffer_info = framebuffer.info().clone();
    let raw_frame_buffer = framebuffer.buffer_mut();
    
    // Initialize the logger with the framebuffer
    init_logger(raw_frame_buffer, frame_buffer_info);
    
    log::info!("Initializing kernel systems...");
    
    // Initialize interrupt descriptor table
    interrupts::init_idt();
    log::info!("IDT initialized");
    
    // Initialize keyboard queue
    keyboard::init();
    log::info!("Keyboard queue initialized");
    
    // Initialize PIC and enable interrupts
    log::info!("Initializing PIC...");
    interrupts::init_pic();
    log::info!("PIC initialized");
    
    unsafe {
        log::info!("Enabling interrupts...");
        x86_64::instructions::interrupts::enable();
    }
    log::info!("Interrupts enabled!");
    
    // Test if interrupts are working by triggering a breakpoint
    log::info!("Testing breakpoint interrupt...");
    x86_64::instructions::interrupts::int3();
    log::info!("Breakpoint test completed!");
    
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