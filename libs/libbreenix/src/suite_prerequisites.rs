//! Measurement prerequisites shared by the effort suites.
//!
//! These return strings so the suite runner's `From<String> for CaseError` makes
//! missing coverage a failure. Passing a prerequisite never passes the case: the
//! caller still has to execute and check the operation being measured.

use std::{format, string::String};

/// CPU cases require simultaneous execution, not time slicing on fewer CPUs.
pub fn cpus(online: usize, required: usize) -> Result<(), String> {
    if online < required {
        Err(format!("CPU prerequisite: online={online}, required>={required} for simultaneous workers and controller/idle CPUs"))
    } else {
        Ok(())
    }
}

pub fn priority_inheritance(offered: i64) -> Result<(), String> {
    if offered <= 0 {
        Err(format!("priority inheritance unavailable: _SC_THREAD_PRIO_INHERIT={offered}; requires >0 to measure PTHREAD_PRIO_INHERIT"))
    } else {
        Ok(())
    }
}

pub fn long_argument(max: u64) -> Result<(), String> {
    if max < 131072 {
        Err(format!("ARG_MAX={max} bytes; requires >=131072 for a 65536-byte argument and near-limit exec"))
    } else {
        Ok(())
    }
}

pub fn argument_allocation(max: u64, count: u64) -> Result<(), String> {
    if count > 2048 {
        Err(format!("E2BIG prerequisite: ARG_MAX={max}, needs {count} x 4096 bytes including NUL; case allocation bound=8388608 bytes"))
    } else {
        Ok(())
    }
}

pub fn stack(soft: u64, depth_kib: usize) -> Result<(), String> {
    let required = 2 * 1024 * depth_kib as u64;
    if soft < required {
        Err(format!("RLIMIT_STACK={soft} bytes; requires >={required} for {depth_kib} KiB control before testing the 256 KiB limit"))
    } else {
        Ok(())
    }
}

pub fn invalid_semaphore_value(max: i64) -> Result<u32, String> {
    if !(0..i64::from(u32::MAX)).contains(&max) {
        Err(format!("SEM_VALUE_MAX={max}; requires 0 <= SEM_VALUE_MAX < {} so SEM_VALUE_MAX+1 fits unsigned int for EINVAL", u32::MAX))
    } else {
        Ok(max as u32 + 1)
    }
}

/// Block allocation can witness writeback only if the stores did not allocate
/// all the required blocks already. Cached readback is not substitute evidence.
pub fn delayed_allocation(stored: i64, required: i64) -> Result<(), String> {
    if stored >= required {
        Err(format!("MS_SYNC unmeasurable: stores allocated {stored}/{required} blocks; need delayed allocation or disk proof; cached readback insufficient"))
    } else {
        Ok(())
    }
}

pub fn idle_measurement(idle_ms: i64, busy_ms: i64, wait_ms: i64, wait_idle_ms: i64, wait_measured: bool) -> Result<(), String> {
    if idle_ms < busy_ms || !wait_measured {
        Err(format!("idle CPU missing: compute idle/busy={idle_ms}/{busy_ms}ms; wake wait/idle={wait_ms}/{wait_idle_ms}ms; spare={wait_measured}"))
    } else {
        Ok(())
    }
}
