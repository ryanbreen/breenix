//! KTAP (Kernel Test Anything Protocol) serial formatter.
//!
//! Emits KTAP-formatted lines to serial output for live progress
//! monitoring and GitHub Actions log readability.
//!
//! # Output Format
//!
//! ```text
//! KTAP version 1
//! 1..25
//! ok 1 kernel_entry
//! ok 2 serial_init
//! not ok 3 pci_enumeration # FAIL error_code=2
//! ok 4 timer_init # SKIP not applicable
//! # 23 passed, 1 failed, 1 skipped
//! ```

use crate::serial_println;

/// Emit KTAP header with version and test plan.
pub fn emit_header(total_tests: u32) {
    serial_println!("KTAP version 1");
    serial_println!("1..{}", total_tests);
}

/// Emit a passing test result.
pub fn emit_pass(test_num: u16, name: &str) {
    serial_println!("ok {} {}", test_num, name);
}

/// Emit a failing test result with error code.
pub fn emit_fail(test_num: u16, name: &str, error_code: u8, error_detail: u32) {
    serial_println!(
        "not ok {} {} # FAIL error_code={} detail={:#x}",
        test_num, name, error_code, error_detail
    );
}

/// Emit a skipped test result.
pub fn emit_skip(test_num: u16, name: &str) {
    serial_println!("ok {} {} # SKIP", test_num, name);
}

/// Emit a timeout test result.
pub fn emit_timeout(test_num: u16, name: &str) {
    serial_println!("not ok {} {} # TIMEOUT", test_num, name);
}

/// Emit the summary line.
pub fn emit_summary(passed: u32, failed: u32, skipped: u32) {
    serial_println!("# {} passed, {} failed, {} skipped", passed, failed, skipped);
}
