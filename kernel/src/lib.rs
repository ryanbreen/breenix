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
#[cfg(target_arch = "x86_64")]
pub mod memory;
pub mod arch_impl;
#[cfg(target_arch = "x86_64")]
pub mod gdt;
#[cfg(target_arch = "x86_64")]
pub mod interrupts;
#[cfg(target_arch = "x86_64")]
pub mod per_cpu;
#[cfg(target_arch = "x86_64")]
pub mod process;
#[cfg(target_arch = "x86_64")]
pub mod task;
#[cfg(target_arch = "x86_64")]
pub mod signal;
#[cfg(target_arch = "x86_64")]
pub mod tls;
#[cfg(target_arch = "x86_64")]
pub mod elf;
#[cfg(target_arch = "x86_64")]
pub mod ipc;
#[cfg(target_arch = "x86_64")]
pub mod keyboard;
#[cfg(target_arch = "x86_64")]
pub mod tty;
#[cfg(target_arch = "x86_64")]
pub mod irq_log;
#[cfg(target_arch = "x86_64")]
pub mod userspace_test;
#[cfg(target_arch = "x86_64")]
pub mod syscall;
#[cfg(target_arch = "x86_64")]
pub mod socket;
#[cfg(target_arch = "x86_64")]
pub mod test_exec;
pub mod time;
#[cfg(target_arch = "x86_64")]
pub mod net;
#[cfg(target_arch = "x86_64")]
pub mod block;
#[cfg(target_arch = "x86_64")]
pub mod fs;
pub mod logger;
#[cfg(target_arch = "x86_64")]
pub mod framebuffer;
#[cfg(feature = "interactive")]
pub mod graphics;

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
        // ARM64: Use semihosting or PSCI for VM exit
        // For now, just halt
        let _ = exit_code;
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

#[test_case]
fn trivial_assertion() {
    assert_eq!(1, 1);
}
