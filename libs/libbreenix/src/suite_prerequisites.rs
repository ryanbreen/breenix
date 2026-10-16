//! Measurement prerequisites shared by the effort suites.
//!
//! Each returns its diagnostic as a string, which the suite runner's
//! `From<String> for CaseError` turns into a FAIL, so a missing prerequisite fails
//! the case. A met prerequisite only lets the case go on: the caller still has to
//! execute and check the operation being measured. Diagnostics stay within the
//! runner's 160-byte message limit.

use std::{format, string::String};

/// CPU cases need `required` processors running at once, which time slicing on
/// fewer cannot stand in for; `purpose` names what the case runs on them.
pub fn cpus(online: usize, required: usize, purpose: &str) -> Result<(), String> {
    if online < required {
        Err(format!("CPU prerequisite: online={online}, required>={required} for {purpose}"))
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

/// The E2BIG case execs ARG_MAX/4096+1 strings of 4096 bytes, NUL included: one
/// more than ARG_MAX holds. Returns that count while the strings fit the case's
/// 8 MiB allocation bound.
pub fn argument_allocation(max: u64) -> Result<u64, String> {
    let count = max / 4096 + 1;
    if count > 2048 {
        Err(format!("E2BIG prerequisite: ARG_MAX={max}, needs {count} x 4096 bytes including NUL; case allocation bound=8388608 bytes"))
    } else {
        Ok(count)
    }
}

/// The stack case uses `depth_kib` of stack under the soft limit as its control,
/// which needs twice that depth, then under a `limit_kib` limit.
pub fn stack(soft: u64, depth_kib: usize, limit_kib: usize) -> Result<(), String> {
    let required = 2 * 1024 * depth_kib as u64;
    if soft < required {
        Err(format!("RLIMIT_STACK={soft} bytes; requires >={required} for {depth_kib} KiB control before testing the {limit_kib} KiB limit"))
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

/// Block counts witness writeback only while the stores have left some of the
/// required blocks unallocated. Cached readback is not substitute evidence.
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
