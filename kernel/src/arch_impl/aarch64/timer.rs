//! ARM64 Generic Timer (CNTVCT_EL0, CNTFRQ_EL0) operations.
#![allow(dead_code)]

use crate::arch_impl::traits::TimerOps;

pub struct Aarch64Timer;

impl TimerOps for Aarch64Timer {
    fn read_timestamp() -> u64 {
        unimplemented!("ARM64: read_timestamp (CNTVCT_EL0) not yet implemented")
    }

    fn frequency_hz() -> Option<u64> {
        unimplemented!("ARM64: frequency_hz (CNTFRQ_EL0) not yet implemented")
    }

    fn ticks_to_nanos(ticks: u64) -> u64 {
        let _ = ticks;
        unimplemented!("ARM64: ticks_to_nanos not yet implemented")
    }
}
