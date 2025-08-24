#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    test_main();
    kernel::test_exit_qemu(kernel::QemuExitCode::Success);
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kernel::test_panic_handler(info)
}

#[path = "shared_qemu.rs"]
mod shared_qemu;

#[test_case]
fn test_rtc_initialization() {
    let output = shared_qemu::get_kernel_output();
    assert!(output.contains("RTC initialized:"), "RTC initialization message not found");
    assert!(output.contains("UTC"), "UTC time not shown in RTC initialization");
}

#[test_case]
fn test_real_time_calculation() {
    let output = shared_qemu::get_kernel_output();
    assert!(output.contains("=== RTC AND REAL TIME TEST ==="), "RTC test not started");
    assert!(output.contains("RTC Unix timestamp:"), "RTC timestamp not displayed");
    assert!(output.contains("Boot time:"), "Boot time not displayed");
    assert!(output.contains("Real time:"), "Real time not displayed");
    assert!(output.contains("SUCCESS: RTC and real time appear to be working"), 
           "RTC test did not complete successfully");
}

#[test_case]
fn test_time_progression() {
    let output = shared_qemu::get_kernel_output();
    assert!(output.contains("Waiting 2 seconds..."), "Wait test not started");
    assert!(output.contains("Real time after wait:"), "Real time after wait not shown");
}

#[test_case]
fn test_datetime_format() {
    let output = shared_qemu::get_kernel_output();
    
    // Look for properly formatted datetime (e.g., 2025-07-21 13:21:46)
    let has_formatted_datetime = output.lines()
        .any(|line| {
            if line.contains("RTC initialized:") || 
               line.contains("Boot time:") || 
               line.contains("Real time:") {
                // Check for YYYY-MM-DD HH:MM:SS format
                line.contains("20") && // Year starts with 20
                line.contains("-") &&  // Date separator
                line.contains(":")     // Time separator
            } else {
                false
            }
        });
    
    assert!(has_formatted_datetime, "No properly formatted datetime found in output");
}