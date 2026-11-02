//! PSCI system power transitions, using the HVC conduit used by SMP bring-up.

/// Request a system reset. Returns if firmware rejects the request.
pub fn system_reset() {
    power_call(0x84000009);
}

/// Request system power-off. Returns if firmware rejects the request.
pub fn system_off() {
    power_call(0x84000008);
}

// Panic callers have already emitted their capture: no locks or allocation here.
fn power_call(function_id: u64) {
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inout("x0") function_id => _,
            inout("x1") 0u64 => _,
            inout("x2") 0u64 => _,
            inout("x3") 0u64 => _,
            options(nostack),
        );
    }
}
